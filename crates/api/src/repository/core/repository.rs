use std::{path::Path, str::FromStr};

use anyhow::{ensure, Context, Result};
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    Connection, PgConnection, PgPool,
};
use tracing::info;

const SQLX_METADATA_SCHEMA: &str = "vpsman_internal";
const SQLX_METADATA_SCHEMA_LOCK_KEY: i64 = 0x5650_534d_5351_4c58;
// API requests are latency-bounded and repeatedly execute structurally large
// telemetry statements for small result sets. Child API pools clone these
// options; the dedicated migration connection deliberately uses the base URL.
pub(crate) const API_POSTGRES_SESSION_OPTIONS: [(&str, &str); 2] =
    [("search_path", "public"), ("jit", "off")];

#[derive(Clone)]
pub(crate) enum Repository {
    Postgres(PgPool),
}

impl Repository {
    pub(crate) async fn connect(
        postgres_url: Option<&str>,
        migrations_dir: &std::path::Path,
    ) -> Result<Self> {
        let Some(postgres_url) = postgres_url else {
            anyhow::bail!("VPSMAN_POSTGRES_URL is required");
        };

        let max_connections = std::env::var("VPSMAN_API_DB_MAX_CONNECTIONS")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(32)
            .clamp(1, 256);
        let connect_options = PgConnectOptions::from_str(postgres_url)
            .context("failed to parse the PostgreSQL connection URL")?;
        migrate_postgres_database(&connect_options, migrations_dir).await?;
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .connect_with(
                connect_options
                    .clone()
                    .options(API_POSTGRES_SESSION_OPTIONS),
            )
            .await
            .context("failed to connect to PostgreSQL")?;
        let repository = Self::Postgres(pool);
        info!("api using PostgreSQL repository");
        Ok(repository)
    }
}

pub(crate) async fn migrate_postgres_database(
    connect_options: &PgConnectOptions,
    migrations_dir: &Path,
) -> Result<()> {
    let mut migration_connection = PgConnection::connect_with(connect_options)
        .await
        .context("failed to open the dedicated PostgreSQL migration connection")?;

    // This transaction-scoped owner exists only to make first creation of the
    // SQLx metadata schema deterministic when API and worker start together.
    // SQLx owns its separate database migration lock after this transaction.
    let mut schema_transaction = migration_connection
        .begin()
        .await
        .context("failed to begin the SQLx metadata schema transaction")?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(SQLX_METADATA_SCHEMA_LOCK_KEY)
        .execute(&mut *schema_transaction)
        .await
        .context("failed to acquire the SQLx metadata schema owner")?;
    sqlx::query("CREATE SCHEMA IF NOT EXISTS vpsman_internal AUTHORIZATION CURRENT_USER")
        .execute(&mut *schema_transaction)
        .await
        .context("failed to provision the SQLx metadata schema")?;
    schema_transaction
        .commit()
        .await
        .context("failed to commit the SQLx metadata schema transaction")?;

    sqlx::query("SET search_path TO vpsman_internal, public")
        .execute(&mut migration_connection)
        .await
        .context("failed to select the private SQLx metadata schema")?;
    let (current_schema, owned_by_current_user): (String, bool) = sqlx::query_as(
        r#"
        SELECT
            current_schema(),
            namespace.nspowner = (
                SELECT role.oid FROM pg_roles role WHERE role.rolname = current_user
            )
        FROM pg_namespace namespace
        WHERE namespace.nspname = $1
        "#,
    )
    .bind(SQLX_METADATA_SCHEMA)
    .fetch_one(&mut migration_connection)
    .await
    .context("failed to verify the private SQLx metadata schema")?;
    ensure!(
        current_schema == SQLX_METADATA_SCHEMA && owned_by_current_user,
        "private SQLx metadata schema is not the current role-owned schema"
    );

    sqlx::migrate::Migrator::new(migrations_dir)
        .await
        .with_context(|| {
            format!(
                "failed to load migrations from {}",
                migrations_dir.display()
            )
        })?
        .run(&mut migration_connection)
        .await
        .context("failed to run PostgreSQL migrations")?;
    migration_connection
        .close()
        .await
        .context("failed to close the dedicated PostgreSQL migration connection")?;
    Ok(())
}
