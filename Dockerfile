# ---- Chef Stage (Prepare dependency recipe) ----
FROM rust:1.87-slim-bullseye AS chef
WORKDIR /app

# Устанавливаем system dependencies для компиляции rdkafka
RUN apt-get update && apt-get install -y \
    cmake \
    g++ \
    pkg-config \
    libssl-dev \
    libsasl2-dev \
    libzstd-dev \
    liblz4-dev \
    && rm -rf /var/lib/apt/lists/*

# Устанавливаем cargo-chef
RUN cargo install cargo-chef
# Копируем файлы, необходимые для определения зависимостей
COPY Cargo.toml Cargo.lock ./
# Копируем исходники, так как cargo-chef анализирует их для составления "рецепта"
COPY src ./src
# Готовим "рецепт" зависимостей. Он будет описывать, какие крейты нужно скомпилировать.
RUN cargo chef prepare --recipe-path recipe.json

# ---- Planner Stage (Cook/Build dependencies) ----
FROM rust:1.87-slim-bullseye AS planner
WORKDIR /app

# Те же dependencies что и в chef stage
RUN apt-get update && apt-get install -y \
    cmake \
    g++ \
    pkg-config \
    libssl-dev \
    libsasl2-dev \
    libzstd-dev \
    liblz4-dev \
    && rm -rf /var/lib/apt/lists/*

RUN cargo install cargo-chef # cargo-chef нужен и для cook
# Копируем только "рецепт"
COPY --from=chef /app/recipe.json recipe.json
# Собираем только зависимости на основе "рецепта".
# Этот слой будет эффективно кешироваться Docker'ом, если recipe.json (и Cargo.lock, неявно) не изменился.
RUN cargo chef cook --release --recipe-path recipe.json

# ---- Builder Stage (Build the application) ----
FROM rust:1.87-slim-bullseye AS builder
WORKDIR /app

# Build dependencies для rdkafka
RUN apt-get update && apt-get install -y \
    cmake \
    g++ \
    pkg-config \
    libssl-dev \
    libsasl2-dev \
    libzstd-dev \
    liblz4-dev \
    && rm -rf /var/lib/apt/lists/*

# Копируем весь исходный код проекта
COPY Cargo.toml Cargo.lock ./
COPY src ./src
# Копируем предварительно скомпилированные зависимости из planner stage
COPY --from=planner /app/target target
# Копируем кеш скачанных крейтов, чтобы не скачивать их заново
COPY --from=planner /usr/local/cargo/registry /usr/local/cargo/registry
# Собираем сам проект, используя уже скомпилированные зависимости
RUN cargo build --release --locked

# ---- Runtime Stage (Final minimal image) ----
FROM debian:bullseye-slim AS runtime

# Runtime dependencies (только то что нужно для работы приложения)
RUN apt-get update && apt-get install -y \
    ca-certificates \
    tzdata \
    libssl1.1 \
    libsasl2-2 \
    libzstd1 \
    liblz4-1 \
    && rm -rf /var/lib/apt/lists/*

# Устанавливаем таймзону
ENV TZ=Etc/UTC
RUN ln -snf /usr/share/zoneinfo/$TZ /etc/localtime && echo $TZ > /etc/timezone

WORKDIR /app
# Копируем скомпилированный бинарник из builder stage
COPY --from=builder /app/target/release/ticket_system .

# Создаем пользователя без привилегий для запуска приложения
RUN useradd --system --uid 1001 --gid 0 appuser
USER appuser

# Порт, который будет слушать приложение
EXPOSE 8000

# Переменные окружения по умолчанию
ENV RUST_LOG="ticket_system=info,tower_http=info"
ENV PORT="8000"

# Команда для запуска приложения
CMD ["./ticket_system"]