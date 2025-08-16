use sqlx::{postgres::PgPoolOptions, Pool, Postgres};
use std::time::Duration;
use tracing::info;

#[derive(Clone)]
pub struct Database {
    pub pool: Pool<Postgres>,
}

impl Database {
    pub async fn new(database_url: &str, pool_size: u32) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(pool_size)
            .acquire_timeout(Duration::from_secs(5))
            .connect(database_url)
            .await?;
        
        Ok(Database { pool })
    }
    
    pub async fn run_migrations(&self) -> Result<(), sqlx::migrate::MigrateError> {
        info!("Running database migrations...");
        sqlx::migrate!("./src/migrations")
            .run(&self.pool)
            .await?;
        info!("Migrations completed");
        Ok(())
    }
}