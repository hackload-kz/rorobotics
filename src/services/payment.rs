use sha2::{Sha256, Digest};
use serde::{Deserialize, Serialize};
use sqlx::{Row, postgres::PgRow};
use std::sync::Arc;
use tracing::{info, error};
use redis::AsyncCommands; 

use crate::{
    AppState,
    redis_client::RedisClient,
    config::PaymentConfig,
};

#[derive(Debug, Serialize)]
struct PaymentInitRequest {
    #[serde(rename = "teamSlug")]
    team_slug: String,
    token: String,
    amount: i64,
    #[serde(rename = "orderId")]
    order_id: String,
    currency: String,
    description: String,
    #[serde(rename = "successURL")]
    success_url: String,
    #[serde(rename = "failURL")]
    fail_url: String,
    #[serde(rename = "notificationURL")]
    notification_url: String,
    email: Option<String>,
    language: String,
}

#[derive(Debug, Deserialize)]
pub struct PaymentInitResponse {
    pub success: bool,
    #[serde(rename = "paymentId")]
    pub payment_id: Option<String>,
    #[serde(rename = "paymentURL")]
    pub payment_url: Option<String>,
    #[serde(rename = "expiresAt")]
    pub expires_at: Option<String>,
    pub code: Option<i32>,
    pub message: Option<String>,
}

#[derive(Clone)]
pub struct PaymentGatewayClient {
    state: Arc<AppState>,
    team_slug: String,
    password: String,
    base_url: String,
    http_client: reqwest::Client,
}

impl PaymentGatewayClient {
    pub fn from_config(config: &PaymentConfig, state: Arc<AppState>) -> Self {
        Self {
            state,
            team_slug: config.merchant_id.clone(),
            password: config.merchant_password.clone(),
            base_url: config.gateway_url.clone(),
            http_client: reqwest::Client::new(),
        }
    }

    fn generate_init_token(&self, amount: i64, currency: &str, order_id: &str) -> String {
        let token_string = format!(
            "{}{}{}{}{}",
            amount, currency, order_id, self.password, self.team_slug
        );
        let mut hasher = Sha256::new();
        hasher.update(token_string.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Создаёт платёж в платёжной системе
    pub async fn create_payment(
        &self,
        amount: i64,
        order_id: String,
        description: String,
        email: Option<String>,
        success_url: String,
        fail_url: String,
        webhook_url: String,
    ) -> Result<PaymentInitResponse, reqwest::Error> {
        let currency = "KZT";
        let token = self.generate_init_token(amount, currency, &order_id);

        let request = PaymentInitRequest {
            team_slug: self.team_slug.clone(),
            token,
            amount,
            order_id,
            currency: currency.to_string(),
            description,
            success_url,
            fail_url,
            notification_url: webhook_url,
            email,
            language: "ru".to_string(),
        };

        let response = self
            .http_client
            .post(&format!("{}/PaymentInit/init", self.base_url))
            .json(&request)
            .send()
            .await?;

        response.json().await
    }

    /// Очистка зарезервированных мест в Redis
    pub async fn clear_redis_reservations(&self, seat_ids: &[i64]) {
        if seat_ids.is_empty() {
            return;
        }

        let mut redis = self.state.redis.clone();
        
        let keys: Vec<String> = seat_ids.iter()
            .map(|id| format!("seat:{}", id))
            .collect();

        // Явно указываем тип возвращаемого значения i64.
        let _ = redis.conn.del::<_, i64>(keys).await;
    }

    /// Освобождение мест и бронирований по истёкшим платежам
    pub async fn cleanup_expired_payments(&self) {
        let expired: Vec<(String, i64, i64)> = sqlx::query_as(
            r#"
            SELECT pt.transaction_id, b.id, b.event_id
            FROM payment_transactions pt
            JOIN bookings b ON b.id = pt.booking_id
            WHERE pt.status = 'pending' 
              AND pt.created_at < NOW() - interval '15 minutes'
            "#
        )
        .fetch_all(&self.state.db.pool)
        .await
        .unwrap_or_default();

        for (payment_id, booking_id, event_id) in expired {
            let mut tx = match self.state.db.pool.begin().await {
                Ok(tx) => tx,
                Err(e) => {
                    error!("Failed to start transaction for cleanup: {}", e);
                    continue;
                },
            };

            let _ = sqlx::query(
                "UPDATE payment_transactions SET status = 'expired', updated_at = NOW() 
                 WHERE transaction_id = $1"
            )
            .bind(&payment_id)
            .execute(&mut *tx)
            .await;

            let seats: Vec<i64> = sqlx::query(
                "UPDATE seats 
                 SET status = 'AVAILABLE', booking_id = NULL, updated_at = NOW() 
                 WHERE booking_id = $1 AND status = 'RESERVED' 
                 RETURNING id"
            )
            .bind(booking_id)
            .fetch_all(&mut *tx)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|row: PgRow| row.get("id"))
            .collect();
            
            let _ = sqlx::query("DELETE FROM bookings WHERE id = $1")
                .bind(booking_id)
                .execute(&mut *tx)
                .await;

            if tx.commit().await.is_ok() {
                self.clear_redis_reservations(&seats).await;
                self.state.cache.invalidate_seats(event_id).await;
                info!(
                    "Expired payment {} cleaned up, {} seats released",
                    payment_id,
                    seats.len()
                );
            }
        }
    }

    pub async fn process_successful_payment(
        &self,
        payment_id: &str,
        booking_id: i64,
        event_id: i64,
    ) {
        let mut tx = match self.state.db.pool.begin().await {
            Ok(tx) => tx,
            Err(e) => {
                error!("Failed to start transaction for successful payment: {}", e);
                return;
            }
        };

        sqlx::query("UPDATE payment_transactions SET status = 'completed', updated_at = NOW() WHERE transaction_id = $1")
            .bind(payment_id)
            .execute(&mut *tx).await.ok();

        sqlx::query("UPDATE bookings SET status = 'paid', updated_at = NOW() WHERE id = $1")
            .bind(booking_id)
            .execute(&mut *tx).await.ok();

        let seats: Vec<i64> = sqlx::query("UPDATE seats SET status = 'SOLD', updated_at = NOW() WHERE booking_id = $1 AND status = 'RESERVED' RETURNING id")
            .bind(booking_id)
            .fetch_all(&mut *tx).await.unwrap_or_default()
            .into_iter()
            .map(|row: PgRow| row.get("id"))
            .collect();

        if tx.commit().await.is_ok() {
            self.clear_redis_reservations(&seats).await;
            self.state.cache.invalidate_seats(event_id).await;
            info!("Payment {} completed, {} seats sold", payment_id, seats.len());
        }
    }

    /// Обрабатывает неуспешный платёж: освобождает места, обновляет статусы.
    pub async fn process_failed_payment(
        &self,
        payment_id: &str,
        booking_id: i64,
        event_id: i64,
    ) {
        let mut tx = match self.state.db.pool.begin().await {
            Ok(tx) => tx,
            Err(e) => {
                error!("Failed to start transaction for failed payment: {}", e);
                return;
            }
        };

        sqlx::query(
            "UPDATE payment_transactions 
             SET status = 'failed', updated_at = NOW() 
             WHERE transaction_id = $1"
        )
        .bind(payment_id)
        .execute(&mut *tx)
        .await
        .ok();

        let seats: Vec<i64> = sqlx::query(
            "UPDATE seats 
             SET status = 'AVAILABLE', updated_at = NOW(), booking_id = NULL 
             WHERE booking_id = $1 AND status = 'RESERVED' 
             RETURNING id"
        )
        .bind(booking_id)
        .fetch_all(&mut *tx)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|row: PgRow| row.get("id"))
        .collect();

        sqlx::query(
            "DELETE FROM bookings WHERE id = $1"
        )
        .bind(booking_id)
        .execute(&mut *tx)
        .await
        .ok();

        if tx.commit().await.is_ok() {
            self.clear_redis_reservations(&seats).await;
            self.state.cache.invalidate_seats(event_id).await;
            info!(
                "Payment {} failed, {} seats released",
                payment_id,
                seats.len()
            );
        }
    }
}