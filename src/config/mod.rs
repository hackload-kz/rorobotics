use serde::Deserialize;
use std::env;

// Главная структура конфигурации - контейнер для всех настроек
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub app: AppConfig,
    pub database: DatabaseConfig,
    pub redis: RedisConfig,
    pub kafka: KafkaConfig,
    pub jwt: JwtConfig,
    pub payment: PaymentConfig,
    pub circuit_breaker: CircuitBreakerConfig,
    pub features: FeatureFlags,
}

// Настройки приложения
#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub host: String,
    pub port: u16,
    pub environment: String,
    pub rust_log: String,
}

// Настройки базы данных
#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    pub user: String,
    pub password: String,
    pub db: String,
    pub pool_size: u32,
}

// Настройки Redis
#[derive(Debug, Clone, Deserialize)]
pub struct RedisConfig {
    pub url: String,
    pub pool_size: u32,
}

// Настройки Kafka
#[derive(Debug, Clone, Deserialize)]
pub struct KafkaConfig {
    pub brokers: String,
    pub consumer_group_id: String,
}

// Настройки JWT
#[derive(Debug, Clone, Deserialize)]
pub struct JwtConfig {
    pub secret: String,
    pub expires_in_hours: i64,
}

// Настройки платежного шлюза
#[derive(Debug, Clone, Deserialize)]
pub struct PaymentConfig {
    pub merchant_id: String,
    pub merchant_password: String,
    pub gateway_url: String,
    pub success_url: String,
    pub fail_url: String,
    pub webhook_url: String,
}

// Настройки Circuit Breaker
#[derive(Debug, Clone, Deserialize)]
pub struct CircuitBreakerConfig {
    pub failure_threshold: u32,
    pub timeout_seconds: u64,
}

// Feature flags для включения/выключения функциональности
#[derive(Debug, Clone, Deserialize)]
pub struct FeatureFlags {
    pub enable_auth: bool,
    pub enable_rate_limiting: bool,
    pub enable_analytics: bool,
}

impl Config {
    pub fn from_env() -> Self {
        Config {
            app: AppConfig {
                host: env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
                port: env::var("PORT")
                    .unwrap_or_else(|_| "8000".to_string())
                    .parse()
                    .expect("PORT must be a valid number"),
                environment: env::var("ENVIRONMENT").unwrap_or_else(|_| "development".to_string()),
                rust_log: env::var("RUST_LOG")
                    .unwrap_or_else(|_| "ticket_system=debug,tower_http=debug".to_string()),
            },
            database: DatabaseConfig {
                url: env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
                user: env::var("POSTGRES_USER").expect("POSTGRES_USER must be set"),
                password: env::var("POSTGRES_PASSWORD").expect("POSTGRES_PASSWORD must be set"),
                db: env::var("POSTGRES_DB").expect("POSTGRES_DB must be set"),
                pool_size: env::var("DB_POOL_SIZE")
                    .unwrap_or_else(|_| "20".to_string())
                    .parse()
                    .expect("DB_POOL_SIZE must be a valid number"),
            },
            redis: RedisConfig {
                url: env::var("REDIS_URL").expect("REDIS_URL must be set"),
                pool_size: env::var("REDIS_POOL_SIZE")
                    .unwrap_or_else(|_| "20".to_string())
                    .parse()
                    .expect("REDIS_POOL_SIZE must be a valid number"),
            },
            kafka: KafkaConfig {
                brokers: env::var("KAFKA_BROKERS").expect("KAFKA_BROKERS must be set"),
                consumer_group_id: env::var("KAFKA_CONSUMER_GROUP_ID")
                    .unwrap_or_else(|_| "ticket-system".to_string()),
            },
            jwt: JwtConfig {
                secret: env::var("JWT_SECRET").expect("JWT_SECRET must be set"),
                expires_in_hours: env::var("JWT_EXPIRES_IN_HOURS")
                    .unwrap_or_else(|_| "24".to_string())
                    .parse()
                    .expect("JWT_EXPIRES_IN_HOURS must be a valid number"),
            },
            payment: PaymentConfig {
                merchant_id: env::var("MERCHANT_ID").expect("MERCHANT_ID must be set"),
                merchant_password: env::var("MERCHANT_PASSWORD").expect("MERCHANT_PASSWORD must be set"),
                gateway_url: env::var("PAYMENT_GATEWAY_URL")
                    .unwrap_or_else(|_| "https://gateway.hackload.com/api/v1".to_string()),
                success_url: env::var("PAYMENT_SUCCESS_URL")
                    .unwrap_or_else(|_| "https://your-domain.com/payment/success".to_string()),
                fail_url: env::var("PAYMENT_FAIL_URL")
                    .unwrap_or_else(|_| "https://your-domain.com/payment/fail".to_string()),
                webhook_url: env::var("PAYMENT_WEBHOOK_URL")
                    .unwrap_or_else(|_| "https://your-domain.com/payment/webhook".to_string()),
            },
            circuit_breaker: CircuitBreakerConfig {
                failure_threshold: env::var("CIRCUIT_BREAKER_FAILURE_THRESHOLD")
                    .unwrap_or_else(|_| "5".to_string())
                    .parse()
                    .expect("CIRCUIT_BREAKER_FAILURE_THRESHOLD must be a valid number"),
                timeout_seconds: env::var("CIRCUIT_BREAKER_TIMEOUT_SECONDS")
                    .unwrap_or_else(|_| "60".to_string())
                    .parse()
                    .expect("CIRCUIT_BREAKER_TIMEOUT_SECONDS must be a valid number"),
            },
            features: FeatureFlags {
                enable_auth: env::var("ENABLE_AUTH")
                    .unwrap_or_else(|_| "true".to_string())
                    .parse()
                    .expect("ENABLE_AUTH must be true or false"),
                enable_rate_limiting: env::var("ENABLE_RATE_LIMITING")
                    .unwrap_or_else(|_| "true".to_string())
                    .parse()
                    .expect("ENABLE_RATE_LIMITING must be true or false"),
                enable_analytics: env::var("ENABLE_ANALYTICS")
                    .unwrap_or_else(|_| "true".to_string())
                    .parse()
                    .expect("ENABLE_ANALYTICS must be true or false"),
            },
        }
    }
}