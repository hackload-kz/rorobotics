use crate::cache::CacheService;
use redis::AsyncCommands;

impl CacheService {
    /// Получает закешированный результат поиска по ключу.
    pub async fn get_cached_search(&self, key: &str) -> Result<Option<String>, redis::RedisError> {
        let mut conn = self.redis.conn.clone();
        conn.get(key).await
    }

    /// Сохраняет результат поиска в кеш с указанным TTL (в секундах).
    pub async fn cache_search_result(
        &self,
        key: &str,
        value: &str,
        ttl_seconds: u64,
    ) -> Result<(), redis::RedisError> {
        let mut conn = self.redis.conn.clone();
        conn.set_ex(key, value, ttl_seconds).await
    }
}