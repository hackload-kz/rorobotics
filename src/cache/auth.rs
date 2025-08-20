use crate::cache::CacheService;
use redis::AsyncCommands;
use tracing::info;

impl CacheService {
    /// Сохранить данные авторизованного пользователя в кеш
    pub async fn cache_auth_user(
        &self,
        email: &str,
        password_hash: &str,
        user_data: &str, // JSON сериализованный AuthUser
        ttl_seconds: u64,
    ) -> Result<(), redis::RedisError> {
        let key = format!("auth:{}:{}", email, password_hash);
        let mut conn = self.redis.conn.clone();
        conn.set_ex(key, user_data, ttl_seconds).await
    }
    
    /// Получить данные пользователя из кеша авторизации
    pub async fn get_cached_auth_user(
        &self,
        email: &str,
        password_hash: &str,
    ) -> Result<Option<String>, redis::RedisError> {
        let key = format!("auth:{}:{}", email, password_hash);
        let mut conn = self.redis.conn.clone();
        conn.get(key).await
    }
    
    /// Инвалидировать все сессии пользователя по email
    pub async fn invalidate_user_auth(&self, email: &str) -> Result<(), redis::RedisError> {
        let pattern = format!("auth:{}:*", email);
        let mut conn = self.redis.conn.clone();
        let keys: Vec<String> = redis::cmd("KEYS")
            .arg(&pattern)
            .query_async(&mut conn)
            .await?;
        if !keys.is_empty() {
            let _: () = conn.del(keys).await?;
        }
        Ok(())
    }

    pub async fn should_update_last_login(&self, user_id: i32) -> bool {
        let key = format!("last_login_update:{}", user_id);
        let mut conn = self.redis.conn.clone();
        let result: Result<String, _> = redis::cmd("SET")
            .arg(&key)
            .arg(1)
            .arg("NX")
            .arg("EX")
            .arg(900)
            .query_async(&mut conn)
            .await;
        result.is_ok()
    }
    
    /// Инвалидировать конкретную сессию (для logout)
    pub async fn invalidate_auth_session(
        &self,
        email: &str,
        password_hash: &str,
    ) -> Result<(), redis::RedisError> {
        let key = format!("auth:{}:{}", email, password_hash);
        let mut conn = self.redis.conn.clone();
        let _: () = conn.del(key).await?;
        info!("Invalidated auth session for user {}", email);
        Ok(())
    }
}