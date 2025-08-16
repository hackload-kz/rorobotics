use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use crate::{models::Event, AppState};

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/events", get(get_events))
}

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    query: Option<String>,
    date: Option<String>, // YYYY-MM-DD формат
}

#[derive(Debug, Serialize)]
pub struct EventResponse {
    id: i64,
    title: String,
}

// GET /api/events - Список событий с фильтрацией
async fn get_events(
    State(state): State<Arc<AppState>>,
    Query(params): Query<EventsQuery>,
) -> Json<Vec<EventResponse>> {
    let events = state.cache.get_events().await;
    
    // Фильтруем события
    let filtered_events: Vec<EventResponse> = events
        .into_iter()
        .filter(|event| {
            // Фильтр по текстовому поиску
            if let Some(ref query) = params.query {
                let query_lower = query.to_lowercase();
                if !event.title.to_lowercase().contains(&query_lower) &&
                   !event.description.as_ref().unwrap_or(&String::new()).to_lowercase().contains(&query_lower) {
                    return false;
                }
            }
            
            // Фильтр по дате (YYYY-MM-DD)
            if let Some(ref date) = params.date {
                let event_date = event.datetime_start.format("%Y-%m-%d").to_string();
                if event_date != *date {
                    return false;
                }
            }
            
            true
        })
        .map(|event| EventResponse {
            id: event.id,
            title: event.title,
        })
        .collect();
    
    Json(filtered_events)
}