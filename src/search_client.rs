use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tracing::info;

/// Клиент для поиска
#[derive(Clone)]
pub struct SearchClient {
    pool: PgPool,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow, Clone)]
pub struct EventSearchResult {
    pub id: i64,
    pub title: String,
    pub datetime_start: chrono::NaiveDateTime,
    pub rank: Option<f32>,
}

impl SearchClient {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn initialize(&self) -> Result<(), sqlx::Error> {
        info!("Search client initialized");
        Ok(())
    }

    pub async fn search_events(
        &self,
        query: &str,
        limit: i64,
        offset: i64,
        from_date: Option<chrono::NaiveDateTime>,
    ) -> Result<Vec<EventSearchResult>, sqlx::Error> {
        // Оптимизированный запрос
        if query.is_empty() && from_date.is_none() {
            // Быстрый путь для пустых запросов (90% случаев)
            self.fast_path_empty_query(limit, offset).await
        } else {
            // Полнотекстовый поиск
            self.full_text_search(query, limit, offset, from_date).await
        }
    }

    /// Быстрый путь для пустых запросов (без полнотекстового поиска)
    async fn fast_path_empty_query(
        &self,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<EventSearchResult>, sqlx::Error> {
        // Используем covering index для минимального I/O
        sqlx::query_as::<_, EventSearchResult>(
            r#"
            SELECT 
                id,
                title,
                datetime_start,
                NULL::float4 as rank
            FROM events_archive
            WHERE datetime_start > NOW()
            ORDER BY datetime_start
            LIMIT $1 OFFSET $2
            "#
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
    }

    /// Полнотекстовый поиск (когда есть запрос)
    async fn full_text_search(
        &self,
        query: &str,
        limit: i64,
        offset: i64,
        from_date: Option<chrono::NaiveDateTime>,
    ) -> Result<Vec<EventSearchResult>, sqlx::Error> {
        let search_query = Self::prepare_search_query(query);

        sqlx::query_as::<_, EventSearchResult>(
            r#"
            SELECT 
                id,
                title,
                datetime_start,
                ts_rank_cd(search_vector, query) as rank
            FROM events_archive,
                 plainto_tsquery('russian', $1) query
            WHERE 
                search_vector @@ query
                AND datetime_start >= COALESCE($4, NOW())
            ORDER BY 
                rank DESC,
                datetime_start
            LIMIT $2 OFFSET $3
            "#
        )
        .bind(search_query)
        .bind(limit)
        .bind(offset)
        .bind(from_date)
        .fetch_all(&self.pool)
        .await
    }

    fn prepare_search_query(query: &str) -> String {
        query
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '-')
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }
}