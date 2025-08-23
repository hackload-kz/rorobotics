pub mod test;
pub mod events;
pub mod bookings;
pub mod payment;

use axum::{
    Router,
    middleware::from_fn_with_state,
};
use std::sync::Arc;
use crate::{AppState, middleware::require_auth};

pub fn routes(state: Arc<AppState>) -> Router<Arc<crate::AppState>> {
    let protected_routes = Router::new()
        .merge(bookings::routes())
        .layer(from_fn_with_state(state.clone(), require_auth));

    let public_routes = Router::new()
        .merge(events::routes())
        .merge(bookings::reset_route());

    Router::new()
        .merge(public_routes)
        .merge(protected_routes)
}