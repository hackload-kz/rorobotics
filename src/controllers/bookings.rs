//! bookings.rs
//!
//! Модуль для управления бронированиями и местами.
//!
//! Включает в себя следующую функциональность:
//! - Создание и отмена бронирований.
//! - Получение списка бронирований пользователя.
//! - Выбор и освобождение мест в рамках бронирования.
//! - Получение информации о доступных местах для события.
//! - Сброс всех данных для тестирования.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, patch, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::sync::Arc;
use crate::AppState;

/// Определяет маршруты, связанные с бронированиями и местами.
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/seats", get(get_seats))
        .route("/seats/select", patch(select_seat))
        .route("/seats/release", patch(release_seat))
        .route("/bookings", get(get_user_bookings))
        .route("/bookings", post(create_booking))
        .route("/bookings/cancel", patch(cancel_booking))
}

/// Определяет маршрут для сброса данных (только для тестирования).
pub fn reset_route() -> Router<Arc<AppState>> {
    Router::new()
        .route("/reset", post(reset_all_test_data))
}

// --- Вспомогательные функции ---

/// Возвращает кастомный статус-код 419, часто используемый для обозначения конфликта,
/// например, когда место уже занято.
fn status_419() -> StatusCode {
    StatusCode::from_u16(419).unwrap_or(StatusCode::CONFLICT)
}

/// Проверяет, принадлежит ли указанное бронирование пользователю.
async fn booking_belongs_to_user(pool: &sqlx::PgPool, booking_id: i64, user_id: i32) -> sqlx::Result<bool> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM bookings WHERE id = $1 AND user_id = $2)"
    )
    .bind(booking_id)
    .bind(user_user_id_to_i64(user_id))
    .fetch_one(pool)
    .await
}

/// Вспомогательная функция для преобразования user_id (i32) в i64,
/// так как база данных ожидает тип BIGINT для этого поля.
fn user_user_id_to_i64(user_id: i32) -> i64 { user_id as i64 }

/// Получает ID события для указанного бронирования.
async fn booking_event_id(pool: &sqlx::PgPool, booking_id: i64) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar::<_, Option<i64>>(
        "SELECT event_id FROM bookings WHERE id = $1"
    )
    .bind(booking_id)
    .fetch_one(pool)
    .await
}

/// Получает ID события для указанного места.
async fn seat_event_id(pool: &sqlx::PgPool, seat_id: i64) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar::<_, Option<i64>>(
        "SELECT event_id FROM seats WHERE id = $1"
    )
    .bind(seat_id)
    .fetch_one(pool)
    .await
}

// --- Управление бронированиями ---

/// POST /api/bookings
///
/// Создает новое, пустое бронирование для указанного события от имени
/// аутентифицированного пользователя.
#[derive(Debug, Deserialize)]
struct CreateBookingRequest { pub event_id: i64 }

#[derive(Debug, Serialize)]
struct CreateBookingResponse { pub id: i64 }

async fn create_booking(
    State(state): State<Arc<AppState>>,
    user: crate::middleware::AuthUser,
    Json(req): Json<CreateBookingRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if req.event_id <= 0 {
        return Err((StatusCode::BAD_REQUEST, "event_id должен быть > 0".to_string()));
    }

    let res = sqlx::query_scalar::<_, i64>(
        "INSERT INTO bookings (event_id, user_id, status)
         VALUES ($1, $2, 'created')
         RETURNING id"
    )
    .bind(req.event_id)
    .bind(user_user_id_to_i64(user.user_id))
    .fetch_one(&state.db.pool)
    .await;

    match res {
        Ok(id) => Ok((StatusCode::CREATED, Json(CreateBookingResponse{ id }))),
        Err(e) => {
            tracing::error!("create_booking sql error: {:?}", e);
            Err((StatusCode::INTERNAL_SERVER_ERROR, "Не удалось создать бронирование".to_string()))
        }
    }
}

/// GET /api/bookings
///
/// Возвращает список всех бронирований текущего пользователя, включая
/// список зарезервированных мест в каждом из них.
#[derive(Debug, Serialize)]
struct BookingSeat { pub id: i64 }

#[derive(Debug, Serialize)]
struct BookingResponse { pub id: i64, pub event_id: i64, pub seats: Vec<BookingSeat> }

async fn get_user_bookings(
    State(state): State<Arc<AppState>>,
    user: crate::middleware::AuthUser,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Получаем все бронирования и связанные с ними места для пользователя.
    let rows = sqlx::query(
        r#"
        SELECT b.id as bid, b.event_id as eid, s.id as sid
        FROM bookings b
        LEFT JOIN seats s ON s.booking_id = b.id
        WHERE b.user_id = $1
        ORDER BY b.created_at DESC, s.id
        "#
    )
    .bind(user_user_id_to_i64(user.user_id))
    .fetch_all(&state.db.pool)
    .await;

    let rows = rows.map_err(|e| {
        tracing::error!("get_user_bookings sql error: {:?}", e);
        (StatusCode::INTERNAL_SERVER_ERROR, "Не удалось получить список бронирований".to_string())
    })?;

    // Группируем места по бронированиям.
    use std::collections::BTreeMap;
    let mut map: BTreeMap<i64, (i64, Vec<i64>)> = BTreeMap::new();
    for r in rows {
        let bid: i64 = r.get("bid");
        let eid: i64 = r.get("eid");
        let sid: Option<i64> = r.try_get("sid").ok();
        let e = map.entry(bid).or_insert((eid, Vec::new()));
        if let Some(sid) = sid { e.1.push(sid); }
    }

    // Формируем финальный ответ.
    let resp: Vec<BookingResponse> = map.into_iter().map(|(bid,(eid,seats))| BookingResponse{
        id: bid,
        event_id: eid,
        seats: seats.into_iter().map(|s| BookingSeat{ id: s }).collect()
    }).collect();

    Ok((StatusCode::OK, Json(resp)))
}

/// PATCH /api/bookings/cancel
///
/// Отменяет бронирование пользователя. Этот процесс включает несколько шагов
/// и выполняется в рамках одной транзакции для обеспечения целостности данных.
#[derive(Debug, Deserialize)]
struct CancelBookingRequest { pub booking_id: i64 }

async fn cancel_booking(
    State(state): State<Arc<AppState>>,
    user: crate::middleware::AuthUser,
    Json(req): Json<CancelBookingRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if req.booking_id <= 0 {
        return Err((StatusCode::BAD_REQUEST, "booking_id должен быть > 0".to_string()));
    }

    // Проверяем, что пользователь является владельцем этого бронирования.
    let belongs = booking_belongs_to_user(&state.db.pool, req.booking_id, user.user_id)
        .await
        .unwrap_or(false);
    if !belongs {
        return Err((StatusCode::FORBIDDEN, "Бронирование не найдено или не принадлежит вам".to_string()));
    }

    // Получаем event_id для последующей инвалидации кэша.
    let event_id = booking_event_id(&state.db.pool, req.booking_id).await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Ошибка БД".to_string()))?
        .ok_or_else(|| (status_419(), "Бронирование не найдено".to_string()))?;

    // Начинаем транзакцию.
    let mut tx = state.db.pool.begin().await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Ошибка транзакции".to_string()))?;

    // Шаг 1: Освобождаем все зарезервированные места, связанные с этим бронированием,
    // и возвращаем их в статус 'FREE'. Собираем ID этих мест для дальнейших действий.
    let freed_result = sqlx::query_scalar::<_, i64>(
        r#"
        UPDATE seats
        SET status = 'FREE', booking_id = NULL
        WHERE booking_id = $1 AND status = 'RESERVED'
        RETURNING id
        "#
    )
    .bind(req.booking_id)
    .fetch_all(&mut *tx)
    .await;

    let freed: Vec<i64> = match freed_result {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("failed to free seats for booking {}: {:?}", req.booking_id, e);
            let _ = tx.rollback().await; // Откатываем транзакцию в случае ошибки.
            return Err((StatusCode::INTERNAL_SERVER_ERROR, "Не удалось освободить места".to_string()));
        }
    };

    // Шаг 2: Помечаем само бронирование как отмененное.
    let upd_result = sqlx::query("UPDATE bookings SET status = 'cancelled' WHERE id = $1")
        .bind(req.booking_id)
        .execute(&mut *tx)
        .await;

    if let Err(e) = upd_result {
        tracing::error!("failed to update booking {}: {:?}", req.booking_id, e);
        let _ = tx.rollback().await;
        return Err((StatusCode::INTERNAL_SERVER_ERROR, "Не удалось отменить бронирование".to_string()));
    }

    // Шаг 3: Если все прошло успешно, коммитим транзакцию.
    if let Err(e) = tx.commit().await {
        tracing::error!("failed to commit cancel_booking tx for {}: {:?}", req.booking_id, e);
        return Err((StatusCode::INTERNAL_SERVER_ERROR, "Ошибка фиксации транзакции".to_string()));
    }

    // Шаг 4: Очищаем временные блокировки (резервы) в Redis.
    // Это некритичная операция, поэтому ошибка здесь не прервет выполнение.
    {
        let mut pipe = redis::pipe();
        for seat_id in &freed {
            pipe.del(format!("seat:{}:reserved", seat_id));
        }
        // В реальном проекте здесь будет асинхронный вызов к Redis.
    }

    // Шаг 5: Инвалидируем кэш со списком мест для данного события,
    // так как состояние мест изменилось.
    state.cache.invalidate_seats(event_id).await;

    Ok((StatusCode::OK, Json(serde_json::json!({"message":"Бронь успешно отменена"}))))
}

// --- Управление местами ---

/// Параметры запроса для получения списка мест.
#[derive(Debug, Deserialize)]
struct SeatsQuery {
    event_id: i64,
    page: Option<u32>,
    #[serde(rename = "pageSize")]
    page_size: Option<u32>,
    row: Option<i32>,
    status: Option<String>, // FREE, RESERVED, SOLD
}

/// Структура ответа для одного места.
#[derive(Debug, Serialize)]
struct SeatResponse {
    id: i64,
    row: i32,
    number: i32,
    status: String,
}

/// GET /api/seats
///
/// Возвращает список мест для события с возможностью фильтрации и пагинации.
async fn get_seats(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SeatsQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Валидация входных параметров.
    if params.event_id <= 0 {
        return Err((StatusCode::BAD_REQUEST, "event_id должен быть > 0".to_string()));
    }
    if let Some(r) = params.row {
        if r <= 0 { return Err((StatusCode::BAD_REQUEST, "row должен быть > 0".to_string())); }
    }
    if let Some(ref st) = params.status {
        if !matches!(st.as_str(), "FREE" | "RESERVED" | "SOLD") {
            return Err((StatusCode::BAD_REQUEST, "status должен быть FREE | RESERVED | SOLD".to_string()));
        }
    }

    let page = params.page.unwrap_or(1).max(1);
    let page_size = params.page_size.unwrap_or(20).clamp(1, 20);
    let offset = (page - 1) * page_size;

    // Динамически строим SQL-запрос в зависимости от переданных фильтров.
    let mut q = String::from("SELECT id, row, number, status FROM seats WHERE event_id = $1");
    let mut bind_idx = 2;
    if params.row.is_some() {
        q.push_str(&format!(" AND row = ${}", bind_idx));
        bind_idx += 1;
    }
    if params.status.is_some() {
        q.push_str(&format!(" AND status = ${}", bind_idx));
        bind_idx += 1;
    }
    q.push_str(&format!(" ORDER BY row, number LIMIT ${} OFFSET ${}", bind_idx, bind_idx + 1));

    let mut dbq = sqlx::query_as::<_, (i64, i32, i32, String)>(&q)
        .bind(params.event_id);

    // Привязываем параметры к запросу.
    if let Some(r) = params.row { dbq = dbq.bind(r); }
    if let Some(st) = params.status { dbq = dbq.bind(st); }

    let seats = dbq
        .bind(page_size as i64)
        .bind(offset as i64)
        .fetch_all(&state.db.pool)
        .await
        .map_err(|e| {
            tracing::error!("get_seats sql error: {:?}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Не удалось получить список мест".to_string())
        })?;

    let payload: Vec<SeatResponse> = seats.into_iter().map(|(id,row,number,status)| SeatResponse{
        id, row, number, status
    }).collect();

    Ok((StatusCode::OK, Json(payload)))
}

/// PATCH /api/seats/select
///
/// Добавляет выбранное место к бронированию пользователя.
/// Использует Redis для атомарной блокировки места на короткое время,
/// чтобы избежать состояния гонки.
#[derive(Debug, Deserialize)]
struct SelectSeatRequest { booking_id: i64, seat_id: i64 }

async fn select_seat(
    State(state): State<Arc<AppState>>,
    user: crate::middleware::AuthUser,
    Json(req): Json<SelectSeatRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if req.booking_id <= 0 || req.seat_id <= 0 {
        return Err((StatusCode::BAD_REQUEST, "booking_id и seat_id должны быть > 0".to_string()));
    }

    // Проверяем, что бронирование принадлежит пользователю.
    let belongs = booking_belongs_to_user(&state.db.pool, req.booking_id, user.user_id)
        .await
        .unwrap_or(false);
    if !belongs {
        return Err((status_419(), "Бронирование не найдено".to_string()));
    }

    // Пытаемся атомарно зарезервировать место в Redis на 5 минут.
    // Если ключ уже существует, значит, кто-то другой пытается занять это место.
    let reserved = state.cache.reserve_seat(req.seat_id, user.user_id).await;
    if !reserved {
        return Err((status_419(), "Место уже зарезервировано".to_string()));
    }

    // Если резерв в Redis успешен, обновляем статус места в основной базе данных.
    // Обновление произойдет только если место было 'FREE'.
    let ok = sqlx::query(
        r#"
        UPDATE seats
        SET status = 'RESERVED', booking_id = $1
        WHERE id = $2 AND status = 'FREE'
        "#
    )
    .bind(req.booking_id)
    .bind(req.seat_id)
    .execute(&state.db.pool)
    .await
    .map(|r| r.rows_affected() > 0)
    .unwrap_or(false);

    if ok {
        // Если место успешно забронировано в БД, инвалидируем кэш.
        if let Ok(Some(eid)) = seat_event_id(&state.db.pool, req.seat_id).await {
            state.cache.invalidate_seats(eid).await;
        }
        Ok((StatusCode::OK, Json(serde_json::json!({"message":"Место успешно добавлено в бронь"}))))
    } else {
        // Если обновить БД не удалось (например, место уже было занято),
        // необходимо откатить резерв в Redis.
        let mut conn = state.redis.conn.clone();
        let _ : Result<(), _> = redis::cmd("DEL")
            .arg(format!("seat:{}:reserved", req.seat_id))
            .query_async(&mut conn)
            .await;
        Err((status_419(), "Не удалось добавить место в бронь".to_string()))
    }
}

/// PATCH /api/seats/release
///
/// Освобождает место, удаляя его из бронирования пользователя.
#[derive(Debug, Deserialize)]
struct ReleaseSeatRequest { seat_id: i64 }

async fn release_seat(
    State(state): State<Arc<AppState>>,
    user: crate::middleware::AuthUser,
    Json(req): Json<ReleaseSeatRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if req.seat_id <= 0 {
        return Err((StatusCode::BAD_REQUEST, "seat_id должен быть > 0".to_string()));
    }

    // Проверяем, что указанное место действительно зарезервировано
    // текущим пользователем.
    let seat_ok = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS(
          SELECT 1
          FROM seats s
          JOIN bookings b ON b.id = s.booking_id
          WHERE s.id = $1 AND s.status = 'RESERVED' AND b.user_id = $2
        )
        "#
    )
    .bind(req.seat_id)
    .bind(user_user_id_to_i64(user.user_id))
    .fetch_one(&state.db.pool)
    .await
    .unwrap_or(false);

    if !seat_ok {
        return Err((StatusCode::FORBIDDEN, "Место не найдено или не принадлежит вам".to_string()));
    }

    // Обновляем статус места на 'FREE' в базе данных.
    let ok = sqlx::query(
        "UPDATE seats SET status = 'FREE', booking_id = NULL WHERE id = $1 AND status = 'RESERVED'"
    )
    .bind(req.seat_id)
    .execute(&state.db.pool)
    .await
    .map(|r| r.rows_affected() > 0)
    .unwrap_or(false);

    if ok {
        // При успехе удаляем временный резерв из Redis и инвалидируем кэш.
        let mut conn = state.redis.conn.clone();
        let _ : Result<(), _> = redis::cmd("DEL")
            .arg(format!("seat:{}:reserved", req.seat_id))
            .query_async(&mut conn)
            .await;

        if let Ok(Some(eid)) = seat_event_id(&state.db.pool, req.seat_id).await {
            state.cache.invalidate_seats(eid).await;
        }

        Ok((StatusCode::OK, Json(serde_json::json!({"message":"Место успешно освобождено"}))))
    } else {
        Err((status_419(), "Не удалось освободить место".to_string()))
    }
}

/// POST /api/reset
///
/// Специальный эндпоинт для полного сброса всех изменяемых данных в системе.
/// Используется для тестирования. Удаляет все бронирования и платежи,
/// сбрасывает статусы мест и очищает кэш.
async fn reset_all_test_data(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    tracing::warn!("🔴 RESET: Начинаем полный сброс тестовых данных");
    
    // Используем транзакцию, чтобы сброс был атомарным.
    let mut tx = state.db.pool.begin().await
        .map_err(|e| {
            tracing::error!("RESET: Не удалось начать транзакцию: {:?}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Ошибка начала транзакции".to_string())
        })?;

    // Шаг 1: Собираем ID всех событий, затронутых бронированиями, для инвалидации кэша.
    let event_ids: Vec<i64> = sqlx::query_scalar::<_, i64>(
        "SELECT DISTINCT event_id FROM bookings"
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap_or_default();

    // Шаг 2: Сбрасываем все занятые места в статус 'FREE'.
    let freed_seats = sqlx::query(
        r#"
        UPDATE seats
        SET status = 'FREE',
            booking_id = NULL
        WHERE status IN ('RESERVED', 'SELECTED')
        RETURNING id
        "#
    )
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!("RESET: Ошибка сброса мест: {:?}", e);
        (StatusCode::INTERNAL_SERVER_ERROR, "Ошибка сброса мест".to_string())
    })?;

    let seats_reset_count = freed_seats.len();
    tracing::info!("RESET: Сброшено {} мест", seats_reset_count);

    // Шаг 3: Удаляем все платежные транзакции.
    let payment_result = sqlx::query(
        "DELETE FROM payment_transactions"
    )
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!("RESET: Ошибка удаления платежей: {:?}", e);
        (StatusCode::INTERNAL_SERVER_ERROR, "Ошибка удаления платежей".to_string())
    })?;
    
    tracing::info!("RESET: Удалено {} платежных транзакций", payment_result.rows_affected());

    // Шаг 4: Удаляем все бронирования.
    let bookings_result = sqlx::query(
        "DELETE FROM bookings"
    )
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!("RESET: Ошибка удаления бронирований: {:?}", e);
        (StatusCode::INTERNAL_SERVER_ERROR, "Ошибка удаления бронирований".to_string())
    })?;
    
    tracing::info!("RESET: Удалено {} бронирований", bookings_result.rows_affected());

    // Шаг 5: Сбрасываем счетчик ID для таблицы бронирований.
    let _ = sqlx::query(
        "ALTER SEQUENCE bookings_id_seq RESTART WITH 1"
    )
    .execute(&mut *tx)
    .await;

    // Коммитим транзакцию.
    tx.commit().await
        .map_err(|e| {
            tracing::error!("RESET: Ошибка коммита транзакции: {:?}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Ошибка фиксации изменений".to_string())
        })?;

    // Шаг 6: Очищаем все временные резервы мест в Redis.
    let mut redis_conn = state.redis.conn.clone();
    let keys: Vec<String> = redis::cmd("KEYS")
        .arg("seat:*:reserved")
        .query_async(&mut redis_conn)
        .await
        .unwrap_or_default();
    
    if !keys.is_empty() {
        let mut pipe = redis::pipe();
        for key in &keys {
            pipe.del(key);
        }
        let _: Result<(), _> = pipe.query_async(&mut redis_conn).await;
        tracing::info!("RESET: Удалено {} резервов в Redis", keys.len());
    }

    // Шаг 7: Инвалидируем кэш для всех затронутых событий.
    for event_id in &event_ids {
        state.cache.invalidate_seats(*event_id).await;
        tracing::debug!("RESET: Инвалидирован кеш для event_id={}", event_id);
    }

    // Шаг 8: Очищаем весь кэш со списками мест.
    let seat_keys: Vec<String> = redis::cmd("KEYS")
        .arg("seats:*")
        .query_async(&mut redis_conn)
        .await
        .unwrap_or_default();
    
    if !seat_keys.is_empty() {
        let mut pipe = redis::pipe();
        for key in &seat_keys {
            pipe.del(key);
        }
        let _: Result<(), _> = pipe.query_async(&mut redis_conn).await;
        tracing::info!("RESET: Очищено {} кешей мест в Redis", seat_keys.len());
    }

    // Формируем детальный отчет об операции.
    let response = serde_json::json!({
        "status": "success",
        "message": "Все тестовые данные успешно сброшены",
        "details": {
            "seats_reset": seats_reset_count,
            "bookings_deleted": bookings_result.rows_affected(),
            "payments_deleted": payment_result.rows_affected(),
            "redis_reserves_cleared": keys.len(),
            "redis_cache_cleared": seat_keys.len(),
            "events_invalidated": event_ids.len()
        },
        "preserved": {
            "users": "✅ Сохранены",
            "events": "✅ Сохранены",
            "seats_structure": "✅ Сохранена (только статусы сброшены)"
        }
    });

    tracing::warn!("🟢 RESET: Операция завершена успешно");
    
    Ok((StatusCode::OK, Json(response)))
}