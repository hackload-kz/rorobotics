//! events.rs
//!
//! Модуль для поиска и получения информации о событиях.
//!
//! Основная функциональность - это поиск событий с использованием кэширования
//! в Redis для снижения нагрузки на базу данных и ускорения ответов.

use axum::{
    body::Body,
    extract::{Query, State},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use crate::AppState;

/// Определяет маршруты, связанные с событиями.
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/events", get(search_events))
}

/// Параметры запроса для поиска событий.
#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    pub query: Option<String>,
    pub date: Option<String>,
    pub page: Option<u32>,
    #[serde(rename = "pageSize")]
    pub page_size: Option<u32>,
}

/// Структура ответа для одного события.
#[derive(Debug, Serialize)]
pub struct EventResponse {
    pub id: i64,
    pub title: String,
    pub datetime_start: chrono::NaiveDateTime,
}

/// GET /api/events
///
/// Ищет события по заданным параметрам (текстовый запрос, дата).
/// Результаты поиска кэшируются в Redis для ускорения повторных запросов
/// с теми же параметрами.
pub async fn search_events(
    State(state): State<Arc<AppState>>,
    Query(params): Query<EventsQuery>,
) -> Response {
    let query_val = params.query.as_deref().unwrap_or_default();
    let date_val = params.date.as_deref().unwrap_or_default();
    let page = params.page.unwrap_or(1);
    let page_size = params.page_size.unwrap_or(20).clamp(1, 20);

    // Шаг 1: Формируем уникальный ключ для кэша на основе всех параметров запроса.
    // Это гарантирует, что каждый уникальный поисковый запрос будет кэширован отдельно.
    let cache_key = format!(
        "search:events:q={}&date={}&p={}&ps={}",
        query_val, date_val, page, page_size
    );

    // Шаг 2: Пытаемся получить результат из кэша Redis.
    if let Ok(Some(cached_json)) = state.cache.get_cached_search(&cache_key).await {
        // Cache HIT: Данные найдены в кэше.
        // Отправляем их клиенту с заголовком X-Cache: HIT для отладки.
        return Response::builder()
            .header("Content-Type", "application/json")
            .header("X-Cache", "HIT")
            .body(Body::from(cached_json))
            .unwrap();
    }

    // Шаг 3: Cache MISS. Если в кэше данных нет, выполняем запрос к поисковому сервису (например, ElasticSearch или БД).
    let from_date = params.date.and_then(|s| {
        NaiveDate::parse_from_str(&s, "%Y-%m-%d")
            .map(|d| d.and_hms_opt(0, 0, 0).unwrap())
            .ok()
    });

    let limit: i64 = page_size as i64;
    let offset: i64 = ((page.max(1) - 1) * page_size) as i64;

    let search_result = state.search_client.search_events(
        query_val,
        limit,
        offset,
        from_date,
    ).await;
    
    // Формируем JSON-ответ на основе результатов поиска.
    let response_json = match search_result {
        Ok(results) => {
            let events_response: Vec<EventResponse> = results
                .into_iter()
                .map(|r| EventResponse {
                    id: r.id,
                    title: r.title,
                    datetime_start: r.datetime_start,
                })
                .collect();

            json!({
                "success": true,
                "events": events_response,
                "count": events_response.len()
            })
        },
        Err(e) => {
            tracing::error!("Failed to search events: {:?}", e);
            return Json(json!({
                "success": false,
                "error": "Failed to retrieve events"
            })).into_response();
        }
    };
    
    // Шаг 4: Сохраняем полученный результат в кэш Redis на 1 час (3600 секунд).
    // Следующий запрос с такими же параметрами будет обслужен из кэша.
    if let Ok(json_str) = serde_json::to_string(&response_json) {
        if let Err(e) = state.cache.cache_search_result(&cache_key, &json_str, 3600).await {
            tracing::error!("Failed to cache search result: {:?}", e);
        }
        
        // Отправляем ответ клиенту с заголовком X-Cache: MISS, указывая,
        // что ответ был сгенерирован, а не взят из кэша.
        return Response::builder()
            .header("Content-Type", "application/json")
            .header("X-Cache", "MISS")
            .body(Body::from(json_str))
            .unwrap();
    }

    // Резервный вариант на случай, если сериализация в JSON не удалась.
    Json(response_json).into_response()
}