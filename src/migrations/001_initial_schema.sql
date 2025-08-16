CREATE TABLE IF NOT EXISTS users (
    user_id SERIAL PRIMARY KEY,
    email VARCHAR(255) UNIQUE NOT NULL,
    password_hash VARCHAR(64) NOT NULL,
    password_plain VARCHAR(255),  -- For testing purposes only
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
    provider TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS bookings (
    id BIGSERIAL PRIMARY KEY,
    event_id BIGINT NOT NULL REFERENCES events_archive(id),
    user_id INTEGER REFERENCES users(user_id),
    status VARCHAR(50) DEFAULT 'created',
    created_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS seats (
    id BIGSERIAL PRIMARY KEY,
    event_id BIGINT NOT NULL REFERENCES events_archive(id),
    row INTEGER NOT NULL,
    number INTEGER NOT NULL,
    status VARCHAR(20) DEFAULT 'FREE',
    booking_id BIGINT REFERENCES bookings(id),
    category VARCHAR(50),
    price DECIMAL(10, 2),
    UNIQUE(event_id, row, number)
);

CREATE TABLE IF NOT EXISTS payment_transactions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    booking_id BIGINT REFERENCES bookings(id),
    transaction_id VARCHAR(255) UNIQUE,
    amount DECIMAL(10, 2) NOT NULL,
    status VARCHAR(50),
    created_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX idx_bookings_user ON bookings(user_id);
CREATE INDEX idx_bookings_event ON bookings(event_id);
CREATE INDEX idx_seats_event ON seats(event_id);
CREATE INDEX idx_seats_status ON seats(status);
CREATE INDEX idx_payment_booking ON payment_transactions(booking_id);