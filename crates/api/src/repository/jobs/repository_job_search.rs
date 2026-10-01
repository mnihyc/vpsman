use anyhow::Result;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{postgres::PgRow, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::{model::JobHistoryView, repository::Repository};

#[path = "job_search_expression.rs"]
mod expression;
pub(crate) use expression::{search_fields, SearchField};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct JobSearchRequest {
    #[serde(default)]
    pub(crate) q: String,
    pub(crate) limit: Option<i64>,
    #[serde(default)]
    pub(crate) sort: Vec<JobSearchSort>,
    pub(crate) cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct JobSearchSort {
    pub(crate) id: String,
    pub(crate) desc: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct JobSearchPage {
    pub(crate) rows: Vec<JobHistoryView>,
    pub(crate) total: i64,
    pub(crate) next_cursor: Option<String>,
    pub(crate) as_of: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct JobSearchValuesQuery {
    pub(crate) field: String,
    #[serde(default)]
    pub(crate) prefix: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct JobSearchValue {
    pub(crate) value: String,
    pub(crate) label: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    query_hash: String,
    #[serde(with = "timestamp_wire")]
    as_of: DateTime<Utc>,
    keys: Vec<Option<String>>,
    id: Uuid,
}

struct SortKey {
    sql: &'static str,
    cast: &'static str,
    desc: bool,
}

pub(crate) struct JobSearch {
    filter: expression::Filter,
    limit: i64,
    sort: Vec<SortKey>,
    query_hash: String,
    cursor: Option<Cursor>,
    as_of: DateTime<Utc>,
}

impl JobSearch {
    pub(crate) fn uses_field(&self, name: &str) -> bool {
        self.filter.uses_field(name)
    }

    pub(crate) fn parse(request: &JobSearchRequest) -> Result<Self, String> {
        let limit = request.limit.unwrap_or(12);
        if !(1..=1000).contains(&limit) {
            return Err("Page size must be between 1 and 1,000.".into());
        }
        let mut sort = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for key in &request.sort {
            let (sql, cast) = match key.id.as_str() {
                "operation" | "type" | "command_type" => ("j.command_type", "text"),
                "targets" | "target_count" => ("j.target_count", "bigint"),
                "result" | "status" => ("j.status", "text"),
                "duration" => ("CASE WHEN j.completed_at IS NULL THEN -1 ELSE GREATEST(0,EXTRACT(EPOCH FROM (j.completed_at-j.created_at))) END", "numeric"),
                "startedBy" | "actor_id" => ("COALESCE(j.actor_id::text,'worker')", "text"),
                "age" | "created_at" => ("j.created_at", "timestamptz"),
                "completed_at" => ("j.completed_at", "timestamptz"),
                _ => return Err(format!("Unsupported Job history sort '{}'.", key.id)),
            };
            if !seen.insert(sql) {
                return Err("Duplicate sort field.".into());
            }
            sort.push(SortKey {
                sql,
                cast,
                desc: key.desc,
            });
        }
        if sort.is_empty() {
            sort.push(SortKey {
                sql: "j.created_at",
                cast: "timestamptz",
                desc: true,
            });
        }
        let query_hash = vpsman_common::payload_hash(
            &serde_json::to_vec(&serde_json::json!({
                "q": request.q, "sort": request.sort, "limit": limit,
            }))
            .map_err(|error| error.to_string())?,
        );
        let cursor = request
            .cursor
            .as_deref()
            .map(|raw| {
                // A cursor contains at most seven sort keys and an ID, never rows.
                if raw.len() > 8192 {
                    return Err("Invalid search cursor.".to_string());
                }
                let bytes = URL_SAFE_NO_PAD
                    .decode(raw)
                    .map_err(|_| "Invalid search cursor.")?;
                let cursor: Cursor =
                    serde_json::from_slice(&bytes).map_err(|_| "Invalid search cursor.")?;
                if cursor.query_hash != query_hash || cursor.keys.len() != sort.len() {
                    return Err(
                        "The search cursor belongs to a different query, sort or page size.".into(),
                    );
                }
                for (key, value) in sort.iter().zip(&cursor.keys) {
                    if let Some(value) = value {
                        let valid = match key.cast {
                            "bigint" => value.parse::<i64>().is_ok(),
                            "numeric" => value.parse::<f64>().is_ok_and(f64::is_finite),
                            "timestamptz" => DateTime::parse_from_rfc3339(value).is_ok(),
                            _ => true,
                        };
                        if !valid {
                            return Err("Invalid search cursor value.".into());
                        }
                    }
                }
                Ok(cursor)
            })
            .transpose()?;
        let as_of = cursor.as_ref().map(|c| c.as_of).unwrap_or_else(Utc::now);
        let filter = expression::parse(&request.q, as_of)?;
        Ok(Self {
            filter,
            limit,
            sort,
            query_hash,
            cursor,
            as_of,
        })
    }

    fn query(&self) -> QueryBuilder<'static, Postgres> {
        let mut sql = QueryBuilder::new(
            "SELECT j.id,j.actor_id,j.command_type,j.source_schedule_id,j.causation_id,j.schedule_lineage,j.privileged,j.status,j.target_count,j.payload_hash,j.max_timeout_secs,j.created_at::text AS created_at,j.completed_at::text AS completed_at, jsonb_build_array("
        );
        for (index, key) in self.sort.iter().enumerate() {
            if index > 0 {
                sql.push(",");
            }
            // JSON timestamp encoding uses RFC3339, preserving microseconds.
            sql.push("to_jsonb((").push(key.sql).push(")");
            if key.cast != "timestamptz" {
                sql.push("::text");
            }
            sql.push(")");
        }
        sql.push(") AS search_keys");
        self.push_source(&mut sql);
        if let Some(cursor) = &self.cursor {
            let uniform_nonnull = self
                .sort
                .iter()
                .all(|key| key.sql != "j.completed_at" && key.desc == self.sort[0].desc)
                && cursor.keys.iter().all(Option::is_some);
            if uniform_nonnull {
                // A row comparison gives PostgreSQL one seek boundary for
                // deep pages (particularly the existing date + ID index).
                sql.push(" AND (");
                for key in &self.sort {
                    sql.push(key.sql).push(",");
                }
                sql.push("j.id)")
                    .push(if self.sort[0].desc { " < (" } else { " > (" });
                for (key, value) in self.sort.iter().zip(&cursor.keys) {
                    bind_cursor(&mut sql, key, value);
                    sql.push(",");
                }
                sql.push_bind(cursor.id).push(")");
            } else {
                sql.push(" AND (");
                // Lexicographic keyset with explicit NULLS LAST, including when a
                // nullable final key is equal and the UUID is the tie breaker.
                for index in 0..=self.sort.len() {
                    if index > 0 {
                        sql.push(" OR ");
                    }
                    sql.push("(");
                    for (prior, key) in self.sort.iter().enumerate().take(index) {
                        sql.push(key.sql).push(" IS NOT DISTINCT FROM ");
                        bind_cursor(&mut sql, key, &cursor.keys[prior]);
                        sql.push(" AND ");
                    }
                    if index == self.sort.len() {
                        sql.push("j.id")
                            .push(if self.sort[0].desc { " < " } else { " > " })
                            .push_bind(cursor.id);
                    } else {
                        let key = &self.sort[index];
                        if cursor.keys[index].is_none() {
                            sql.push("FALSE");
                        } else {
                            sql.push("(")
                                .push(key.sql)
                                .push(if key.desc { " < " } else { " > " });
                            bind_cursor(&mut sql, key, &cursor.keys[index]);
                            sql.push(" OR ").push(key.sql).push(" IS NULL)");
                        }
                    }
                    sql.push(")");
                }
                sql.push(")");
            }
        }
        sql.push(" ORDER BY ");
        for key in &self.sort {
            sql.push(key.sql)
                .push(if key.desc { " DESC" } else { " ASC" });
            // Only completion is nullable. Requiring NULLS LAST on NOT NULL
            // creation time prevents use of the existing descending index.
            if key.sql == "j.completed_at" {
                sql.push(" NULLS LAST");
            }
            sql.push(",");
        }
        sql.push(if self.sort[0].desc {
            "j.id DESC"
        } else {
            "j.id ASC"
        });
        sql.push(" LIMIT ").push_bind(self.limit + 1);
        sql
    }

    fn push_source(&self, sql: &mut QueryBuilder<'static, Postgres>) {
        sql.push(" FROM jobs j LEFT JOIN operators actor ON actor.id=j.actor_id LEFT JOIN schedules schedule ON schedule.id=j.source_schedule_id WHERE j.created_at <= ")
            .push_bind(self.as_of).push(" AND (");
        self.filter.push(sql, self.as_of);
        sql.push(")");
    }

    #[cfg(test)]
    pub(crate) async fn explain(&self, pool: &sqlx::PgPool) -> Result<Vec<serde_json::Value>> {
        use sqlx::Execute;
        let mut count = QueryBuilder::new("SELECT count(*)");
        self.push_source(&mut count);
        let mut plans = Vec::new();
        for mut builder in [count, self.query()] {
            let mut query = builder.build();
            let statement = format!("EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) {}", query.sql());
            let arguments = query
                .take_arguments()
                .map_err(|error| anyhow::anyhow!("{error}"))?
                .unwrap();
            let plan = sqlx::query_scalar_with::<_, serde_json::Value, _>(&statement, arguments)
                .fetch_one(pool)
                .await?;
            plans.push(plan);
        }
        Ok(plans)
    }
}

fn bind_cursor(sql: &mut QueryBuilder<'static, Postgres>, key: &SortKey, value: &Option<String>) {
    sql.push_bind(value.clone()).push("::").push(key.cast);
}

impl Repository {
    pub(crate) async fn search_jobs(&self, search: &JobSearch) -> Result<JobSearchPage> {
        let Self::Postgres(pool) = self;
        let mut regexes = Vec::new();
        search.filter.regexes(&mut regexes);
        if !regexes.is_empty() {
            // PostgreSQL validates patterns even if no historical row matches.
            sqlx::query("SELECT '' ~ pattern FROM unnest($1::text[]) patterns(pattern)")
                .bind(regexes)
                .fetch_all(pool)
                .await?;
        }
        // Count and page describe the same database snapshot. This is a
        // read-only MVCC transaction, with no lifecycle or ownership locks.
        let mut tx = pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await?;
        let mut count = QueryBuilder::new("SELECT count(*)");
        search.push_source(&mut count);
        let total: i64 = count.build_query_scalar().fetch_one(&mut *tx).await?;
        let mut sql = search.query();
        let mut records = sql.build().fetch_all(&mut *tx).await?;
        tx.commit().await?;
        let more = records.len() > search.limit as usize;
        records.truncate(search.limit as usize);
        let next_cursor = if more {
            let last = records.last().expect("positive page size");
            let keys: serde_json::Value = last.try_get("search_keys")?;
            let cursor = Cursor {
                query_hash: search.query_hash.clone(),
                as_of: search.as_of,
                id: last.try_get("id")?,
                keys: keys
                    .as_array()
                    .expect("SQL array")
                    .iter()
                    .map(|v| match v {
                        serde_json::Value::Null => None,
                        serde_json::Value::String(text) => Some(text.clone()),
                        _ => Some(v.to_string()),
                    })
                    .collect(),
            };
            Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&cursor)?))
        } else {
            None
        };
        let rows = records
            .iter()
            .map(history_row)
            .collect::<Result<Vec<_>>>()?;
        Ok(JobSearchPage {
            rows,
            total,
            next_cursor,
            as_of: search.as_of.to_rfc3339(),
        })
    }

    pub(crate) async fn job_search_values(
        &self,
        query: &JobSearchValuesQuery,
    ) -> Result<Vec<JobSearchValue>> {
        let Self::Postgres(pool) = self;
        let pattern = expression::like_pattern(&query.prefix, true);
        // Hints are a bounded dropdown, not the search result set. Match across
        // historical IDs, not only the newest job page or currently live VPSs.
        let sql = match query.field.as_str() {
            "target" | "client_id" => "SELECT DISTINCT t.client_id AS value, COALESCE(c.display_name,t.client_id) AS label FROM job_targets t LEFT JOIN clients c ON c.id=t.client_id WHERE t.client_id ILIKE $1 ESCAPE '\\' OR c.display_name ILIKE $1 ESCAPE '\\' ORDER BY value LIMIT 20",
            "actor" => "SELECT username AS value, username AS label FROM operators WHERE username ILIKE $1 ESCAPE '\\' ORDER BY username LIMIT 20",
            "actor_id" => "SELECT DISTINCT actor_id::text AS value,actor_id::text AS label FROM jobs WHERE actor_id::text ILIKE $1 ESCAPE '\\' ORDER BY value LIMIT 20",
            "schedule" => "SELECT name AS value,name AS label FROM schedules WHERE name ILIKE $1 ESCAPE '\\' ORDER BY name,id LIMIT 20",
            "schedule_id" | "source_schedule_id" => "SELECT DISTINCT source_schedule_id::text AS value,source_schedule_id::text AS label FROM jobs WHERE source_schedule_id::text ILIKE $1 ESCAPE '\\' ORDER BY value LIMIT 20",
            _ => anyhow::bail!("unsupported_job_search_hint_field"),
        };
        let rows = sqlx::query(sql).bind(pattern).fetch_all(pool).await?;
        rows.iter()
            .map(|row| {
                Ok(JobSearchValue {
                    value: row.try_get("value")?,
                    label: row.try_get("label")?,
                })
            })
            .collect()
    }
}

fn history_row(row: &PgRow) -> Result<JobHistoryView> {
    Ok(JobHistoryView {
        id: row.try_get("id")?,
        actor_id: row.try_get("actor_id")?,
        command_type: row.try_get("command_type")?,
        source_schedule_id: row.try_get("source_schedule_id")?,
        causation_id: row.try_get("causation_id")?,
        schedule_lineage: row.try_get("schedule_lineage")?,
        privileged: row.try_get("privileged")?,
        status: row.try_get("status")?,
        target_count: row.try_get("target_count")?,
        payload_hash: row.try_get("payload_hash")?,
        max_timeout_secs: row.try_get::<i64, _>("max_timeout_secs")?.max(1) as u64,
        created_at: row.try_get("created_at")?,
        completed_at: row.try_get("completed_at")?,
    })
}

mod timestamp_wire {
    use super::*;
    pub(super) fn serialize<S: serde::Serializer>(
        value: &DateTime<Utc>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.to_rfc3339())
    }
    pub(super) fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<DateTime<Utc>, D::Error> {
        let value = String::deserialize(deserializer)?;
        DateTime::parse_from_rfc3339(&value)
            .map(|t| t.with_timezone(&Utc))
            .map_err(serde::de::Error::custom)
    }
}
