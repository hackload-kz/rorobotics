use axum::{
    extract::State,
    routing::get,
    Json, Router,
};
use std::sync::Arc;

pub fn routes() -> Router<Arc<crate::AppState>> {
    Router::new()
        .route("/test", get(test_handler))
}

async fn test_handler(
    State(_state): State<Arc<crate::AppState>>,
    user: crate::middleware::AuthUser,  // Basic Auth
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "message": "Basic Auth работает!",
        "user": {
            "id": user.user_id,
            "email": user.email,
            "name": format!("{} {}", user.first_name, user.surname)
        }
    }))
}