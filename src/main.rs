use axum::{routing::get, Router};
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use ticket_system::{
    AppState,
    config::Config,
    database::Database,
    redis_client::RedisClient,
    controllers,
    cache,
};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    let config = Config::from_env();

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(&config.app.rust_log))
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting Billetter API for Hackathon");

    // Подключаемся к БД
    let db = Database::new(&config.database.url, config.database.pool_size)
        .await
        .expect("Failed to connect to database");
    info!("Database connected");
    
    // Запускаем миграции
    db.run_migrations()
        .await
        .expect("Failed to run migrations");

    // Подключаемся к Redis
    let redis = RedisClient::new(&config.redis.url)
        .await
        .expect("Failed to connect to Redis");
    info!("Redis connected");

    // Добавляем:
    let cache = cache::CacheService::new(redis.clone(), db.clone());
    cache.warmup_cache().await;
    info!("Cache warmed up");

    let app_state = Arc::new(AppState { db, redis, cache });

    // Создаем роутер
    let app = Router::new()
        .route("/", get(|| async { "Billetter API v1.0" }))
        .route("/health", get(|| async { "OK" }))
        .nest("/api", controllers::routes())
        .with_state(app_state)
        .layer(TraceLayer::new_for_http());

    let addr = SocketAddr::from(([0, 0, 0, 0], config.app.port));
    info!("Server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app.into_make_service())
        .await
        .unwrap();
}