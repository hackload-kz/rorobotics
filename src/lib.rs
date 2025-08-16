pub mod config;
pub mod database;
pub mod redis_client;
pub mod models;
pub mod controllers;
pub mod middleware;
pub mod cache;
pub mod services;
pub mod search_client;

// Shared state для всего приложения
#[derive(Clone)]
pub struct AppState {
    pub db: database::Database,
    pub redis: redis_client::RedisClient,
    pub cache: cache::CacheService,
    pub config: config::Config,
    pub search_client: search_client::SearchClient,
}