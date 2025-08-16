use serde::Serialize;
use sqlx::FromRow;
use chrono::{NaiveDate, NaiveDateTime};

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct User {
    pub user_id: i32,
    pub email: String,
    pub password_hash: String,
    pub password_plain: Option<String>, // For testing only
    pub first_name: String,
    pub surname: String,
    pub birthday: Option<NaiveDate>,
    pub registered_at: NaiveDateTime,
    pub is_active: bool,
    pub last_logged_in: NaiveDateTime,
}

impl User {
    // Найти пользователя по email
    pub async fn find_by_email(email: &str, db: &crate::database::Database) -> Result<Option<User>, sqlx::Error> {
        sqlx::query_as::<_, User>(
            "SELECT * FROM users WHERE email = $1"
        )
        .bind(email)
        .fetch_optional(&db.pool)
        .await
    }
    
    // Проверить пароль (для хакатона используем plain password)
    pub fn verify_password(&self, password: &str) -> bool {
        if let Some(ref plain) = self.password_plain {
            plain == password
        } else {
            // В продакшене здесь был бы bcrypt
            false
        }
    }
}