use crate::{database::Database, redis_client::RedisClient};
use tracing::info;

pub mod auth;
pub mod events;
pub mod search;
pub mod seats;

#[derive(Clone)]
pub struct CacheService {
    redis: RedisClient,
    db: Database,
}

impl CacheService {
    pub fn new(redis: RedisClient, db: Database) -> Self {
        Self { redis, db }
    }

    // Прогрев кеша при старте
    pub async fn warmup_cache(&self) {
        info!("Starting cache warmup...");
        
        // Загружаем события
        let _ = self.get_events().await;
        
        // Загружаем места для event_id=1
        let _ = self.get_seats(1).await;
        
        info!("Cache warmup done");
    }
}
