use axum::{routing::get, Router};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::task;
use tower_http::trace::TraceLayer;
use tracing::{info, error};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use std::time::Duration;

use ticket_system::{
    AppState,
    config::Config,
    database::Database,
    redis_client::RedisClient,
    controllers,
    cache,
    services::payment::PaymentGatewayClient,
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

    // Connect to the database
    let db = Database::new(&config.database.url, config.database.pool_size)
        .await
        .expect("Failed to connect to database");
    info!("Database connected");
    
    // Run migrations
    db.run_migrations()
        .await
        .expect("Failed to run migrations");

    // Connect to Redis
    let redis = RedisClient::new(&config.redis.url)
        .await
        .expect("Failed to connect to Redis");
    info!("Redis connected");

    // Initialize the cache
    let cache = cache::CacheService::new(redis.clone(), db.clone());
    cache.warmup_cache().await;
    info!("Cache warmed up");

    // Create the shared application state
    let app_state = Arc::new(AppState { db: db.clone(), redis: redis.clone(), cache, config: config.clone() });
    
    // --- Start background tasks ---
    
    // Task to clean up expired payments every 5 minutes
    let payment_client = PaymentGatewayClient::from_config(&config.payment, app_state.clone());
    task::spawn(async move {
        loop {
            payment_client.cleanup_expired_payments().await;
            tokio::time::sleep(Duration::from_secs(300)).await;
        }
    });

    // --- Start the web server ---

    // Create the main router
    let app = Router::new()
        .route("/", get(|| async { "Billetter API v1.0" }))
        .route("/health", get(|| async { "OK" }))
        // Mount the routes from the controllers module
        .nest("/api", controllers::routes(app_state.clone()))
        // Pass the application state to the router
        .with_state(app_state.clone())
        .layer(TraceLayer::new_for_http());

    let addr = SocketAddr::from(([0, 0, 0, 0], config.app.port));
    info!("Server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app.into_make_service())
        .await
        .unwrap();
}