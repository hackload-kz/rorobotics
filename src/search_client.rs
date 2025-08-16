use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tracing::info;

/// Клиент для полнотекстового поиска через PostgreSQL
#[derive(Clone)]
pub struct SearchClient {
    pool: PgPool,
}

/// Результат поиска события
#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct EventSearchResult {
    pub id: i64,
    pub title: String,
    pub datetime_start: chrono::NaiveDateTime,
    pub rank: Option<f32>,  // Релевантность результата
}

impl SearchClient {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn initialize(&self) -> Result<(), sqlx::Error> {
        info!("Search indexes initialized");
        Ok(())
    }

    pub async fn search_events(
        &self,
        query: &str,
        limit: i64,
        offset: i64,
        from_date: Option<chrono::NaiveDateTime>,
    ) -> Result<Vec<EventSearchResult>, sqlx::Error> {
        let search_query_val = Self::prepare_search_query(query);

        let table_name = if from_date.is_none() || 
            from_date.map(|d| d > chrono::Utc::now().naive_utc() - chrono::Duration::days(90)).unwrap_or(true) {
            "events_current"
        } else {
            "events_archive"
        };
        
        let sql = format!(r#"
            SELECT 
                id,
                title,
                datetime_start,
                ts_rank(search_vector, plainto_tsquery('russian', $1)) as rank
            FROM {},
                 plainto_tsquery('russian', $1) as query_ts
            WHERE 
                (search_vector @@ query_ts OR $1 = '') -- Полнотекстовый поиск опционален, если query пуст
                AND (datetime_start >= $4 OR $4 IS NULL) -- Фильтр по дате "от", игнорируется если $6 NULL
                AND (CASE WHEN $4 IS NULL THEN datetime_start > NOW() ELSE TRUE END) -- По умолчанию будущие события, если date не указан
            ORDER BY 
                CASE WHEN $1 = '' THEN datetime_start END ASC, -- Сортировка по дате для пустых запросов
                CASE WHEN $1 != '' THEN ts_rank(search_vector, query_ts) END DESC, -- Сортировка по релевантности для непустых запросов
                datetime_start ASC -- Дополнительная сортировка по дате
            LIMIT $2 OFFSET $3
        "#, table_name);

        let results = sqlx::query_as::<_, EventSearchResult>(&sql)
            .bind(search_query_val)      // $1: search_query
            .bind(limit)                 // $2: limit (pageSize)
            .bind(offset)                // $3: offset (from page)
            .bind(from_date)             // $6: from_date (Option<chrono::NaiveDateTime>)
            .fetch_all(&self.pool)
            .await?;
        
        Ok(results)
    }

    pub async fn suggest_events(
        &self,
        prefix: &str,
        limit: i64,
    ) -> Result<Vec<(i64, String)>, sqlx::Error> {
        let results = sqlx::query_as::<_, (i64, String)>(
            r#"
            SELECT DISTINCT ON (title) id, title
            FROM events_archive
            WHERE 
                title ILIKE $1 || '%'
                AND datetime_start > NOW()
            ORDER BY title, datetime_start DESC
            LIMIT $2
            "#
        )
        .bind(prefix)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(results)
    }

    pub async fn find_similar_events(
        &self,
        event_id: i64,
        limit: i64,
    ) -> Result<Vec<EventSearchResult>, sqlx::Error> {
        let results = sqlx::query_as::<_, EventSearchResult>(
            r#"
            WITH target_event AS (
                SELECT search_vector
                FROM events_archive
                WHERE id = $1
            )
            SELECT 
                e.id,
                e.title,
                e.description,
                e.datetime_start,
                e.provider,
                ts_rank(e.search_vector, t.search_vector) as rank
            FROM events_archive e, target_event t
            WHERE 
                e.id != $1
                AND e.datetime_start > NOW()
            ORDER BY 
                ts_rank(e.search_vector, t.search_vector) DESC,
                e.datetime_start ASC
            LIMIT $2
            "#
        )
        .bind(event_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(results)
    }

    /// Получить популярные типы событий (для фильтров)
    pub async fn get_event_types(&self) -> Result<Vec<(String, i64)>, sqlx::Error> {
        let results = sqlx::query_as::<_, (String, i64)>(
            r#"
            SELECT type, COUNT(*) as count
            FROM events_archive
            WHERE datetime_start > NOW()
            GROUP BY type
            ORDER BY count DESC
            LIMIT 20
            "#
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(results)
    }

    /// Получить популярных провайдеров (для фильтров)
    pub async fn get_providers(&self) -> Result<Vec<(String, i64)>, sqlx::Error> {
        let results = sqlx::query_as::<_, (String, i64)>(
            r#"
            SELECT provider, COUNT(*) as count
            FROM events_archive
            WHERE datetime_start > NOW()
            GROUP BY provider
            ORDER BY count DESC
            LIMIT 50
            "#
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(results)
    }

    /// Подготавливает поисковый запрос (экранирует спецсимволы)
    fn prepare_search_query(query: &str) -> String {
        // Убираем спецсимволы и лишние пробелы
        query
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '-')
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }
}
