use crate::{database::Database, redis_client::RedisClient};
use redis::AsyncCommands;
use tracing::info;
use crate::models::{Event, Seat};

#[derive(Clone)]
pub struct CacheService {
    redis: RedisClient,
    db: Database,
}

impl CacheService {
    pub fn new(redis: RedisClient, db: Database) -> Self {
        Self { redis, db }
    }

    // Прогрев кеша при старте
    pub async fn warmup_cache(&self) {
        info!("Starting cache warmup...");
        
        // Загружаем события
        if let Ok(events) = self.load_events_from_db().await {
            info!("Loaded {} events", events.len());
            let _ = self.save_events_to_cache(&events).await;
        }
        
        // Загружаем места для event_id=1
        if let Ok(seats) = self.load_seats_from_db(1).await {
            info!("Loaded {} seats", seats.len());
            let _ = self.save_seats_to_cache(1, &seats).await;
        }
        
        info!("Cache warmup done");
    }

    // Получить события
    pub async fn get_events(&self) -> Vec<Event> {
        // Сначала пробуем кеш
        if let Ok(events) = self.get_events_from_cache().await {
            return events;
        }
        
        // Если кеш не работает - идем в БД
        if let Ok(events) = self.load_events_from_db().await {
            let _ = self.save_events_to_cache(&events).await;
            return events;
        }
        
        vec![]
    }

    // Получить места с учетом резервов
    pub async fn get_seats(&self, event_id: i64) -> Vec<Seat> {
        // Сначала пробуем кеш
        if let Ok(mut seats) = self.get_seats_from_cache(event_id).await {
            // Обновляем статусы с учетом резервов
            self.update_seats_with_reservations(&mut seats).await;
            return seats;
        }
        
        // Если кеш не работает - идем в БД
        if let Ok(mut seats) = self.load_seats_from_db(event_id).await {
            let _ = self.save_seats_to_cache(event_id, &seats).await;
            self.update_seats_with_reservations(&mut seats).await;
            return seats;
        }
        
        vec![]
    }

    // Атомарно резервировать место на 5 минут
    pub async fn reserve_seat(&self, seat_id: i64, user_id: i32) -> bool {
        let key = format!("seat:{}:reserved", seat_id);
        let mut conn = self.redis.conn.clone();
        
        // SET NX EX - атомарная операция без гонок
        let result: Result<String, _> = redis::cmd("SET")
            .arg(&key)
            .arg(user_id)
            .arg("NX")  // только если ключа нет
            .arg("EX")  // TTL в секундах
            .arg(300)   // 5 минут
            .query_async(&mut conn)
            .await;
            
        result.is_ok()
    }

    // Инвалидировать кеш мест
    pub async fn invalidate_seats(&self, event_id: i64) {
        let key = format!("seats:{}", event_id);
        let mut conn = self.redis.conn.clone();
        let _: Result<(), _> = conn.del(&key).await;
        info!("Invalidated seats cache for event {}", event_id);
    }

    // Проверить зарезервировано ли место пользователем
    pub async fn is_seat_reserved_by_user(&self, seat_id: i64, user_id: i32) -> bool {
        let key = format!("seat:{}:reserved", seat_id);
        let mut conn = self.redis.conn.clone();
        let reserved_user: Option<i32> = conn.get(&key).await.unwrap_or(None);
        reserved_user == Some(user_id)
    }

    // === Работа с БД ===
    
    async fn load_events_from_db(&self) -> Result<Vec<Event>, sqlx::Error> {
        sqlx::query_as::<_, Event>(
            "SELECT id, title, description, type as event_type, datetime_start, provider 
             FROM events_archive 
             WHERE datetime_start > NOW()
             ORDER BY datetime_start"
        )
        .fetch_all(&self.db.pool)
        .await
    }

    async fn load_seats_from_db(&self, event_id: i64) -> Result<Vec<Seat>, sqlx::Error> {
        sqlx::query_as::<_, Seat>(
            "SELECT id, event_id, row, number, status, booking_id, category, price::FLOAT as price
             FROM seats 
             WHERE event_id = $1
             ORDER BY row, number"
        )
        .bind(event_id)
        .fetch_all(&self.db.pool)
        .await
    }

    // === Работа с кешем ===
    
    async fn get_events_from_cache(&self) -> Result<Vec<Event>, redis::RedisError> {
        let mut conn = self.redis.conn.clone();
        let data: String = conn.get("events").await?;
        let events: Vec<Event> = serde_json::from_str(&data).map_err(|_| {
            redis::RedisError::from((redis::ErrorKind::TypeError, "Parse error"))
        })?;
        Ok(events)
    }

    async fn save_events_to_cache(&self, events: &[Event]) -> Result<(), redis::RedisError> {
        let data = serde_json::to_string(events).map_err(|_| {
            redis::RedisError::from((redis::ErrorKind::TypeError, "Serialize error"))
        })?;
        let mut conn = self.redis.conn.clone();
        conn.set_ex("events", data, 3600).await // 1 час
    }

    async fn get_seats_from_cache(&self, event_id: i64) -> Result<Vec<Seat>, redis::RedisError> {
        let mut conn = self.redis.conn.clone();
        let key = format!("seats:{}", event_id);
        let data: String = conn.get(key).await?;
        let seats: Vec<Seat> = serde_json::from_str(&data).map_err(|_| {
            redis::RedisError::from((redis::ErrorKind::TypeError, "Parse error"))
        })?;
        Ok(seats)
    }

    async fn save_seats_to_cache(&self, event_id: i64, seats: &[Seat]) -> Result<(), redis::RedisError> {
        let data = serde_json::to_string(seats).map_err(|_| {
            redis::RedisError::from((redis::ErrorKind::TypeError, "Serialize error"))
        })?;
        let key = format!("seats:{}", event_id);
        let mut conn = self.redis.conn.clone();
        conn.set_ex(key, data, 86400).await // 24 часа
    }

    // === Утилиты ===
    
    // Обновить статусы мест с учетом резервов
    async fn update_seats_with_reservations(&self, seats: &mut [Seat]) {
        let mut conn = self.redis.conn.clone();
        let mut pipe = redis::pipe();

        // 1. Собираем все ключи, которые нужно проверить
        for seat in seats.iter() {
            if seat.status == "FREE" {
                let key = format!("seat:{}:reserved", seat.id);
                pipe.exists(key);
            }
        }

        // 2. Выполняем все команды за один раз
        let results: Vec<bool> = match pipe.query_async(&mut conn).await {
            Ok(res) => res,
            Err(_) => return, // Если Redis упал, ничего не делаем
        };

        // 3. Обновляем статусы локально
        let mut reserved_iter = results.iter();
        for seat in seats.iter_mut() {
            if seat.status == "FREE" {
                if let Some(is_reserved) = reserved_iter.next() {
                    if *is_reserved {
                        seat.status = "SELECTED".to_string();
                    }
                }
            }
        }
    }

}