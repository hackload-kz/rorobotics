<div align="center">
 <img src="docs/images/main.png" alt="Preview" height="200"/>
<h1>High-Load Ticket System 🎟️</h1>
</div>

## 🚀 Быстрый старт

```bash
# Копируем конфигурацию
cp .env.example .env

# Запускаем инфраструктуру и приложение
make dev

# Загружаем данные (после запуска приложения)
docker exec -i ticket_system_db psql -U ticket_user -d ticket_system < data/users.sql
docker exec -i ticket_system_db psql -U ticket_user -d ticket_system < data/events.sql
docker exec -i ticket_system_db psql -U ticket_user -d ticket_system < data/events_archive.sql
docker exec -i ticket_system_db psql -U ticket_user -d ticket_system < data/seats.sql
```

## 📦 Команды Make

```bash
make dev        # Development режим с hot-reload
make dev-build  # Development режим со сборкой
make prod       # Production режим
make clean      # Очистка volumes и контейнеров
```

## 🗄️ База данных

**Таблицы:**
- `users` - 1М пользователей для Basic Auth
- `events_archive` - События (6B+ записей)
- `events_current` - View актуальных событий (последние 3 месяца)
- `bookings` - Бронирования
- `seats` - 100k мест для концерта (event_id=1)
- `payment_transactions` - Платежи

## 🔑 Авторизация

Basic Auth с email:password из таблицы users

```bash
# Пример запроса с авторизацией
curl -H "Authorization: Basic base64(email:password)" http://localhost:8000/api/bookings

# Пример без авторизации (публичные эндпоинты)
curl http://localhost:8000/api/events
```

## 🏗️ Архитектура

- **Backend**: Rust (Axum)
- **Database**: PostgreSQL с полнотекстовым поиском
- **Cache**: Redis + атомарные резервы (5 минут TTL)

## 📡 API Endpoints

### 🎭 События (публичные)
- `GET /api/events` - Список актуальных событий
  - Query params: `page`, `pageSize`, `type`, `provider`, `date_from`, `date_to`
- `GET /api/events/search` - Полнотекстовый поиск по событиям
  - Query params: `q` (поисковый запрос), `page`, `pageSize`

### 🎫 Бронирования (требуют авторизацию)
- `POST /api/bookings` - Создать пустое бронирование
  - Body: `{ "event_id": 1 }`
- `GET /api/bookings` - Список бронирований пользователя
- `PATCH /api/bookings/initiatePayment` - Инициировать оплату
  - Body: `{ "booking_id": 1 }`
- `PATCH /api/bookings/cancel` - Отменить бронирование
  - Body: `{ "booking_id": 1 }`

### 💺 Места (требуют авторизацию)
- `GET /api/seats` - Список мест с пагинацией и фильтрацией
  - Query params: 
    - `event_id` (обязательный)
    - `page` (default: 1)
    - `pageSize` (default: 20, max: 20)
    - `row` (фильтр по ряду)
    - `status` (FREE | RESERVED | SOLD)
- `PATCH /api/seats/select` - Добавить место в бронирование (атомарный резерв на 5 минут)
  - Body: `{ "booking_id": 1, "seat_id": 1 }`
- `PATCH /api/seats/release` - Освободить место из бронирования
  - Body: `{ "seat_id": 1 }`

### 🧪 Тестирование (публичные)
- `POST /api/reset` - Сброс всех тестовых данных
  - Очищает: бронирования, платежи, резервы
  - Сохраняет: пользователей, события, структуру мест

## 🔄 Статусы и коды ответов

### Статусы бронирования
- `created` - Новое бронирование
- `pending_payment` - Ожидает оплаты
- `cancelled` - Отменено

### Статусы мест
- `FREE` - Свободно
- `SELECTED` - Временно зарезервировано (5 минут)
- `RESERVED` - Забронировано
- `SOLD` - Продано

### HTTP коды
- `200` - Успешно
- `201` - Создано
- `400` - Некорректный запрос
- `401` - Не авторизован
- `403` - Доступ запрещен
- `419` - Конфликт (место уже занято / бронирование не найдено)
- `500` - Внутренняя ошибка

## 🧪 Примеры тестирования

### Полный флоу бронирования:
```bash
# 1. Создать бронирование
curl -X POST http://localhost:8000/api/bookings \
  -H "Authorization: Basic base64(email:password)" \
  -H "Content-Type: application/json" \
  -d '{"event_id": 1}'

# 2. Выбрать места
curl -X PATCH http://localhost:8000/api/seats/select \
  -H "Authorization: Basic base64(email:password)" \
  -H "Content-Type: application/json" \
  -d '{"booking_id": 1, "seat_id": 100}'

# 3. Инициировать платеж
curl -X PATCH http://localhost:8000/api/bookings/initiatePayment \
  -H "Authorization: Basic base64(email:password)" \
  -H "Content-Type: application/json" \
  -d '{"booking_id": 1}'

# 4. Сброс для нового теста
curl -X POST http://localhost:8000/api/reset
```

### Команды для отладки:
```bash
# Проверить резервы в Redis
docker exec -it ticket_system_cache redis-cli KEYS "seat:*:reserved"
docker exec -it ticket_system_cache redis-cli TTL "seat:100:reserved"

# Проверить кеш событий
docker exec -it ticket_system_cache redis-cli GET "events"
docker exec -it ticket_system_cache redis-cli GET "seats:1"

# Проверить данные в БД
docker exec -i ticket_system_db psql -U ticket_user -d ticket_system -c "SELECT COUNT(*) FROM seats WHERE status='FREE';"
docker exec -i ticket_system_db psql -U ticket_user -d ticket_system -c "SELECT * FROM bookings ORDER BY created_at DESC LIMIT 5;"

# Логи приложения
docker logs ticket_system_app --tail 50 -f
```

## 🚀 Особенности производительности

- **Кеширование**: Места кешируются на 24 часа, события на 1 час
- **Атомарность**: Резервирование мест через Redis SET NX EX (без race conditions)
- **Пагинация**: Максимум 20 записей за запрос
- **Индексы**: FTS для поиска, B-tree для дат и foreign keys
- **Транзакции**: Критические операции (отмена брони, сброс) в транзакциях