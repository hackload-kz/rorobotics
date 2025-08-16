use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use chrono::NaiveDateTime;

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Event {
    pub id: i64,
    pub title: String,
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub event_type: String,
    pub datetime_start: NaiveDateTime,
    pub provider: String,
}