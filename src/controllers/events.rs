use axum::{
    extract::{Query, State},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use crate::AppState;
use chrono::NaiveDate;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/events", get(search_events))
}

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    pub query: Option<String>,
    pub date: Option<String>,
    pub page: Option<u32>,
    pub pageSize: Option<u32>,
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
) -> impl IntoResponse {
    let query_val = params.query.unwrap_or_default();
    let from_date = params.date.and_then(|s| {
        NaiveDate::parse_from_str(&s, "%Y-%m-%d")
            .map(|d| d.and_hms_opt(0, 0, 0).unwrap_or_else(|| d.and_hms(0,0,0)))
            .ok()
    });

    // Расчет лимита и смещения
    let page = params.page.unwrap_or(1);
    let page_size = params.pageSize.unwrap_or(20);
    let limit = page_size.max(1).min(20) as i64;
    let offset = ((page.max(1) - 1) * page_size) as i64;

    match state.search_client.search_events(
        &query_val,
        limit,
        offset,
        from_date,
    ).await {
        Ok(results) => {
            let events_response: Vec<EventResponse> = results
                .into_iter()
                .map(|r| EventResponse {
                    id: r.id,
                    title: r.title,
                    datetime_start: r.datetime_start,
                })
                .collect();

            Json(json!({
                "success": true,
                "events": events_response,
                "count": events_response.len()
            }))
        },
        Err(e) => {
            tracing::error!("Failed to search events: {:?}", e);
            Json(json!({
                "success": false,
                "events": [],
                "error": "Failed to retrieve events"
            }))
        }
    }
}