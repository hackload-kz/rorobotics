CREATE TABLE IF NOT EXISTS users (
    user_id SERIAL PRIMARY KEY,
    email VARCHAR(255) UNIQUE NOT NULL,
    password_hash VARCHAR(64) NOT NULL,
    password_plain VARCHAR(255),
    first_name VARCHAR(100) NOT NULL,
    surname VARCHAR(100) NOT NULL,
    birthday DATE,
    registered_at TIMESTAMP NOT NULL,
    is_active BOOLEAN NOT NULL,
    last_logged_in TIMESTAMP NOT NULL
);

CREATE TABLE IF NOT EXISTS events_archive (
    id BIGSERIAL PRIMARY KEY,
    title TEXT NOT NULL,
    description TEXT,
    type TEXT NOT NULL,
    datetime_start TIMESTAMP NOT NULL,
    provider TEXT NOT NULL,
    search_vector tsvector GENERATED ALWAYS AS (
        setweight(to_tsvector('russian', coalesce(title, '')), 'A') ||
        setweight(to_tsvector('russian', coalesce(description, '')), 'B') ||
        setweight(to_tsvector('russian', coalesce(type, '')), 'C') ||
        setweight(to_tsvector('russian', coalesce(provider, '')), 'C')
    ) STORED
);

CREATE INDEX IF NOT EXISTS idx_events_search_vector ON events_archive USING GIN (search_vector);
CREATE INDEX IF NOT EXISTS idx_events_datetime_start ON events_archive (datetime_start DESC);
CREATE INDEX IF NOT EXISTS idx_events_type ON events_archive (type);
CREATE INDEX IF NOT EXISTS idx_events_provider ON events_archive (provider);

CREATE TABLE IF NOT EXISTS bookings (
    id BIGSERIAL PRIMARY KEY,
    event_id BIGINT NOT NULL,
    user_id INTEGER REFERENCES users(user_id),
    status VARCHAR(50) DEFAULT 'created',
    created_at TIMESTAMPTZ DEFAULT NOW()
);

ALTER TABLE bookings DROP CONSTRAINT IF EXISTS fk_bookings_event;
ALTER TABLE bookings DROP COLUMN IF EXISTS event_datetime_start;
ALTER TABLE bookings ADD CONSTRAINT bookings_event_id_fkey FOREIGN KEY (event_id) REFERENCES events_archive(id);


CREATE TABLE IF NOT EXISTS seats (
    id BIGSERIAL PRIMARY KEY,
    event_id BIGINT NOT NULL,
    row INTEGER NOT NULL,
    number INTEGER NOT NULL,
    status VARCHAR(20) DEFAULT 'FREE',
    booking_id BIGINT REFERENCES bookings(id),
    category VARCHAR(50),
    price DECIMAL(10, 2),
    UNIQUE(event_id, row, number)
);

ALTER TABLE seats DROP CONSTRAINT IF EXISTS fk_seats_event;
ALTER TABLE seats DROP COLUMN IF EXISTS event_datetime_start;
ALTER TABLE seats ADD CONSTRAINT seats_event_id_fkey FOREIGN KEY (event_id) REFERENCES events_archive(id);


CREATE TABLE IF NOT EXISTS payment_transactions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    booking_id BIGINT REFERENCES bookings(id),
    transaction_id VARCHAR(255) UNIQUE,
    amount DECIMAL(10, 2) NOT NULL,
    status VARCHAR(50),
    created_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE OR REPLACE VIEW events_current AS
SELECT * FROM events_archive
WHERE datetime_start > CURRENT_DATE - interval '3 months';

CREATE INDEX IF NOT EXISTS idx_bookings_user ON bookings(user_id);
CREATE INDEX IF NOT EXISTS idx_bookings_event ON bookings(event_id);
CREATE INDEX IF NOT EXISTS idx_seats_event ON seats(event_id);
CREATE INDEX IF NOT EXISTS idx_seats_status ON seats(status);
CREATE INDEX IF NOT EXISTS idx_payment_booking ON payment_transactions(booking_id);