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

// --- Request/Response структуры ---
#[derive(Debug, Deserialize)]
pub struct InitiatePaymentRequest {
    pub booking_id: i64,
}

#[derive(Serialize)]
pub struct ApiError {
    success: bool,
    message: String,
}

type ApiResult<T> = Result<T, (StatusCode, Json<ApiError>)>;

fn to_api_error(status: StatusCode, message: &str) -> (StatusCode, Json<ApiError>) {
    (status, Json(ApiError { success: false, message: message.to_string() }))
}

// --- HTTP Handlers ---

/// POST /api/bookings/initiatePayment
pub async fn initiate_payment(
    State(state): State<Arc<AppState>>,
    user: AuthUser,
    Json(req): Json<InitiatePaymentRequest>,
) -> ApiResult<impl IntoResponse> {
    if req.booking_id <= 0 {
        return Err(to_api_error(StatusCode::BAD_REQUEST, "ID бронирования должен быть > 0"));
    }

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
        tracing::error!("Ошибка базы данных при получении бронирования: {:?}", e);
        to_api_error(StatusCode::INTERNAL_SERVER_ERROR, "Ошибка базы данных")
    })?;

    let (booking_id, event_title, total_price, seat_count, user_email) = booking_data
        .ok_or_else(|| to_api_error(StatusCode::NOT_FOUND, "Бронирование не найдено или пустое"))?;

    if total_price <= 0.0 {
        return Err(to_api_error(StatusCode::BAD_REQUEST, "Некорректная стоимость бронирования"));
    }

    let payment_client = PaymentGatewayClient::from_config(&state.config.payment, state.clone());
    
    let amount = (total_price * 100.0) as i64;
    let order_id = format!("booking-{}-{}", booking_id, Utc::now().timestamp());
    let description = format!("{} - {} билет(ов)", event_title, seat_count);

    let payment_response = payment_client.create_payment(
        amount,
        order_id.clone(),
        description.clone(),
        Some(user_email),
        state.config.payment.success_url.clone(),
        state.config.payment.fail_url.clone(),
        state.config.payment.webhook_url.clone(),
    ).await.map_err(|e| {
        tracing::error!("Ошибка платежного шлюза: {:?}", e);
        to_api_error(StatusCode::BAD_GATEWAY, "Ошибка платежного шлюза. Повторите попытку позже.")
    })?;

    if !payment_response.success {
        let error_msg = payment_response.message.unwrap_or_else(|| "Неизвестная ошибка".to_string());
        tracing::error!("Платежный шлюз вернул ошибку: {}", error_msg);
        return Err(to_api_error(StatusCode::BAD_GATEWAY, &error_msg));
    }

    let payment_id = payment_response.payment_id
        .ok_or_else(|| to_api_error(StatusCode::INTERNAL_SERVER_ERROR, "Не удалось получить ID платежа от шлюза"))?;

    let mut tx = state.db.pool.begin().await
        .map_err(|e| {
            tracing::error!("Не удалось начать транзакцию БД: {}", e);
            to_api_error(StatusCode::INTERNAL_SERVER_ERROR, "Ошибка БД")
        })?;

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
        tracing::error!("Не удалось сохранить транзакцию: {}", e);
        to_api_error(StatusCode::INTERNAL_SERVER_ERROR, "Не удалось сохранить транзакцию")
    })?;

    sqlx::query("UPDATE bookings SET status = 'pending_payment' WHERE id = $1")
        .bind(booking_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            tracing::error!("Не удалось обновить бронирование: {}", e);
            to_api_error(StatusCode::INTERNAL_SERVER_ERROR, "Не удалось обновить бронирование")
        })?;

    tx.commit().await
        .map_err(|e| {
            tracing::error!("Не удалось завершить транзакцию БД: {}", e);
            to_api_error(StatusCode::INTERNAL_SERVER_ERROR, "Ошибка БД")
        })?;

    tracing::info!("Создан платеж для бронирования {}: payment_id={}, сумма={}", 
        booking_id, payment_id, total_price);

    Ok((StatusCode::OK, Json(json!({
        "success": true,
        "payment_url": payment_response.payment_url,
        "payment_id": payment_id,
        "amount": total_price,
        "currency": "KZT",
        "description": description,
        "expires_at": payment_response.expires_at
    }))))
}

/// POST /api/webhook/payment
pub async fn payment_webhook(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let payment_id = payload["paymentId"].as_str().unwrap_or_default().to_string();
    let status = payload["status"].as_str().unwrap_or_default().to_string();
    
    tracing::info!("Webhook: payment_id={}, status={}", payment_id, status);

    let payment_client = PaymentGatewayClient::from_config(&state.config.payment, state.clone());

    let booking_info: Option<(i64, i64)> = sqlx::query_as(
        "SELECT b.id, b.event_id FROM bookings b 
         JOIN payment_transactions pt ON pt.booking_id = b.id 
         WHERE pt.transaction_id = $1"
    )
    .bind(&payment_id)
    .fetch_optional(&state.db.pool)
    .await.ok().flatten();

    let (booking_id, event_id) = match booking_info {
        Some(info) => info,
        None => {
            tracing::warn!("Платеж {} не найден в БД", payment_id);
            return (StatusCode::OK, Json(json!({"received": true})));
        }
    };

    match status.as_str() {
        "CONFIRMED" | "completed" => {
            payment_client.process_successful_payment(&payment_id, booking_id, event_id).await;
        },
        "CANCELLED" | "FAILED" | "REJECTED" => {
            payment_client.process_failed_payment(&payment_id, booking_id, event_id).await;
        },
        _ => {
            tracing::debug!("Неизвестный статус {} для платежа {}", status, payment_id);
        }
    }

    (StatusCode::OK, Json(json!({"received": true})))
}

/// GET /api/bookings/{booking_id}/payment-status
pub async fn get_payment_status(
    State(state): State<Arc<AppState>>,
    Path(booking_id): Path<i64>,
    user: AuthUser,
) -> ApiResult<impl IntoResponse> {
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
        tracing::error!("Ошибка БД при получении статуса: {}", e);
        to_api_error(StatusCode::INTERNAL_SERVER_ERROR, "Ошибка БД")
    })?;

    match status {
        Some((status, payment_id)) => {
            Ok((StatusCode::OK, Json(json!({
                "success": true,
                "booking_id": booking_id,
                "payment_status": status,
                "payment_id": payment_id
            }))))
        },
        None => Err(to_api_error(StatusCode::NOT_FOUND, "Платеж для данного бронирования не найден"))
    }
}