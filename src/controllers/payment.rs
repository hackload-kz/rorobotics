//! payment.rs
//!
//! Модуль отвечает за всю логику, связанную с платежами:
//! - Инициация платежа для бронирования.
//! - Обработка вебхуков от платежной системы.
//! - Проверка статуса платежа.
//! - Обработка успешного и неуспешного завершения оплаты.
//! - Мониторинг состояния Circuit Breaker для платежного шлюза.

use axum::{
    extract::{State, Json, Path},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use chrono::Utc;

use crate::{
    AppState,
    middleware::AuthUser,
    services::payment::PaymentGatewayClient,
};

// --- Модели запросов и ответов ---

/// Модель запроса на инициацию платежа.
#[derive(Debug, Deserialize)]
pub struct InitiatePaymentRequest {
    pub booking_id: i64,
}

/// Стандартная структура для ответа с ошибкой API.
#[derive(Serialize)]
pub struct ApiError {
    success: bool,
    message: String,
    code: Option<i32>,
}

/// Тип-обертка для стандартизации ответов API.
type ApiResult<T> = Result<T, (StatusCode, Json<ApiError>)>;

/// Вспомогательная функция для создания стандартного ответа с ошибкой.
fn to_api_error(status: StatusCode, message: &str) -> (StatusCode, Json<ApiError>) {
    (status, Json(ApiError { success: false, message: message.to_string(), code: None }))
}

/// Вспомогательная функция для создания ответа с ошибкой, включающего код ошибки.
fn to_api_error_with_code(status: StatusCode, message: &str, code: i32) -> (StatusCode, Json<ApiError>) {
    (status, Json(ApiError { success: false, message: message.to_string(), code: Some(code) }))
}

// --- Обработчики HTTP запросов ---

/// PATCH /api/bookings/initiatePayment
///
/// Инициирует процесс оплаты для существующего бронирования.
/// 1. Проверяет, что бронирование существует, принадлежит пользователю и содержит места.
/// 2. Рассчитывает общую стоимость.
/// 3. Вызывает API платежного шлюза для создания платежной сессии.
/// 4. Сохраняет информацию о транзакции в базу данных.
/// 5. Возвращает URL для перенаправления пользователя на страницу оплаты.
pub async fn initiate_payment(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<InitiatePaymentRequest>,
) -> ApiResult<impl IntoResponse> {
    if req.booking_id <= 0 {
        return Err(to_api_error(StatusCode::BAD_REQUEST, "Booking ID must be > 0"));
    }

    // Получаем из базы данные о бронировании: его ID, название события,
    // общую стоимость, количество мест и email пользователя.
    let booking_data: Option<(i64, String, f64, i32, String)> = sqlx::query_as(
        r#"
        SELECT b.id, e.title,
               COALESCE(SUM(s.price), 0) as total_price,
               COUNT(s.id)::int as seat_count,
               u.email
        FROM bookings b
        JOIN events_archive e ON e.id = b.event_id
        JOIN users u ON u.user_id = b.user_id
        LEFT JOIN seats s ON s.booking_id = b.id AND s.status = 'RESERVED'
        WHERE b.id = $1 AND b.user_id = $2
        GROUP BY b.id, e.title, u.email
        HAVING COUNT(s.id) > 0
        "#
    )
    .bind(req.booking_id)
    .bind(user.user_id)
    .fetch_optional(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Database error getting booking: {:?}", e);
        to_api_error(StatusCode::INTERNAL_SERVER_ERROR, "Database error")
    })?;

    let (booking_id, event_title, total_price, seat_count, user_email) = booking_data
        .ok_or_else(|| to_api_error(StatusCode::NOT_FOUND, "Booking not found or empty"))?;

    // Убедимся, что стоимость бронирования положительная.
    if total_price <= 0.0 {
        return Err(to_api_error(StatusCode::BAD_REQUEST, "Invalid booking price"));
    }

    let payment_client = PaymentGatewayClient::from_config(&state.config.payment, state.clone());
    
    // Платежные системы обычно требуют сумму в минимальных денежных единицах (копейки, центы).
    let amount = (total_price * 100.0) as i64;
    let order_id = format!("booking-{}-{}", booking_id, Utc::now().timestamp());
    let description = format!("{} - {} билет(ов)", event_title, seat_count);

    // Вызываем внешний сервис для создания платежа.
    let payment_response = payment_client.create_payment(
        amount,
        order_id.clone(),
        description.clone(),
        Some(user_email),
        state.config.payment.success_url.clone(),
        state.config.payment.fail_url.clone(),
        state.config.payment.webhook_url.clone(),
    ).await.map_err(|e| {
        // Обработка ошибок, связанных с Circuit Breaker.
        match e {
            crate::services::payment::CircuitBreakerError::Open => {
                tracing::error!("Payment gateway circuit breaker is open");
                to_api_error(StatusCode::SERVICE_UNAVAILABLE, "Payment service temporarily unavailable. Please try again later.")
            },
            crate::services::payment::CircuitBreakerError::PaymentGatewayError(http_err) => {
                tracing::error!("Payment gateway HTTP error: {:?}", http_err);
                to_api_error(StatusCode::BAD_GATEWAY, "Payment gateway connection error")
            }
        }
    })?;

    // Если платежный шлюз вернул ошибку, обрабатываем ее.
    if !payment_response.success {
        let error_msg = payment_response.message.unwrap_or_else(|| "Unknown error".to_string());
        let error_code = payment_response.code.unwrap_or(9999);
        
        tracing::error!("Payment gateway returned error: code={}, message={}", error_code, error_msg);
        
        // Обрабатываем специфические коды ошибок от платежного шлюза.
        let status_code = match error_code {
            1001 => StatusCode::UNAUTHORIZED,       // Ошибка аутентификации
            1002 => StatusCode::CONFLICT,           // Дублирующийся платеж
            1004 => StatusCode::PAYMENT_REQUIRED,   // Недостаточно средств
            1006 => StatusCode::BAD_REQUEST,        // Неподдерживаемая валюта
            3015 => StatusCode::TOO_MANY_REQUESTS,  // Превышение лимита запросов
            _ => StatusCode::BAD_GATEWAY,           // Все остальные ошибки
        };
        
        return Err(to_api_error_with_code(status_code, &error_msg, error_code));
    }

    let payment_id = payment_response.payment_id
        .ok_or_else(|| to_api_error(StatusCode::INTERNAL_SERVER_ERROR, "No payment ID from gateway"))?;

    // Начинаем транзакцию в базе данных.
    let mut tx = state.db.pool.begin().await
        .map_err(|e| {
            tracing::error!("Failed to start DB transaction: {}", e);
            to_api_error(StatusCode::INTERNAL_SERVER_ERROR, "Database error")
        })?;

    // Создаем запись о платежной транзакции.
    sqlx::query(
        "INSERT INTO payment_transactions (booking_id, transaction_id, amount, status)
         VALUES ($1, $2, $3, 'pending')"
    )
    .bind(booking_id)
    .bind(&payment_id)
    .bind(total_price)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!("Failed to save transaction: {}", e);
        to_api_error(StatusCode::INTERNAL_SERVER_ERROR, "Failed to save transaction")
    })?;

    // Обновляем статус бронирования на "ожидает оплаты".
    sqlx::query("UPDATE bookings SET status = 'pending_payment' WHERE id = $1")
        .bind(booking_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            tracing::error!("Failed to update booking: {}", e);
            to_api_error(StatusCode::INTERNAL_SERVER_ERROR, "Failed to update booking")
        })?;

    // Завершаем транзакцию.
    tx.commit().await
        .map_err(|e| {
            tracing::error!("Failed to commit DB transaction: {}", e);
            to_api_error(StatusCode::INTERNAL_SERVER_ERROR, "Database error")
        })?;

    tracing::info!("Payment created for booking {}: payment_id={}, amount={}",
        booking_id, payment_id, total_price);

    // Возвращаем успешный ответ с URL для оплаты.
    Ok((StatusCode::OK, Json(json!({
        "success": true,
        "payment_url": payment_response.payment_url,
        "payment_id": payment_id,
        "amount": total_price,
        "currency": "KZT", // Валюта, которую мы поддерживаем.
        "description": description,
        "expires_at": payment_response.expires_at
    }))))
}

/// POST /api/webhook/payment
///
/// Обрабатывает входящие вебхуки от платежной системы для обновления статуса платежа.
/// Этот эндпоинт является публичным, так как запросы приходят от внешнего сервиса.
pub async fn payment_webhook(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let payment_id = payload["paymentId"].as_str().unwrap_or_default().to_string();
    let status = payload["status"].as_str().unwrap_or_default().to_string();
    
    tracing::info!("Webhook received: payment_id={}, status={}", payment_id, status);

    let payment_client = PaymentGatewayClient::from_config(&state.config.payment, state.clone());

    // Делегируем обработку уведомления специализированному методу.
    payment_client.process_webhook_notification(&payment_id, &status).await;

    // Всегда возвращаем 200 OK, чтобы платежная система не повторяла отправку.
    (StatusCode::OK, Json(json!({"received": true})))
}

/// GET /api/bookings/{booking_id}/payment-status
///
/// Позволяет пользователю проверить статус оплаты своего бронирования.
/// Если статус "pending", дополнительно опрашивает платежный шлюз для получения
/// самой актуальной информации.
pub async fn get_payment_status(
    State(state): State<Arc<AppState>>,
    Path(booking_id): Path<i64>,
    user: AuthUser,
) -> ApiResult<impl IntoResponse> {
    // Получаем последний статус платежа из нашей базы.
    let status: Option<(String, String)> = sqlx::query_as(
        "SELECT pt.status, pt.transaction_id FROM payment_transactions pt
         JOIN bookings b ON b.id = pt.booking_id
         WHERE pt.booking_id = $1 AND b.user_id = $2
         ORDER BY pt.created_at DESC LIMIT 1"
    )
    .bind(booking_id)
    .bind(user.user_id)
    .fetch_optional(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Database error getting payment status: {}", e);
        to_api_error(StatusCode::INTERNAL_SERVER_ERROR, "Database error")
    })?;

    match status {
        Some((status, payment_id)) => {
            let mut actual_status = status.clone();
            
            // Если платеж все еще в статусе "pending", стоит проверить его состояние
            // напрямую в платежном шлюзе, чтобы получить актуальные данные.
            if status == "pending" {
                let payment_client = PaymentGatewayClient::from_config(&state.config.payment, state.clone());
                
                if let Ok(check_response) = payment_client.check_payment_status(&payment_id).await {
                    if check_response.success {
                        if let Some(gateway_status) = check_response.status {
                            match gateway_status.as_str() {
                                "CONFIRMED" => actual_status = "completed".to_string(),
                                "FAILED" | "CANCELLED" | "EXPIRED" => actual_status = "failed".to_string(),
                                "AUTHORIZED" => {
                                    // Если платеж авторизован, но не подтвержден,
                                    // пытаемся подтвердить его автоматически.
                                    if let (Some(amount), Some(currency), Some(order_id)) =
                                        (check_response.amount, check_response.currency, check_response.order_id) {
                                        if let Ok(confirm_response) = payment_client.confirm_payment(&payment_id, amount, &currency, &order_id).await {
                                            if confirm_response.success {
                                                actual_status = "completed".to_string();
                                                tracing::info!("Auto-confirmed payment {} during status check", payment_id);
                                            }
                                        }
                                    }
                                },
                                _ => {} // В остальных случаях оставляем статус "pending".
                            }
                        }
                    }
                }
            }

            Ok((StatusCode::OK, Json(json!({
                "success": true,
                "booking_id": booking_id,
                "payment_status": actual_status,
                "payment_id": payment_id
            }))))
        },
        None => Err(to_api_error(StatusCode::NOT_FOUND, "Payment for this booking not found"))
    }
}

/// GET /api/payments/success
///
/// Обработчик для страницы, на которую пользователя перенаправляют после успешной оплаты.
/// Возвращает JSON-ответ, удобный для клиентских приложений.
pub async fn payment_success_handler(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>
) -> (StatusCode, Json<serde_json::Value>) {
    let payment_id = query.get("paymentId").cloned();
    let order_id = query.get("orderId").cloned();

    tracing::info!("Payment success callback: payment_id={:?}, order_id={:?}", payment_id, order_id);

    // Дополнительно проверяем статус платежа, чтобы убедиться, что он подтвержден.
    if let Some(ref pid) = payment_id {
        let payment_client = PaymentGatewayClient::from_config(&state.config.payment, state.clone());
        
        if let Ok(check_response) = payment_client.check_payment_status(pid).await {
            if let Some(status) = &check_response.status {
                match status.as_str() {
                    "CONFIRMED" => {
                        tracing::info!("Payment {} confirmed in success callback", pid);
                    },
                    // Если платеж только авторизован, пытаемся его подтвердить.
                    "AUTHORIZED" => {
                        if let (Some(amount), Some(currency), Some(oid)) =
                            (check_response.amount, check_response.currency, check_response.order_id) {
                            if let Ok(confirm_response) = payment_client.confirm_payment(pid, amount, &currency, &oid).await {
                                if confirm_response.success {
                                    tracing::info!("Auto-confirmed payment {} in success callback", pid);
                                }
                            }
                        }
                    },
                    _ => {
                        tracing::warn!("Payment {} has unexpected status {} in success callback", pid, status);
                    }
                }
            }
        }
    }

    (StatusCode::OK, Json(json!({
        "success": true,
        "status": "completed",
        "message": "Payment completed successfully",
        "payment_id": payment_id,
        "order_id": order_id
    })))
}

/// GET /api/payments/circuit-breaker-status
///
/// Предоставляет информацию о текущем состоянии Circuit Breaker для мониторинга
/// доступности платежного шлюза.
pub async fn get_circuit_breaker_status(
    State(state): State<Arc<AppState>>,
) -> ApiResult<impl IntoResponse> {
    let payment_client = PaymentGatewayClient::from_config(&state.config.payment, state.clone());
    let (circuit_state, failure_count) = payment_client.get_circuit_breaker_status();
    
    Ok((StatusCode::OK, Json(json!({
        "success": true,
        "circuit_breaker": {
            "state": format!("{:?}", circuit_state),
            "failure_count": failure_count,
            "threshold": state.config.circuit_breaker.failure_threshold,
            "timeout_seconds": state.config.circuit_breaker.timeout_seconds
        }
    }))))
}

/// GET /api/payments/fail
///
/// Обработчик для страницы, на которую пользователя перенаправляют после неудачной
/// или отмененной оплаты. Возвращает JSON-ответ.
pub async fn payment_fail_handler(
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>
) -> (StatusCode, Json<serde_json::Value>) {
    let payment_id = query.get("paymentId").cloned();
    let order_id = query.get("orderId").cloned();

    tracing::info!("Payment fail callback: payment_id={:?}, order_id={:?}", payment_id, order_id);

    (StatusCode::OK, Json(json!({
        "success": false,
        "status": "failed",
        "message": "Payment failed or was cancelled",
        "payment_id": payment_id,
        "order_id": order_id
    })))
}