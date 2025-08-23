//! payment.rs
//!
//! Этот модуль реализует сервисный слой для взаимодействия с внешним платёжным шлюзом.
//!
//! Ключевые компоненты:
//! 1.  **CircuitBreaker**: Реализация паттерна "Автоматический выключатель" для обеспечения
//!     отказоустойчивости при работе с внешним API. Он предотвращает постоянные запросы
//!     к неработающему сервису.
//! 2.  **PaymentGatewayClient**: Основной клиент, который инкапсулирует всю логику
//!     отправки запросов к платёжному шлюзу, генерации токенов и обработки ответов.
//!     Все сетевые вызовы защищены с помощью `CircuitBreaker`.
//! 3.  **Обработчики жизненного цикла платежа**: Функции для обработки успешных,
//!     неудавшихся и просроченных платежей, а также для обработки вебхуков.

use sha2::{Sha256, Digest};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{info, error, warn};
use redis::AsyncCommands;
use tokio::time::{Duration, Instant};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::{
    AppState,
    config::PaymentConfig,
};

/// Состояния "Автоматического выключателя" (Circuit Breaker).
#[derive(Debug, Clone, PartialEq)]
pub enum CircuitState {
    /// **Closed (Замкнуто)**: Нормальный режим работы. Запросы к сервису разрешены.
    Closed,
    /// **Open (Разомкнуто)**: Режим блокировки. Запросы к сервису временно запрещены
    /// после обнаружения множественных сбоев.
    Open,
    /// **HalfOpen (Полуоткрыто)**: Тестовый режим. После таймаута в состоянии Open,
    /// разрешается один пробный запрос для проверки, восстановился ли сервис.
    HalfOpen,
}

/// Реализация паттерна "Автоматический выключатель" для контроля доступа к внешнему сервису.
#[derive(Debug)]
pub struct CircuitBreaker {
    /// Текущее состояние (Closed, Open, HalfOpen).
    state: std::sync::RwLock<CircuitState>,
    /// Счетчик последовательных сбоев.
    failure_count: AtomicU32,
    /// Время последнего сбоя для расчета таймаута.
    last_failure_time: AtomicU64,
    /// Порог сбоев, после которого выключатель переходит в состояние Open.
    failure_threshold: u32,
    /// Длительность таймаута в состоянии Open, после которого происходит переход в HalfOpen.
    timeout_duration: Duration,
}

impl CircuitBreaker {
    /// Создает новый экземпляр CircuitBreaker.
    pub fn new(failure_threshold: u32, timeout_seconds: u64) -> Self {
        Self {
            state: std::sync::RwLock::new(CircuitState::Closed),
            failure_count: AtomicU32::new(0),
            last_failure_time: AtomicU64::new(0),
            failure_threshold,
            timeout_duration: Duration::from_secs(timeout_seconds),
        }
    }

    /// Проверяет, можно ли выполнить следующий запрос к сервису.
    pub fn can_execute(&self) -> bool {
        let state = self.state.read().unwrap();
        
        match *state {
            // Если "замкнуто", запросы всегда разрешены.
            CircuitState::Closed => true,
            // Если "разомкнуто", проверяем, прошел ли таймаут.
            CircuitState::Open => {
                let now = Instant::now().elapsed().as_secs();
                let last_failure = self.last_failure_time.load(Ordering::Relaxed);
                
                // Если с момента последнего сбоя прошло достаточно времени...
                if now - last_failure >= self.timeout_duration.as_secs() {
                    // ...переходим в "полуоткрытое" состояние для тестового запроса.
                    drop(state); // Освобождаем блокировку чтения перед записью.
                    *self.state.write().unwrap() = CircuitState::HalfOpen;
                    info!("Circuit breaker transitioning to HalfOpen state");
                    true // Разрешаем тестовый запрос.
                } else {
                    false // Таймаут еще не истек, запрос блокируется.
                }
            },
            // В "полуоткрытом" состоянии разрешаем один пробный запрос.
            CircuitState::HalfOpen => true,
        }
    }

    /// Регистрирует успешное выполнение запроса.
    pub fn record_success(&self) {
        let mut state = self.state.write().unwrap();
        
        match *state {
            // Если тестовый запрос в HalfOpen прошел успешно, "замыкаем" цепь.
            CircuitState::HalfOpen => {
                *state = CircuitState::Closed;
                self.failure_count.store(0, Ordering::Relaxed);
                info!("Circuit breaker recovered - transitioning to Closed state");
            },
            // В обычном режиме просто сбрасываем счетчик ошибок.
            CircuitState::Closed => {
                self.failure_count.store(0, Ordering::Relaxed);
            },
            _ => {} // В состоянии Open ничего не делаем.
        }
    }

    /// Регистрирует неудачное выполнение запроса.
    pub fn record_failure(&self) {
        let failure_count = self.failure_count.fetch_add(1, Ordering::Relaxed) + 1;
        self.last_failure_time.store(
            Instant::now().elapsed().as_secs(),
            Ordering::Relaxed
        );

        let mut state = self.state.write().unwrap();
        
        match *state {
            // Если в "замкнутом" состоянии достигнут порог ошибок, "размыкаем" цепь.
            CircuitState::Closed => {
                if failure_count >= self.failure_threshold {
                    *state = CircuitState::Open;
                    error!("Circuit breaker OPENED - {} failures reached threshold {}",
                          failure_count, self.failure_threshold);
                }
            },
            // Если тестовый запрос в HalfOpen провалился, возвращаемся в Open.
            CircuitState::HalfOpen => {
                *state = CircuitState::Open;
                warn!("Circuit breaker test failed - returning to Open state");
            },
            _ => {}
        }
    }

    /// Возвращает текущее состояние выключателя для мониторинга.
    pub fn get_state(&self) -> CircuitState {
        self.state.read().unwrap().clone()
    }
}

/// Ошибки, которые могут возникнуть при работе через Circuit Breaker.
#[derive(Debug)]
pub enum CircuitBreakerError {
    /// Ошибка, означающая, что Circuit Breaker находится в состоянии Open и блокирует запрос.
    Open,
    /// Ошибка, полученная от HTTP-клиента при попытке выполнить запрос.
    PaymentGatewayError(reqwest::Error),
}

impl std::fmt::Display for CircuitBreakerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CircuitBreakerError::Open => write!(f, "Circuit breaker is open - payment gateway temporarily unavailable"),
            CircuitBreakerError::PaymentGatewayError(e) => write!(f, "Payment gateway error: {}", e),
        }
    }
}

impl std::error::Error for CircuitBreakerError {}

// --- Модели данных для API платёжного шлюза ---

/// Запрос на инициацию платежа.
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

/// Ответ от API на инициацию платежа.
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

/// Запрос на проверку статуса платежа.
#[derive(Debug, Serialize)]
struct PaymentCheckRequest {
    #[serde(rename = "teamSlug")]
    team_slug: String,
    token: String,
    #[serde(rename = "paymentId")]
    payment_id: String,
}

/// Ответ от API на проверку статуса платежа.
#[derive(Debug, Deserialize)]
pub struct PaymentCheckResponse {
    pub success: bool,
    pub status: Option<String>,
    #[serde(rename = "paymentId")]
    pub payment_id: Option<String>,
    pub amount: Option<i64>,
    pub currency: Option<String>,
    #[serde(rename = "orderId")]
    pub order_id: Option<String>,
    pub code: Option<i32>,
    pub message: Option<String>,
}

/// Запрос на подтверждение (списание) платежа.
#[derive(Debug, Serialize)]
struct PaymentConfirmRequest {
    #[serde(rename = "teamSlug")]
    team_slug: String,
    token: String,
    #[serde(rename = "paymentId")]
    payment_id: String,
    amount: i64,
    currency: String,
    #[serde(rename = "orderId")]
    order_id: String,
}

/// Ответ от API на подтверждение платежа.
#[derive(Debug, Deserialize)]
pub struct PaymentConfirmResponse {
    pub success: bool,
    pub code: Option<i32>,
    pub message: Option<String>,
}

/// Клиент для взаимодействия с API платёжного шлюза.
#[derive(Clone)]
pub struct PaymentGatewayClient {
    /// Доступ к общему состоянию приложения (БД, Redis, конфиг).
    state: Arc<AppState>,
    /// Идентификатор продавца.
    team_slug: String,
    /// Секретный пароль для генерации токенов.
    password: String,
    /// Базовый URL платёжного шлюза.
    base_url: String,
    /// Асинхронный HTTP-клиент.
    http_client: reqwest::Client,
    /// Экземпляр Circuit Breaker для этого клиента.
    circuit_breaker: Arc<CircuitBreaker>,
}

impl PaymentGatewayClient {
    /// Создает и конфигурирует клиент на основе настроек приложения.
    pub fn from_config(config: &PaymentConfig, state: Arc<AppState>) -> Self {
        let circuit_breaker = Arc::new(CircuitBreaker::new(
            state.config.circuit_breaker.failure_threshold,
            state.config.circuit_breaker.timeout_seconds,
        ));

        Self {
            state,
            team_slug: config.merchant_id.clone(),
            password: config.merchant_password.clone(),
            base_url: config.gateway_url.clone(),
            http_client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30)) // Таймаут для HTTP-запросов.
                .build()
                .expect("Failed to create HTTP client"),
            circuit_breaker,
        }
    }

    /// Выполняет асинхронную операцию, пропуская её через Circuit Breaker.
    async fn execute_with_circuit_breaker<F, T>(&self, operation: F) -> Result<T, CircuitBreakerError>
    where
        F: std::future::Future<Output = Result<T, reqwest::Error>>,
    {
        // Перед выполнением запроса проверяем состояние выключателя.
        if !self.circuit_breaker.can_execute() {
            warn!("Circuit breaker is OPEN - blocking payment gateway request");
            return Err(CircuitBreakerError::Open);
        }

        match operation.await {
            // Если операция успешна, сообщаем об этом выключателю.
            Ok(result) => {
                self.circuit_breaker.record_success();
                Ok(result)
            },
            // В случае ошибки, также сообщаем об этом для обновления счетчика сбоев.
            Err(e) => {
                error!("Payment gateway request failed: {:?}", e);
                self.circuit_breaker.record_failure();
                Err(CircuitBreakerError::PaymentGatewayError(e))
            }
        }
    }

    /// Генерирует токен для запроса на инициацию или подтверждение платежа.
    fn generate_init_token(&self, amount: i64, currency: &str, order_id: &str) -> String {
        let token_string = format!(
            "{}{}{}{}{}",
            amount, currency, order_id, self.password, self.team_slug
        );
        let mut hasher = Sha256::new();
        hasher.update(token_string.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Генерирует специальный токен для запроса на проверку статуса платежа.
    fn generate_check_token(&self, payment_id: &str) -> String {
        let token_string = format!(
            "{}{}{}",
            payment_id, self.password, self.team_slug
        );
        let mut hasher = Sha256::new();
        hasher.update(token_string.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Создаёт платёж в платёжной системе, используя защиту Circuit Breaker.
    pub async fn create_payment(
        &self,
        amount: i64,
        order_id: String,
        description: String,
        email: Option<String>,
        success_url: String,
        fail_url: String,
        webhook_url: String,
    ) -> Result<PaymentInitResponse, CircuitBreakerError> {
        let currency = "KZT"; // Используем KZT как основную валюту.
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

        info!("Creating payment with circuit breaker: amount={}, currency={}", amount, currency);
        info!("Circuit breaker state: {:?}", self.circuit_breaker.get_state());

        let operation = async {
            self
                .http_client
                .post(&format!("{}/api/v1/PaymentInit/init", self.base_url))
                .json(&request)
                .send()
                .await?
                .json::<PaymentInitResponse>()
                .await
        };

        self.execute_with_circuit_breaker(operation).await
    }

    /// Проверяет статус платежа через API, используя защиту Circuit Breaker.
    pub async fn check_payment_status(&self, payment_id: &str) -> Result<PaymentCheckResponse, CircuitBreakerError> {
        let token = self.generate_check_token(payment_id);

        let request = PaymentCheckRequest {
            team_slug: self.team_slug.clone(),
            token,
            payment_id: payment_id.to_string(),
        };

        info!("Checking payment status with circuit breaker: payment_id={}", payment_id);

        let operation = async {
            self
                .http_client
                .post(&format!("{}/api/v1/PaymentCheck/check", self.base_url))
                .json(&request)
                .send()
                .await?
                .json::<PaymentCheckResponse>()
                .await
        };

        self.execute_with_circuit_breaker(operation).await
    }

    /// Подтверждает (списывает средства) авторизованный платёж.
    pub async fn confirm_payment(
        &self,
        payment_id: &str,
        amount: i64,
        currency: &str,
        order_id: &str,
    ) -> Result<PaymentConfirmResponse, CircuitBreakerError> {
        let token = self.generate_init_token(amount, currency, order_id);

        let request = PaymentConfirmRequest {
            team_slug: self.team_slug.clone(),
            token,
            payment_id: payment_id.to_string(),
            amount,
            currency: currency.to_string(),
            order_id: order_id.to_string(),
        };

        info!("Confirming payment with circuit breaker: payment_id={}", payment_id);

        let operation = async {
            self
                .http_client
                .post(&format!("{}/api/v1/PaymentConfirm/confirm", self.base_url))
                .json(&request)
                .send()
                .await?
                .json::<PaymentConfirmResponse>()
                .await
        };

        self.execute_with_circuit_breaker(operation).await
    }

    /// Возвращает текущее состояние Circuit Breaker для мониторинга.
    pub fn get_circuit_breaker_status(&self) -> (CircuitState, u32) {
        (
            self.circuit_breaker.get_state(),
            self.circuit_breaker.failure_count.load(Ordering::Relaxed),
        )
    }

    /// Очищает временные блокировки мест в Redis.
    pub async fn clear_redis_reservations(&self, seat_ids: &[i64]) {
        if seat_ids.is_empty() {
            return;
        }

        let mut redis = self.state.redis.clone();
        let keys: Vec<String> = seat_ids.iter()
            .map(|id| format!("seat:{}", id))
            .collect();
        let _: Result<i64, _> = redis.conn.del(keys).await;
    }

    /// Фоновый процесс для очистки "зависших" и просроченных платежей.
    pub async fn cleanup_expired_payments(&self) {
        // Находим все платежи в статусе 'pending', которые были созданы более 15 минут назад.
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
            // Перед тем как отменить платеж, делаем последнюю попытку проверить его статус через API,
            // но только если Circuit Breaker не в состоянии Open.
            if self.circuit_breaker.can_execute() {
                if let Ok(check_response) = self.check_payment_status(&payment_id).await {
                    if check_response.success {
                        if let Some(status) = &check_response.status {
                            match status.as_str() {
                                "CONFIRMED" | "AUTHORIZED" => {
                                    // Если платеж внезапно прошел, обрабатываем его как успешный.
                                    info!("Payment {} was confirmed during cleanup", payment_id);
                                    self.process_successful_payment(&payment_id, booking_id, event_id).await;
                                    continue; // Переходим к следующему.
                                },
                                _ => {}
                            }
                        }
                    }
                }
            } else {
                warn!("Circuit breaker is OPEN - skipping API check for payment {}, proceeding with cleanup", payment_id);
            }

            // Если API недоступно или статус не изменился, отменяем платеж.
            self.cleanup_expired_payment(payment_id, booking_id, event_id).await;
        }
    }

    /// Логика отмены одного просроченного платежа.
    async fn cleanup_expired_payment(&self, payment_id: String, booking_id: i64, event_id: i64) {
        let mut tx = match self.state.db.pool.begin().await {
            Ok(tx) => tx,
            Err(e) => {
                error!("Failed to start transaction for cleanup: {}", e);
                return;
            }
        };

        // Помечаем платеж как 'expired'.
        sqlx::query("UPDATE payment_transactions SET status = 'expired' WHERE transaction_id = $1")
            .bind(&payment_id)
            .execute(&mut *tx)
            .await.ok();

        // Освобождаем зарезервированные места.
        let seats: Vec<i64> = sqlx::query_scalar("UPDATE seats SET status = 'AVAILABLE', booking_id = NULL WHERE booking_id = $1 AND status = 'RESERVED' RETURNING id")
            .bind(booking_id)
            .fetch_all(&mut *tx)
            .await.unwrap_or_default();

        // Удаляем бронирование.
        sqlx::query("DELETE FROM bookings WHERE id = $1")
            .bind(booking_id)
            .execute(&mut *tx)
            .await.ok();

        // Если транзакция прошла успешно, очищаем кэши.
        if tx.commit().await.is_ok() {
            self.clear_redis_reservations(&seats).await;
            self.state.cache.invalidate_seats(event_id).await;
            info!("Expired payment {} cleaned up, {} seats released", payment_id, seats.len());
        }
    }

    /// Обрабатывает успешное завершение платежа.
    pub async fn process_successful_payment(&self, payment_id: &str, booking_id: i64, event_id: i64) {
        let mut tx = match self.state.db.pool.begin().await {
            Ok(tx) => tx,
            Err(e) => {
                error!("Failed to start transaction for successful payment: {}", e);
                return;
            }
        };

        // 1. Обновляем статус транзакции.
        sqlx::query("UPDATE payment_transactions SET status = 'completed' WHERE transaction_id = $1")
            .bind(payment_id)
            .execute(&mut *tx).await.ok();

        // 2. Обновляем статус бронирования.
        sqlx::query("UPDATE bookings SET status = 'paid' WHERE id = $1")
            .bind(booking_id)
            .execute(&mut *tx).await.ok();

        // 3. Помечаем места как проданные ('SOLD').
        let seats: Vec<i64> = sqlx::query_scalar("UPDATE seats SET status = 'SOLD' WHERE booking_id = $1 AND status = 'RESERVED' RETURNING id")
            .bind(booking_id)
            .fetch_all(&mut *tx).await.unwrap_or_default();

        if tx.commit().await.is_ok() {
            self.clear_redis_reservations(&seats).await;
            self.state.cache.invalidate_seats(event_id).await;
            info!("Payment {} completed, {} seats sold", payment_id, seats.len());
        }
    }

    /// Обрабатывает неудачное завершение платежа (отмена, ошибка).
    pub async fn process_failed_payment(&self, payment_id: &str, booking_id: i64, event_id: i64) {
        let mut tx = match self.state.db.pool.begin().await {
            Ok(tx) => tx,
            Err(e) => {
                error!("Failed to start transaction for failed payment: {}", e);
                return;
            }
        };

        // 1. Обновляем статус транзакции.
        sqlx::query("UPDATE payment_transactions SET status = 'failed' WHERE transaction_id = $1")
            .bind(payment_id)
            .execute(&mut *tx).await.ok();

        // 2. Освобождаем места.
        let seats: Vec<i64> = sqlx::query_scalar("UPDATE seats SET status = 'AVAILABLE', booking_id = NULL WHERE booking_id = $1 AND status = 'RESERVED' RETURNING id")
            .bind(booking_id)
            .fetch_all(&mut *tx).await.unwrap_or_default();

        // 3. Удаляем бронирование.
        sqlx::query("DELETE FROM bookings WHERE id = $1")
            .bind(booking_id)
            .execute(&mut *tx).await.ok();

        if tx.commit().await.is_ok() {
            self.clear_redis_reservations(&seats).await;
            self.state.cache.invalidate_seats(event_id).await;
            info!("Payment {} failed, {} seats released", payment_id, seats.len());
        }
    }

    /// Обрабатывает входящее уведомление (webhook) от платёжной системы.
    pub async fn process_webhook_notification(&self, payment_id: &str, status: &str) {
        info!("Processing webhook: payment_id={}, status={}", payment_id, status);

        // Находим связанное бронирование по ID платежа.
        let booking_info: Option<(i64, i64)> = sqlx::query_as(
            "SELECT b.id, b.event_id FROM bookings b
             JOIN payment_transactions pt ON pt.booking_id = b.id
             WHERE pt.transaction_id = $1"
        )
        .bind(payment_id)
        .fetch_optional(&self.state.db.pool)
        .await.ok().flatten();

        let (booking_id, event_id) = match booking_info {
            Some(info) => info,
            None => {
                warn!("Payment {} not found in database", payment_id);
                return;
            }
        };

        match status {
            "CONFIRMED" => {
                self.process_successful_payment(payment_id, booking_id, event_id).await;
            },
            "AUTHORIZED" => {
                // Пытаемся автоматически подтвердить платёж, если это возможно.
                if self.circuit_breaker.can_execute() {
                    if let Ok(check) = self.check_payment_status(payment_id).await {
                        if let (Some(amount), Some(currency), Some(order_id)) =
                            (check.amount, check.currency, check.order_id) {
                            if self.confirm_payment(payment_id, amount, &currency, &order_id).await.is_ok() {
                                self.process_successful_payment(payment_id, booking_id, event_id).await;
                                return;
                            }
                        }
                    }
                }
                // Если подтвердить не удалось (например, Circuit Breaker открыт),
                // оставляем платеж в 'pending', и он будет обработан позже фоновым процессом.
                warn!("Could not auto-confirm payment {} (circuit breaker or API error), leaving in pending", payment_id);
            },
            "CANCELLED" | "FAILED" | "EXPIRED" | "REFUNDED" => {
                self.process_failed_payment(payment_id, booking_id, event_id).await;
            },
            "NEW" => {
                // Статус 'NEW' не требует действий.
            },
            _ => {
                warn!("Unknown payment status '{}' for payment {}", status, payment_id);
            }
        }
    }
}