use crate::cache::CacheService;
use crate::models::Event;
use redis::AsyncCommands;
use tracing::info;

impl CacheService {
    // Получить события
    pub async fn get_events(&self) -> Vec<Event> {
        // Сначала пробуем кеш
        if let Ok(events) = self.get_events_from_cache().await {
            return events;
        }
        
        // Если кеш не работает - идем в БД
        if let Ok(events) = self.load_events_from_db().await {
            let _ = self.save_events_to_cache(&events).await;
            return events;
        }
        
        vec![]
    }
    

    async fn load_events_from_db(&self) -> Result<Vec<Event>, sqlx::Error> {
        sqlx::query_as::<_, Event>(
            "SELECT id, title, description, type as event_type, datetime_start, provider 
             FROM events_archive 
             WHERE datetime_start > NOW()
             ORDER BY datetime_start"
        )
        .fetch_all(&self.db.pool)
        .await
    }

    // === Работа с кешем ===
    async fn get_events_from_cache(&self) -> Result<Vec<Event>, redis::RedisError> {
        let mut conn = self.redis.conn.clone();
        let data: String = conn.get("events").await?;
        let events: Vec<Event> = serde_json::from_str(&data).map_err(|_| {
            redis::RedisError::from((redis::ErrorKind::TypeError, "Parse error"))
        })?;
        Ok(events)
    }

    async fn save_events_to_cache(&self, events: &[Event]) -> Result<(), redis::RedisError> {
        let data = serde_json::to_string(events).map_err(|_| {
            redis::RedisError::from((redis::ErrorKind::TypeError, "Serialize error"))
        })?;
        let mut conn = self.redis.conn.clone();
        conn.set_ex("events", data, 3600).await
    }
}