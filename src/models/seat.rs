use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Seat {
    pub id: i64,
    pub event_id: i64,
    pub row: i32,
    pub number: i32,
    pub status: String,
    pub booking_id: Option<i64>,
    pub category: Option<String>,
    pub price: Option<f64>,
}
