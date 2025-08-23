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
- **Payments**: Интеграция с платежным шлюзом + Circuit Breaker

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
- `GET /api/bookings/{booking_id}/payment-status` - Статус платежа по бронированию
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

### 💳 Платежи

#### Публичные эндпоинты:
- `POST /api/webhook/payment` - Webhook для уведомлений от платежного шлюза
  - Body: `{ "paymentId": "string", "status": "CONFIRMED|FAILED|..." }`
- `GET /api/payments/success` - Callback успешной оплаты
  - Query params: `paymentId`, `orderId`
- `GET /api/payments/fail` - Callback неуспешной оплаты  
  - Query params: `paymentId`, `orderId`
- `GET /api/payments/circuit-breaker-status` - Статус circuit breaker для мониторинга

### 🧪 Тестирование (публичные)
- `POST /api/reset` - Сброс всех тестовых данных
  - Очищает: бронирования, платежи, резервы
  - Сохраняет: пользователей, события, структуру мест

## ⚡ Circuit Breaker для платежного шлюза

Система защищена Circuit Breaker'ом для обеспечения стабильности при проблемах с платежным провайдером.

### 🔄 Состояния Circuit Breaker:
- **Closed** - Нормальная работа, все запросы проходят
- **Open** - Блокировка запросов при достижении лимита ошибок  
- **HalfOpen** - Тестирование восстановления после timeout'а

### ⚙️ Конфигурация (через .env):
```bash
CIRCUIT_BREAKER_FAILURE_THRESHOLD=5    # Количество ошибок для открытия
CIRCUIT_BREAKER_TIMEOUT_SECONDS=60     # Время до попытки восстановления
```

### 📊 Мониторинг:
```bash
# Проверить статус Circuit Breaker
curl http://localhost:8000/api/payments/circuit-breaker-status

# Пример ответа:
{
  "success": true,
  "circuit_breaker": {
    "state": "Closed",
    "failure_count": 0,
    "threshold": 5,
    "timeout_seconds": 60
  }
}
```

### 🛡️ Поведение при проблемах:
- При открытом Circuit Breaker новые платежи возвращают HTTP 503
- Webhook'и продолжают обрабатываться локально
- Фоновая очистка пропускает API проверки
- Автоматическое восстановление через указанный timeout

## 🔄 Статусы и коды ответов

### Статусы бронирования
- `created` - Новое бронирование
- `pending_payment` - Ожидает оплаты
- `paid` - Оплачено
- `cancelled` - Отменено

### Статусы платежей
- `pending` - Ожидает обработки
- `completed` - Успешно завершен
- `failed` - Ошибка платежа
- `expired` - Истек срок платежа

### Статусы мест
- `FREE` - Свободно
- `SELECTED` - Временно зарезервировано (5 минут)
- `RESERVED` - Забронировано
- `SOLD` - Продано

### HTTP коды
- `200` - Успешно
- `201` - Создано
- `400` - Некорректный запрос
- `401` - Не авторизован / ошибка аутентификации
- `403` - Доступ запрещен
- `404` - Не найдено
- `409` - Конфликт (дублирующийся платеж)
- `419` - Конфликт (место уже занято / бронирование не найдено)
- `429` - Превышение лимитов
- `500` - Внутренняя ошибка
- `502` - Ошибка платежного шлюза
- `503` - Сервис временно недоступен (Circuit Breaker открыт)

## 🧪 Примеры тестирования

### Полный флоу бронирования с оплатой:
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

# 4. Проверить статус платежа
curl -H "Authorization: Basic base64(email:password)" \
  http://localhost:8000/api/bookings/1/payment-status

# 5. Сброс для нового теста
curl -X POST http://localhost:8000/api/reset
```

### Тестирование платежной системы:
```bash
# Проверить статус Circuit Breaker
curl http://localhost:8000/api/payments/circuit-breaker-status

# Симуляция успешного callback'а
curl "http://localhost:8000/api/payments/success?paymentId=test-payment-123&orderId=booking-1-1640995200"

# Симуляция неуспешного callback'а  
curl "http://localhost:8000/api/payments/fail?paymentId=test-payment-123&orderId=booking-1-1640995200"

# Webhook от платежного шлюза (симуляция)
curl -X POST http://localhost:8000/api/webhook/payment \
  -H "Content-Type: application/json" \
  -d '{
    "paymentId": "test-payment-123",
    "status": "CONFIRMED",
    "teamSlug": "rorobotics",
    "timestamp": "2024-01-01T12:00:00Z"
  }'
```

### Команды для отладки:
```bash
# Проверить резервы в Redis
docker exec -it ticket_system_cache redis-cli KEYS "seat:*"
docker exec -it ticket_system_cache redis-cli TTL "seat:100"

# Проверить кеш событий
docker exec -it ticket_system_cache redis-cli GET "events"
docker exec -it ticket_system_cache redis-cli GET "seats:1"

# Проверить данные в БД
docker exec -i ticket_system_db psql -U ticket_user -d ticket_system -c "SELECT COUNT(*) FROM seats WHERE status='FREE';"
docker exec -i ticket_system_db psql -U ticket_user -d ticket_system -c "SELECT * FROM bookings ORDER BY created_at DESC LIMIT 5;"
docker exec -i ticket_system_db psql -U ticket_user -d ticket_system -c "SELECT * FROM payment_transactions ORDER BY created_at DESC LIMIT 5;"

# Проверить статусы платежей
docker exec -i ticket_system_db psql -U ticket_user -d ticket_system -c "SELECT pt.transaction_id, pt.status, pt.amount, b.id as booking_id FROM payment_transactions pt JOIN bookings b ON b.id = pt.booking_id ORDER BY pt.created_at DESC LIMIT 10;"

# Логи приложения
docker logs ticket_system_app --tail 50 -f
```

## 🚀 Особенности производительности

- **Кеширование**: Места кешируются на 24 часа, события на 1 час
- **Атомарность**: Резервирование мест через Redis SET NX EX (без race conditions)
- **Пагинация**: Максимум 20 записей за запрос
- **Индексы**: FTS для поиска, B-tree для дат и foreign keys
- **Транзакции**: Критические операции (отмена брони, сброс) в транзакциях
- **Circuit Breaker**: Защита от каскадных сбоев платежного провайдера
- **Автоочистка**: Фоновое освобождение истекших резерваций (15 минут TTL)