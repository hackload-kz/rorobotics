use redis::{aio::MultiplexedConnection, Client};

#[derive(Clone)]
pub struct RedisClient {
    pub conn: MultiplexedConnection,
}

impl RedisClient {
    pub async fn new(redis_url: &str) -> redis::RedisResult<Self> {
        let client = Client::open(redis_url)?;
        let conn = client.get_multiplexed_tokio_connection().await?;
        Ok(RedisClient { conn })
    }
}