use sqlx::{Row, postgres::PgRow};
use std::sync::Arc;
use tracing::{info, error, warn};
use redis::AsyncCommands;

use crate::AppState;

pub struct CleanupService {
    state: Arc<AppState>,
}

impl CleanupService {
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// Запускает полную очистку: платежи + бронирования
    pub async fn run_full_cleanup(&self) {
        info!("🧹 Starting full cleanup process");
        
        // Сначала очищаем истёкшие платежи
        self.cleanup_expired_payments().await;
        
        // Затем очищаем старые бронирования
        self.cleanup_expired_bookings().await;
        
        // В конце очищаем висящие Redis резервы
        self.cleanup_orphaned_redis_reserves().await;
        
        info!("✅ Full cleanup process completed");
    }

    /// Очистка истёкших платежей (из payment модуля)
    async fn cleanup_expired_payments(&self) {
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

        if expired.is_empty() {
            info!("💳 No expired payments to cleanup");
            return;
        }

        info!("💳 Found {} expired payments to cleanup", expired.len());

        for (payment_id, booking_id, event_id) in expired {
            self.cleanup_expired_payment(payment_id, booking_id, event_id).await;
        }
    }

    /// Очистка отдельного истёкшего платежа
    async fn cleanup_expired_payment(&self, payment_id: String, booking_id: i64, event_id: i64) {
        let mut tx = match self.state.db.pool.begin().await {
            Ok(tx) => tx,
            Err(e) => {
                error!("Failed to start transaction for payment cleanup: {}", e);
                return;
            }
        };

        // Обновляем статус платежа на expired
        let _ = sqlx::query(
            "UPDATE payment_transactions SET status = 'expired', updated_at = NOW() 
             WHERE transaction_id = $1"
        )
        .bind(&payment_id)
        .execute(&mut *tx)
        .await;

        // Освобождаем места
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

        // Удаляем бронирование
        let _ = sqlx::query("DELETE FROM bookings WHERE id = $1")
            .bind(booking_id)
            .execute(&mut *tx)
            .await;

        if tx.commit().await.is_ok() {
            self.clear_redis_reservations(&seats).await;
            self.state.cache.invalidate_seats(event_id).await;
            info!("💳 Expired payment {} cleaned up, {} seats released", payment_id, seats.len());
        } else {
            error!("Failed to commit payment cleanup transaction for {}", payment_id);
        }
    }

    /// Очистка старых бронирований без платежей
    async fn cleanup_expired_bookings(&self) {
        info!("🎫 Starting booking cleanup");

        // 1. Очищаем пустые старые бронирования (без мест)
        self.cleanup_empty_old_bookings().await;

        // 2. Очищаем бронирования с местами, но без платежа
        self.cleanup_bookings_with_seats_no_payment().await;
    }

    /// Очистка пустых старых бронирований (старше 2 часов)
    async fn cleanup_empty_old_bookings(&self) {
        let empty_bookings: Vec<i64> = sqlx::query_scalar(
            r#"
            SELECT b.id 
            FROM bookings b
            LEFT JOIN seats s ON s.booking_id = b.id
            WHERE b.status = 'created'
              AND b.created_at < NOW() - interval '2 hours'
              AND s.id IS NULL
            "#
        )
        .fetch_all(&self.state.db.pool)
        .await
        .unwrap_or_default();

        if empty_bookings.is_empty() {
            info!("🎫 No empty old bookings to cleanup");
            return;
        }

        info!("🎫 Found {} empty old bookings to cleanup", empty_bookings.len());

        for booking_id in empty_bookings {
            let result = sqlx::query(
                "DELETE FROM bookings WHERE id = $1 AND status = 'created'"
            )
            .bind(booking_id)
            .execute(&self.state.db.pool)
            .await;

            match result {
                Ok(affected) if affected.rows_affected() > 0 => {
                    info!("🎫 Deleted empty booking {}", booking_id);
                },
                Ok(_) => {
                    warn!("🎫 Booking {} was not deleted (status changed?)", booking_id);
                },
                Err(e) => {
                    error!("🎫 Failed to delete empty booking {}: {:?}", booking_id, e);
                }
            }
        }
    }

    /// Очистка бронирований с местами, но без платежа (старше 30 минут)
    async fn cleanup_bookings_with_seats_no_payment(&self) {
        let stale_bookings: Vec<(i64, i64)> = sqlx::query_as(
            r#"
            SELECT DISTINCT b.id, b.event_id
            FROM bookings b
            JOIN seats s ON s.booking_id = b.id
            LEFT JOIN payment_transactions pt ON pt.booking_id = b.id
            WHERE b.status = 'created'
              AND b.created_at < NOW() - interval '30 minutes'
              AND s.status = 'RESERVED'
              AND pt.id IS NULL
            "#
        )
        .fetch_all(&self.state.db.pool)
        .await
        .unwrap_or_default();

        if stale_bookings.is_empty() {
            info!("🎫 No stale bookings with seats to cleanup");
            return;
        }

        info!("🎫 Found {} stale bookings with seats to cleanup", stale_bookings.len());

        for (booking_id, event_id) in stale_bookings {
            self.cleanup_stale_booking(booking_id, event_id).await;
        }
    }

    /// Очистка отдельного зависшего бронирования
    async fn cleanup_stale_booking(&self, booking_id: i64, event_id: i64) {
        let mut tx = match self.state.db.pool.begin().await {
            Ok(tx) => tx,
            Err(e) => {
                error!("Failed to start transaction for booking cleanup: {}", e);
                return;
            }
        };

        // Освобождаем места
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

        // Удаляем бронирование
        let booking_result = sqlx::query("DELETE FROM bookings WHERE id = $1")
            .bind(booking_id)
            .execute(&mut *tx)
            .await;

        match booking_result {
            Ok(_) => {
                if tx.commit().await.is_ok() {
                    self.clear_redis_reservations(&seats).await;
                    self.state.cache.invalidate_seats(event_id).await;
                    info!("🎫 Stale booking {} cleaned up, {} seats released", booking_id, seats.len());
                } else {
                    error!("Failed to commit booking cleanup transaction for {}", booking_id);
                }
            },
            Err(e) => {
                error!("Failed to delete stale booking {}: {:?}", booking_id, e);
                let _ = tx.rollback().await;
            }
        }
    }

    /// Очистка висящих резервов в Redis (без соответствующих записей в БД)
    async fn cleanup_orphaned_redis_reserves(&self) {
        let mut redis_conn = self.state.redis.conn.clone();
        
        // Получаем все ключи резервов в Redis
        let redis_keys: Vec<String> = redis::cmd("KEYS")
            .arg("seat:*")
            .query_async(&mut redis_conn)
            .await
            .unwrap_or_default();

        if redis_keys.is_empty() {
            info!("🔑 No Redis reserves to check");
            return;
        }

        info!("🔑 Checking {} Redis reserves for orphaned entries", redis_keys.len());

        let mut orphaned_keys = Vec::new();

        for key in redis_keys {
            // Извлекаем seat_id из ключа (формат: seat:123 или seat:123:reserved)
            if let Some(seat_id_str) = self.extract_seat_id_from_key(&key) {
                if let Ok(seat_id) = seat_id_str.parse::<i64>() {
                    // Проверяем, есть ли это место в БД с соответствующим статусом
                    let seat_exists: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM seats WHERE id = $1 AND status IN ('RESERVED', 'SELECTED'))"
                    )
                    .bind(seat_id)
                    .fetch_one(&self.state.db.pool)
                    .await
                    .unwrap_or(false);

                    if !seat_exists {
                        orphaned_keys.push(key);
                    }
                }
            }
        }

        if orphaned_keys.is_empty() {
            info!("🔑 No orphaned Redis reserves found");
            return;
        }

        info!("🔑 Found {} orphaned Redis reserves to cleanup", orphaned_keys.len());

        // Удаляем осиротевшие ключи
        let _: Result<i64, _> = redis_conn.del(orphaned_keys.clone()).await;
        
        info!("🔑 Cleaned up {} orphaned Redis reserves", orphaned_keys.len());
    }

    /// Извлекает seat_id из Redis ключа
    fn extract_seat_id_from_key(&self, key: &str) -> Option<String> {
        // Поддерживаем форматы: seat:123 и seat:123:reserved
        if let Some(stripped) = key.strip_prefix("seat:") {
            if let Some(colon_pos) = stripped.find(':') {
                // Формат seat:123:reserved
                Some(stripped[..colon_pos].to_string())
            } else {
                // Формат seat:123
                Some(stripped.to_string())
            }
        } else {
            None
        }
    }

    /// Очистка резерваций в Redis
    async fn clear_redis_reservations(&self, seat_ids: &[i64]) {
        if seat_ids.is_empty() {
            return;
        }

        let mut redis = self.state.redis.clone();
        
        let keys: Vec<String> = seat_ids.iter()
            .map(|id| format!("seat:{}", id))
            .collect();

        let _: Result<i64, _> = redis.conn.del(keys).await;
    }

    /// Получает статистику для мониторинга
    pub async fn get_cleanup_stats(&self) -> CleanupStats {
        // Считаем количество записей для очистки
        let expired_payments: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM payment_transactions 
             WHERE status = 'pending' AND created_at < NOW() - interval '15 minutes'"
        )
        .fetch_one(&self.state.db.pool)
        .await
        .unwrap_or(0);

        let empty_bookings: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*) 
            FROM bookings b
            LEFT JOIN seats s ON s.booking_id = b.id
            WHERE b.status = 'created'
              AND b.created_at < NOW() - interval '2 hours'
              AND s.id IS NULL
            "#
        )
        .fetch_one(&self.state.db.pool)
        .await
        .unwrap_or(0);

        let stale_bookings: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(DISTINCT b.id)
            FROM bookings b
            JOIN seats s ON s.booking_id = b.id
            LEFT JOIN payment_transactions pt ON pt.booking_id = b.id
            WHERE b.status = 'created'
              AND b.created_at < NOW() - interval '30 minutes'
              AND s.status = 'RESERVED'
              AND pt.id IS NULL
            "#
        )
        .fetch_one(&self.state.db.pool)
        .await
        .unwrap_or(0);

        let mut redis_conn = self.state.redis.conn.clone();
        let redis_reserves: i64 = redis::cmd("EVAL")
            .arg("return #redis.call('keys', ARGV[1])")
            .arg(0)
            .arg("seat:*")
            .query_async(&mut redis_conn)
            .await
            .unwrap_or(0);

        CleanupStats {
            expired_payments,
            empty_bookings,
            stale_bookings,
            redis_reserves,
        }
    }
}

#[derive(Debug)]
pub struct CleanupStats {
    pub expired_payments: i64,
    pub empty_bookings: i64,
    pub stale_bookings: i64,
    pub redis_reserves: i64,
}

impl CleanupStats {
    pub fn total_items_to_cleanup(&self) -> i64 {
        self.expired_payments + self.empty_bookings + self.stale_bookings
    }
}