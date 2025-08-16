use axum::{
    extract::{FromRequestParts, State},
    http::{header, request::Parts, Request, StatusCode},
    middleware::Next,
    response::Response,
    Extension,
};
use base64::{Engine as _, engine::general_purpose};
use std::sync::Arc;
use sqlx::FromRow;
use tracing::error;
use crate::AppState;

/// Структура для представления аутентифицированного пользователя
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: i32,
    pub email: String,
    pub first_name: String,
    pub surname: String,
}

/// Структура для получения данных пользователя из БД
#[derive(FromRow)]
struct UserRow {
    user_id: i32,
    email: String,
    password_plain: Option<String>,
    first_name: String,
    surname: String,
    is_active: bool,
}

impl FromRequestParts<Arc<AppState>> for AuthUser {
    type Rejection = StatusCode;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let auth_header = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .ok_or(StatusCode::UNAUTHORIZED)?;
        let encoded = auth_header
            .strip_prefix("Basic ")
            .ok_or(StatusCode::UNAUTHORIZED)?;
        let decoded = general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| StatusCode::UNAUTHORIZED)?;
        let credentials = String::from_utf8(decoded)
            .map_err(|_| StatusCode::UNAUTHORIZED)?;
        let mut parts = credentials.splitn(2, ':');
        let email = parts.next().ok_or(StatusCode::UNAUTHORIZED)?;
        let password = parts.next().ok_or(StatusCode::UNAUTHORIZED)?;
        let row: Option<UserRow> = sqlx::query_as(
            "SELECT user_id, email, password_plain, first_name, surname, is_active
             FROM users 
             WHERE email = $1 AND is_active = true"
        )
        .bind(email)
        .fetch_optional(&state.db.pool)
        .await
        .map_err(|e| {
            error!("Database error during auth: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        let user = row.ok_or(StatusCode::UNAUTHORIZED)?;

        if user.password_plain != Some(password.to_string()) {
            return Err(StatusCode::UNAUTHORIZED);
        }

        sqlx::query("UPDATE users SET last_logged_in = NOW() WHERE user_id = $1")
            .bind(user.user_id)
            .execute(&state.db.pool)
            .await
            .ok();

        Ok(AuthUser {
            user_id: user.user_id,
            email: user.email,
            first_name: user.first_name,
            surname: user.surname,
        })
    }
}

pub async fn require_auth(
    State(state): State<Arc<AppState>>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let (mut parts, body) = request.into_parts();
    let auth_user = AuthUser::from_request_parts(&mut parts, &state).await?;
    let mut request = Request::from_parts(parts, body);
    request.extensions_mut().insert(auth_user);
    
    Ok(next.run(request).await)
}

pub async fn get_auth_user_from_extensions(Extension(user): Extension<AuthUser>) -> AuthUser {
    user
}