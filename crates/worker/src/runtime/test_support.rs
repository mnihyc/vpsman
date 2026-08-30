use std::{path::Path, str::FromStr};

use anyhow::Result;
use sqlx::{
    postgres::{PgConnectOptions, PgListener, PgPoolOptions},
    PgPool,
};
use uuid::Uuid;

pub(crate) struct PgWorkerTestDb {
    pub(crate) pool: PgPool,
    admin_pool: PgPool,
    connect_options: PgConnectOptions,
    db_name: String,
}

impl PgWorkerTestDb {
    pub(crate) async fn maybe_new() -> Option<Self> {
        let base_url = match std::env::var("VPSMAN_TEST_POSTGRES_URL") {
            Ok(value) if !value.trim().is_empty() => value,
            _ => {
                eprintln!("skipping worker Postgres test: VPSMAN_TEST_POSTGRES_URL is unset");
                return None;
            }
        };
        Some(
            Self::new(&base_url)
                .await
                .expect("failed to create worker test database"),
        )
    }

    async fn new(base_url: &str) -> Result<Self> {
        let base_options = PgConnectOptions::from_str(base_url)?;
        let admin_pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(base_options.clone().database("postgres"))
            .await?;
        let db_name = format!("vpsman_worker_{}", Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE DATABASE {}", quote_ident(&db_name)))
            .execute(&admin_pool)
            .await?;
        let database_options = base_options.database(&db_name);
        crate::migrate_postgres_database(&database_options, &workspace_migrations_dir()).await?;
        let connect_options = database_options.options([("search_path", "public")]);
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect_with(connect_options.clone())
            .await?;
        Ok(Self {
            pool,
            admin_pool,
            connect_options,
            db_name,
        })
    }

    pub(crate) async fn additional_pool(&self, max_connections: u32) -> Result<PgPool> {
        Ok(PgPoolOptions::new()
            .min_connections(0)
            .max_connections(max_connections)
            .connect_with(self.connect_options.clone())
            .await?)
    }

    pub(crate) async fn telemetry_retention_pool(&self) -> Result<PgPool> {
        Ok(crate::telemetry_retention_pool_options()
            .connect_with(self.connect_options.clone())
            .await?)
    }

    pub(crate) async fn notification_listener(&self) -> Result<PgListener> {
        Ok(PgListener::connect_with(&self.pool).await?)
    }

    pub(crate) async fn cleanup(self) {
        let Self {
            pool,
            admin_pool,
            connect_options: _,
            db_name,
        } = self;
        pool.close().await;
        let _ = sqlx::query(
            r#"
            SELECT pg_terminate_backend(pid)
            FROM pg_stat_activity
            WHERE datname = $1
              AND pid <> pg_backend_pid()
            "#,
        )
        .bind(&db_name)
        .execute(&admin_pool)
        .await;
        let _ = sqlx::query(&format!(
            "DROP DATABASE IF EXISTS {}",
            quote_ident(&db_name)
        ))
        .execute(&admin_pool)
        .await;
        admin_pool.close().await;
    }
}

fn quote_ident(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn workspace_migrations_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("migrations")
}
