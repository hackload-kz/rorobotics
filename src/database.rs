use sqlx::{postgres::PgPoolOptions, PgPool};
use std::time::Duration;

#[derive(Clone)]
pub struct Database {
    pub pool: PgPool,
}

impl Database {
    pub async fn new(database_url: &str) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .acquire_timeout(Duration::from_secs(1))
            .idle_timeout(Duration::from_secs(300))
            .connect(database_url)
            .await?;
        
        Ok(Database { pool })
    }
    
    pub async fn run_migrations(&self) -> Result<(), sqlx::migrate::MigrateError> {
        sqlx::migrate!("./src/migrations")
            .run(&self.pool)
            .await?;
        Ok(())
    }
}