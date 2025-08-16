pub mod test;
pub mod events;
pub mod bookings;

use axum::Router;
use std::sync::Arc;

pub fn routes() -> Router<Arc<crate::AppState>> {
    Router::new()
        .merge(events::routes())
        .merge(bookings::routes())
}