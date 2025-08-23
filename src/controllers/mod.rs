//! mod.rs
//!
//! Корневой модуль маршрутизации API.

pub mod analytics;
pub mod bookings;
pub mod events;
pub mod payment;

use axum::{
    Router,
    middleware::from_fn_with_state,
    routing::{get, post, patch},
};
use std::sync::Arc;
use crate::{AppState, middleware::require_auth};

/// Собирает и возвращает главный маршрутизатор приложения.
///
/// # Arguments
/// * `state` - Общее состояние приложения (`Arc<AppState>`), которое будет доступно
///             во всех обработчиках.
pub fn routes(state: Arc<AppState>) -> Router<Arc<crate::AppState>> {
    // --- Защищенные маршруты ---
    // Группа маршрутов, для доступа к которым пользователь должен быть аутентифицирован.
    // Мидлвэр `require_auth` проверяет наличие и валидность токена.
    let protected_routes = Router::new()
        .merge(bookings::routes())
        // Маршруты для инициации и проверки статуса платежа.
        .route("/bookings/initiatePayment", patch(payment::initiate_payment))
        .route("/bookings/{booking_id}/payment-status", get(payment::get_payment_status))
        .layer(from_fn_with_state(state.clone(), require_auth));

    // --- Публичные маршруты ---
    // Группа маршрутов, которые не требуют аутентификации.
    let public_routes = Router::new()
        .merge(events::routes())
        .merge(bookings::reset_route())
        // Вебхук от платежной системы, который не требует аутентификации.
        .route("/webhook/payment", post(payment::payment_webhook))
        // Эндпоинты, на которые платежная система перенаправляет пользователя после оплаты.
        .route("/payments/success", get(payment::payment_success_handler))
        .route("/payments/fail", get(payment::payment_fail_handler))
        // Эндпоинт для мониторинга состояния автоматического выключателя (Circuit Breaker).
        .route("/payments/circuit-breaker-status", get(payment::get_circuit_breaker_status))
        .merge(analytics::routes());

    // Объединяем публичные и защищенные маршруты в один роутер.
    Router::new()
        .merge(public_routes)
        .merge(protected_routes)
}