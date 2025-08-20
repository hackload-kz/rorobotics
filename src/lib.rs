pub mod config;
pub mod database;
pub mod redis_client;
pub mod models;
pub mod controllers;
pub mod middleware;
pub mod cache;
pub mod services;
pub mod search_client;

use std::sync::Arc;
use tokio::task;

// Shared state для всего приложения
#[derive(Clone)]
pub struct AppState {
    pub db: database::Database,
    pub redis: redis_client::RedisClient,
    pub cache: cache::CacheService,
    pub config: config::Config,
    pub search_client: search_client::SearchClient,
}

impl AppState {
    pub async fn new(config: config::Config) -> Result<Arc<Self>, Box<dyn std::error::Error>> {
        let db = database::Database::new(&config.database.url).await?;
        
        db.run_migrations().await?;
        
        let redis = redis_client::RedisClient::new(&config.redis.url).await?;
        let cache = cache::CacheService::new(redis.clone(), db.clone());
        let search_client = search_client::SearchClient::new(db.pool.clone());
        let state = Arc::new(Self {
            db,
            redis,
            cache,
            config,
            search_client,
        });
        
        let state_for_bg = state.clone();
        task::spawn(async move {
            // Warmup cache в фоне
            state_for_bg.cache.warmup_cache().await;
            
            // Initialize search в фоне
            if let Err(e) = state_for_bg.search_client.initialize().await {
                tracing::error!("Search initialization failed: {:?}", e);
            }
        });
        
        Ok(state)
    }
}