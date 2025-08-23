//! analytics.rs
//!
//! Модуль для получения аналитики и статистики по событиям.
//!
//! Включает в себя следующую функциональность:
//! - Получение детальной аналитики продаж для конкретного события.
//! - Подсчет статистики по местам (проданные, зарезервированные, свободные).
//! - Расчет общей выручки и количества завершенных бронирований.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use sqlx::Row;
use crate::AppState;

/// Определяет маршруты, связанные с аналитикой.
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/analytics", get(get_event_analytics))
}

// --- Вспомогательные функции ---

/// Проверяет, существует ли событие с указанным ID в базе данных.
async fn event_exists(pool: &sqlx::PgPool, event_id: i64) -> sqlx::Result<bool> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM events_archive WHERE id = $1)"
    )
    .bind(event_id)
    .fetch_one(pool)
    .await
}

// --- Управление аналитикой ---

/// GET /api/analytics
///
/// Возвращает детальную аналитику продаж для указанного события,
/// включая статистику по местам, выручку и количество бронирований.
#[derive(Debug, Deserialize)]
struct AnalyticsQuery {
    pub id: i64,
}

#[derive(Debug, Serialize)]
struct AnalyticsResponse {
    pub event_id: i64,
    pub total_seats: i32,
    pub sold_seats: i32,
    pub reserved_seats: i32,
    pub free_seats: i32,
    pub total_revenue: String,
    pub bookings_count: i32,
}

async fn get_event_analytics(
    State(state): State<Arc<AppState>>,
    Query(params): Query<AnalyticsQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if params.id <= 0 {
        return Err((StatusCode::BAD_REQUEST, "ID события должен быть > 0".to_string()));
    }

    // Проверяем, что событие существует.
    let exists = event_exists(&state.db.pool, params.id)
        .await
        .map_err(|e| {
            tracing::error!("get_event_analytics: ошибка проверки события {}: {:?}", params.id, e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Ошибка проверки события".to_string())
        })?;

    if !exists {
        return Err((StatusCode::NOT_FOUND, "Событие не найдено".to_string()));
    }

    // Получаем детальную статистику по местам и бронированиям для события.
    // Используем оконные функции и фильтры для эффективного подсчета.
    let analytics_row = sqlx::query(
        r#"
        SELECT 
            $1 as event_id,
            COUNT(s.id)::int as total_seats,
            COUNT(s.id) FILTER (WHERE s.status = 'SOLD')::int as sold_seats,
            COUNT(s.id) FILTER (WHERE s.status = 'RESERVED')::int as reserved_seats,
            COUNT(s.id) FILTER (WHERE s.status IN ('FREE', 'AVAILABLE'))::int as free_seats,
            COALESCE(SUM(s.price) FILTER (WHERE s.status = 'SOLD'), 0)::float8 as total_revenue,
            COUNT(DISTINCT b.id) FILTER (WHERE b.status = 'paid')::int as bookings_count
        FROM seats s
        LEFT JOIN bookings b ON b.id = s.booking_id AND b.status = 'paid'
        WHERE s.event_id = $1
        "#
    )
    .bind(params.id)
    .fetch_optional(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("get_event_analytics: sql ошибка для события {}: {:?}", params.id, e);
        (StatusCode::INTERNAL_SERVER_ERROR, "Не удалось получить аналитику".to_string())
    })?;

    // Если для события нет мест, возвращаем нулевую статистику.
    let row = match analytics_row {
        Some(row) => row,
        None => {
            let empty_response = AnalyticsResponse {
                event_id: params.id,
                total_seats: 0,
                sold_seats: 0,
                reserved_seats: 0,
                free_seats: 0,
                total_revenue: "0.00".to_string(),
                bookings_count: 0,
            };
            return Ok((StatusCode::OK, Json(empty_response)));
        }
    };

    // Извлекаем данные из результата запроса.
    let event_id: i64 = row.get("event_id");
    let total_seats: i32 = row.get("total_seats");
    let sold_seats: i32 = row.get("sold_seats");
    let reserved_seats: i32 = row.get("reserved_seats");
    let free_seats: i32 = row.get("free_seats");
    let total_revenue: f64 = row.get("total_revenue");
    let bookings_count: i32 = row.get("bookings_count");

    // Формируем финальный ответ с корректным форматированием выручки.
    let response = AnalyticsResponse {
        event_id,
        total_seats,
        sold_seats,
        reserved_seats,
        free_seats,
        total_revenue: format!("{:.2}", total_revenue),
        bookings_count,
    };

    tracing::info!(
        "Аналитика для события {}: {} мест, {} продано, выручка {}",
        event_id, total_seats, sold_seats, response.total_revenue
    );

    Ok((StatusCode::OK, Json(response)))
}