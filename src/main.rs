use axum::{routing::get, Router};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use ticket_system::{
    AppState,
    config::Config,
    controllers,
};

#[tokio::main(flavor = "multi_thread", worker_threads = 32)]
async fn main() {
    dotenvy::dotenv().ok();
    let config = Config::from_env();
    let app_state = AppState::new(config.clone())
        .await
        .expect("Failed to initialize application state");
    let app = Router::new()
        .route("/", get(root_handler))
        .nest("/api", controllers::routes(app_state.clone()))
        .with_state(app_state.clone());
    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "8000".to_string())
        .parse::<u16>()
        .unwrap_or(8000);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app)
        .await
        .unwrap();
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new("info"))
        .with(tracing_subscriber::fmt::layer().compact())
        .init();
    info!("Server listening on http://{}", addr);
}

async fn root_handler() -> &'static str {
    "Billetter API"
}
