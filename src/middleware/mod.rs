use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts, StatusCode},
};
use base64::{Engine as _, engine::general_purpose};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: i32,
    pub email: String,
    pub first_name: String,
    pub surname: String,
}

// Структура для результата из БД
#[derive(sqlx::FromRow)]
struct UserRow {
    user_id: i32,
    email: String,
    password_plain: Option<String>,
    first_name: String,
    surname: String,
    is_active: bool,
}

// Basic Auth extractor
impl FromRequestParts<Arc<crate::AppState>> for AuthUser {
    type Rejection = StatusCode;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<crate::AppState>
    ) -> Result<Self, Self::Rejection> {
        // Получаем заголовок Authorization
        let auth_header = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .ok_or(StatusCode::UNAUTHORIZED)?;

        // Проверяем что это Basic auth
        let encoded = auth_header
            .strip_prefix("Basic ")
            .ok_or(StatusCode::UNAUTHORIZED)?;

        // Декодируем base64
        let decoded = general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| StatusCode::UNAUTHORIZED)?;

        let credentials = String::from_utf8(decoded)
            .map_err(|_| StatusCode::UNAUTHORIZED)?;

        // Разделяем email:password
        let mut parts = credentials.splitn(2, ':');
        let email = parts.next().ok_or(StatusCode::UNAUTHORIZED)?;
        let password = parts.next().ok_or(StatusCode::UNAUTHORIZED)?;

        // Проверяем в БД (без макросов)
        let row: Option<UserRow> = sqlx::query_as(
            "SELECT user_id, email, password_plain, first_name, surname, is_active 
             FROM users 
             WHERE email = $1 AND is_active = true"
        )
        .bind(email)
        .fetch_optional(&state.db.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let user = row.ok_or(StatusCode::UNAUTHORIZED)?;

        // Проверяем пароль (для хакатона используем password_plain)
        if user.password_plain != Some(password.to_string()) {
            return Err(StatusCode::UNAUTHORIZED);
        }

        // Обновляем last_logged_in
        sqlx::query("UPDATE users SET last_logged_in = NOW() WHERE user_id = $1")
            .bind(user.user_id)
            .execute(&state.db.pool)
            .await
            .ok(); // Игнорируем ошибку обновления

        Ok(AuthUser {
            user_id: user.user_id,
            email: user.email,
            first_name: user.first_name,
            surname: user.surname,
        })
    }
}