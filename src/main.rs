use axum::{routing::get, Router};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use ticket_system::{
    AppState,
    config::Config,
    database::Database,
    redis_client::RedisClient,
    controllers,
    cache,
    services::payment::PaymentGatewayClient,
    search_client::SearchClient,
};

#[tokio::main(flavor = "multi_thread", worker_threads = 16)]
async fn main() {
    dotenvy::dotenv().ok();
    let config = Config::from_env();
    
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new("info"))
        .with(tracing_subscriber::fmt::layer().compact())
        .init();

    info!("Starting Billetter API - NO DATABASE MODE");

    let app_state = AppState::new(config.clone()).await.expect("Failed to initialize application state");

    let app = Router::new()
        .route("/", get(root_handler))
        .nest("/api", controllers::routes(app_state.clone()))
        .with_state(app_state.clone());

    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "8000".to_string())
        .parse::<u16>()
        .unwrap_or(8000);
    
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("Server listening on http://{}", addr);

    let listener = TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app)
        .await
        .unwrap();
}

// Простейший handler
async fn root_handler() -> &'static str {
    "Billetter API v1.0 - NO DB MODE"
}
