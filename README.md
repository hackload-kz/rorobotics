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
- `events_archive` - События (6B+)
- `bookings` - Бронирования
- `seats` - 100k мест для концерта (event_id=1)
- `payment_transactions` - Платежи

## 🔑 Авторизация

Basic Auth с email:password из таблицы users

```bash
# Пример запроса
curl -H "Authorization: Basic base64(email:password)" http://localhost:8000/api/events
```

## 🏗️ Архитектура

- **Backend**: Rust (Axum)
- **Database**: PostgreSQL
- **Cache**: Redis + атомарные резервы
- **Queue**: Redpanda (Kafka-compatible)

## 📡 API Endpoints

### События
- `GET /api/events` - Список актуальных событий

### Бронирования
- `POST /api/bookings` - Создать пустое бронирование
- `GET /api/bookings` - Список бронирований пользователя
- `PATCH /api/bookings/initiatePayment` - Инициировать оплату
- `PATCH /api/bookings/cancel` - Отменить бронирование

### Места
- `GET /api/seats?event_id=1&page=1&pageSize=20` - Список мест с пагинацией
- `PATCH /api/seats/select` - Добавить место в бронирование
- `PATCH /api/seats/release` - Освободить место

## 🎯 Требования к производительности

- **10,000** одновременных пользователей
- **100,000** мест на концерт
- **80%** билетов продается за первые 4 часа
- Атомарное резервирование без гонок

## ⚡ Кеширование и резервирование

### Структура Redis кеша:
```
events                    # Список актуальных событий (TTL: 1 час)
seats:1                   # Все места события (TTL: 24 часа)
seat:123:reserved         # Резерв места пользователем (TTL: 5 мин)
```

### Флоу резервирования:
1. **Создание бронирования** - пустая запись в БД
2. **Резерв места** - атомарный `SET seat:123:reserved user_id NX EX 300`
3. **Обновление БД** - статус FREE → RESERVED + привязка к booking
4. **Автоосвобождение** - Redis TTL освобождает места через 5 минут

## 🧪 Тестирование

### Команды для отладки:
```bash
# Проверить кеш Redis
docker exec -it ticket_system_cache redis-cli KEYS "*"
docker exec -it ticket_system_cache redis-cli GET events

# Проверить данные в БД
docker exec -i ticket_system_db psql -U ticket_user -d ticket_system -c "SELECT COUNT(*) FROM seats WHERE status='FREE';"

# Логи приложения
docker logs ticket_system_app --tail 50
```

## 🔄 События Redpanda

### Топики:
- `seat.selected` - место зарезервировано
- `booking.created` - бронирование создано  
- `booking.cancelled` - бронирование отменено
- `payment.initiated` - оплата запущена

### Consumer группы:
- `notifications` - email/SMS уведомления
- `analytics` - метрики в реальном времени
- `audit` - логирование всех операций
