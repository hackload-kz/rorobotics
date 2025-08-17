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

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/events", get(search_events))
}

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    pub query: Option<String>,
    pub date: Option<String>,
    pub page: Option<u32>,
    #[serde(rename = "pageSize")]
    pub page_size: Option<u32>,
}


#[derive(Debug, Serialize)]
pub struct EventResponse {
    pub id: i64,
    pub title: String,
    pub datetime_start: chrono::NaiveDateTime,
}

pub async fn search_events(
    State(state): State<Arc<AppState>>,
    Query(params): Query<EventsQuery>,
) -> Response {
    let query_val = params.query.as_deref().unwrap_or_default();
    let date_val = params.date.as_deref().unwrap_or_default();
    let page = params.page.unwrap_or(1);
    let page_size = params.page_size.unwrap_or(20).clamp(1, 20);

    // 1. Создаем уникальный ключ для кеша на основе параметров запроса
    let cache_key = format!(
        "search:events:q={}&date={}&p={}&ps={}",
        query_val, date_val, page, page_size
    );

    // 2. Пытаемся получить результат из кеша
    if let Ok(Some(cached_json)) = state.cache.get_cached_search(&cache_key).await {
        return Response::builder()
            .header("Content-Type", "application/json")
            .header("X-Cache", "HIT") 
            .body(Body::from(cached_json))
            .unwrap();
    }

    // 3. Cache Miss: если в кеше нет, идем в базу данных
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
    
    // 4. Сериализуем и сохраняем результат в кеш
    if let Ok(json_str) = serde_json::to_string(&response_json) {
        if let Err(e) = state.cache.cache_search_result(&cache_key, &json_str, 3600).await {
            tracing::error!("Failed to cache search result: {:?}", e);
        }
        
        return Response::builder()
            .header("Content-Type", "application/json")
            .header("X-Cache", "MISS")
            .body(Body::from(json_str))
            .unwrap();
    }

    // Fallback в случае ошибки сериализации
    Json(response_json).into_response()
}