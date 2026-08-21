use std::{path::Path, time::Duration};

use anyhow::{bail, ensure, Context, Result};
use sqlx::{migrate::Migrator, PgPool, Row};

const MIGRATION_LOCK_NAMESPACE: i32 = 0x5650_534d;
const MIGRATION_LOCK_RESOURCE: i32 = 18;
const MIGRATION_LOCK_WAIT_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const MIGRATION_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(100);
const IMPORT_CLASS_MIGRATION_VERSION: i64 = 18;
const IMPORT_CLASS_MIGRATION_DESCRIPTION: &str = "traffic counter import class stream index";
const IMPORT_CLASS_MIGRATION_SOURCE: &str =
    include_str!("../../../../migrations/0018_traffic_counter_import_class_stream_index.sql");
const IMPORT_CLASS_INDEX: &str = "traffic_counter_samples_import_class_stream_idx";
const IMPORT_CLASS_INDEX_DEFINITION: &str = "CREATE INDEX traffic_counter_samples_import_class_stream_idx ON public.traffic_counter_samples USING btree (client_id, source_kind, interface, ((sample_source ~~ 'vnstat_import:%'::text)), observed_at)";
const CREATE_IMPORT_CLASS_INDEX_SQL: &str = r#"
    CREATE INDEX CONCURRENTLY traffic_counter_samples_import_class_stream_idx
    ON public.traffic_counter_samples (
        client_id,
        source_kind,
        interface,
        (sample_source LIKE 'vnstat_import:%'),
        observed_at
    )
"#;
const DROP_IMPORT_CLASS_INDEX_SQL: &str =
    "DROP INDEX CONCURRENTLY public.traffic_counter_samples_import_class_stream_idx";

#[derive(Clone, Debug)]
struct ImportClassIndexState {
    relkind: String,
    definition: Option<String>,
    valid: bool,
    ready: bool,
    live: bool,
}

impl ImportClassIndexState {
    fn has_exact_definition(&self) -> bool {
        self.relkind == "i" && self.definition.as_deref() == Some(IMPORT_CLASS_INDEX_DEFINITION)
    }

    fn is_usable(&self) -> bool {
        self.has_exact_definition() && self.valid && self.ready && self.live
    }
}

/// Run the workspace migrations and enforce the catalog contract required by
/// the non-transactional traffic stream index migration.
///
/// PostgreSQL can leave an invalid index behind when a concurrent build is
/// canceled, including in the narrow interval before SQLx records the
/// migration ledger row. Migration 0018 deliberately uses `IF NOT EXISTS` so
/// that a retry can finish the ledger step. This post-migration check then
/// repairs only an exact, migration-owned invalid definition. A same-name
/// object with any other definition fails closed and is never dropped.
pub async fn run_postgres_migrations(pool: &PgPool, migrations_dir: &Path) -> Result<()> {
    let migrator = Migrator::new(migrations_dir).await.with_context(|| {
        format!(
            "failed to load migrations from {}",
            migrations_dir.display()
        )
    })?;
    let required_migrations = migrator
        .iter()
        .filter(|migration| migration.version == IMPORT_CLASS_MIGRATION_VERSION)
        .collect::<Vec<_>>();
    ensure!(
        required_migrations.len() == 1,
        "migration source must contain exactly one migration 0018"
    );
    let required_migration = required_migrations[0];
    ensure!(
        required_migration.no_tx,
        "migration 0018 must remain a no-transaction migration"
    );
    ensure!(
        required_migration.description == IMPORT_CLASS_MIGRATION_DESCRIPTION,
        "migration 0018 description does not match the release contract"
    );
    ensure!(
        required_migration.sql.as_ref() == IMPORT_CLASS_MIGRATION_SOURCE,
        "migration 0018 does not match the source embedded in this binary"
    );
    let expected_description = required_migration.description.to_string();
    let expected_checksum = required_migration.checksum.to_vec();

    let mut connection = pool
        .acquire()
        .await
        .context("failed to acquire the PostgreSQL migration connection")?;
    // A canceled startup future must destroy this dedicated session rather
    // than return a session-level advisory lock to the pool.
    connection.close_on_drop();

    acquire_migration_contract_lock(&mut connection).await?;

    let result = async {
        migrator
            .run(&mut *connection)
            .await
            .context("failed to run PostgreSQL migrations")?;
        ensure_import_class_migration_ledger(
            &mut connection,
            expected_description.as_str(),
            expected_checksum.as_slice(),
        )
        .await?;
        ensure_import_class_index(&mut connection).await
    }
    .await;

    let unlock_result = sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock($1, $2)")
        .bind(MIGRATION_LOCK_NAMESPACE)
        .bind(MIGRATION_LOCK_RESOURCE)
        .fetch_one(&mut *connection)
        .await
        .context("failed to unlock the PostgreSQL migration contract")
        .and_then(|unlocked| {
            ensure!(unlocked, "PostgreSQL migration contract lock was not held");
            Ok(())
        });

    let unlock_failure = unlock_result.as_ref().err().map(ToString::to_string);
    let close_result = connection
        .close()
        .await
        .context("failed to close the dedicated PostgreSQL migration connection");
    let close_failure = close_result.as_ref().err().map(ToString::to_string);

    match result {
        Err(error) => {
            let cleanup_failures = [
                unlock_failure.map(|message| format!("unlock: {message}")),
                close_failure.map(|message| format!("close: {message}")),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
            if cleanup_failures.is_empty() {
                Err(error)
            } else {
                Err(error.context(format!(
                    "PostgreSQL migration session cleanup also failed ({})",
                    cleanup_failures.join("; ")
                )))
            }
        }
        Ok(()) => {
            unlock_result?;
            close_result?;
            Ok(())
        }
    }
}

async fn acquire_migration_contract_lock(connection: &mut sqlx::PgConnection) -> Result<()> {
    tokio::time::timeout(MIGRATION_LOCK_WAIT_TIMEOUT, async {
        loop {
            let acquired = sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock($1, $2)")
                .bind(MIGRATION_LOCK_NAMESPACE)
                .bind(MIGRATION_LOCK_RESOURCE)
                .fetch_one(&mut *connection)
                .await
                .context("failed to try the PostgreSQL migration contract lock")?;
            if acquired {
                return Ok(());
            }
            // A blocking advisory-lock statement owns a virtual transaction
            // while it waits. CREATE INDEX CONCURRENTLY must wait for older
            // virtual transactions, so that shape deadlocks with the lock
            // holder's concurrent index repair. Each unsuccessful try-lock
            // completes its autocommit statement before this asynchronous
            // sleep and therefore leaves no waiter transaction behind.
            tokio::time::sleep(MIGRATION_LOCK_POLL_INTERVAL).await;
        }
    })
    .await
    .context("timed out waiting for the PostgreSQL migration contract lock")?
}

async fn ensure_import_class_migration_ledger(
    connection: &mut sqlx::PgConnection,
    expected_description: &str,
    expected_checksum: &[u8],
) -> Result<()> {
    let row = sqlx::query(
        r#"
        SELECT description, success, checksum
        FROM public._sqlx_migrations
        WHERE version = $1
        "#,
    )
    .bind(IMPORT_CLASS_MIGRATION_VERSION)
    .fetch_optional(&mut *connection)
    .await
    .context("failed to inspect migration 0018 ledger state")?
    .context("migration 0018 is absent from the PostgreSQL migration ledger")?;
    let description: String = row.try_get("description")?;
    let success: bool = row.try_get("success")?;
    let checksum: Vec<u8> = row.try_get("checksum")?;
    ensure!(success, "migration 0018 ledger row is not successful");
    ensure!(
        description == expected_description,
        "migration 0018 ledger description mismatch"
    );
    ensure!(
        checksum == expected_checksum,
        "migration 0018 ledger checksum mismatch"
    );
    Ok(())
}

async fn ensure_import_class_index(connection: &mut sqlx::PgConnection) -> Result<()> {
    let initial = load_import_class_index_state(connection).await?;
    let needs_create = match initial {
        None => true,
        Some(state) if state.is_usable() => return Ok(()),
        Some(state) if state.has_exact_definition() => {
            sqlx::raw_sql(DROP_IMPORT_CLASS_INDEX_SQL)
                .execute(&mut *connection)
                .await
                .context("failed to drop the exact invalid traffic import class index")?;
            true
        }
        Some(state) => {
            bail!(
                "PostgreSQL relation public.{IMPORT_CLASS_INDEX} has an unexpected definition (relkind {}, definition {:?}); refusing to replace it",
                state.relkind,
                state.definition
            );
        }
    };

    if needs_create {
        sqlx::raw_sql(CREATE_IMPORT_CLASS_INDEX_SQL)
            .execute(&mut *connection)
            .await
            .context("failed to create the traffic import class stream index")?;
    }

    let repaired = load_import_class_index_state(connection)
        .await?
        .context("traffic import class stream index is missing after repair")?;
    ensure!(
        repaired.is_usable(),
        "traffic import class stream index failed its exact post-repair contract: {repaired:?}"
    );
    Ok(())
}

async fn load_import_class_index_state(
    connection: &mut sqlx::PgConnection,
) -> Result<Option<ImportClassIndexState>> {
    let row = sqlx::query(
        r#"
        SELECT
            relation.relkind::text AS relkind,
            CASE
                WHEN relation.relkind = 'i'
                THEN pg_get_indexdef(relation.oid)
                ELSE NULL
            END AS definition,
            COALESCE(index.indisvalid, FALSE) AS valid,
            COALESCE(index.indisready, FALSE) AS ready,
            COALESCE(index.indislive, FALSE) AS live
        FROM pg_catalog.pg_class relation
        JOIN pg_catalog.pg_namespace namespace
          ON namespace.oid = relation.relnamespace
        LEFT JOIN pg_catalog.pg_index index
          ON index.indexrelid = relation.oid
        WHERE namespace.nspname = 'public'
          AND relation.relname = $1
        "#,
    )
    .bind(IMPORT_CLASS_INDEX)
    .fetch_optional(&mut *connection)
    .await
    .context("failed to inspect the traffic import class stream index")?;

    row.map(|row| {
        Ok::<_, sqlx::Error>(ImportClassIndexState {
            relkind: row.try_get("relkind")?,
            definition: row.try_get("definition")?,
            valid: row.try_get("valid")?,
            ready: row.try_get("ready")?,
            live: row.try_get("live")?,
        })
    })
    .transpose()
    .context("failed to decode the traffic import class stream index contract")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_index_contract_requires_all_catalog_readiness_bits() {
        let exact = ImportClassIndexState {
            relkind: "i".to_string(),
            definition: Some(IMPORT_CLASS_INDEX_DEFINITION.to_string()),
            valid: true,
            ready: true,
            live: true,
        };
        assert!(exact.has_exact_definition());
        assert!(exact.is_usable());

        for state in [
            ImportClassIndexState {
                valid: false,
                ..exact.clone()
            },
            ImportClassIndexState {
                ready: false,
                ..exact.clone()
            },
            ImportClassIndexState {
                live: false,
                ..exact.clone()
            },
        ] {
            assert!(state.has_exact_definition());
            assert!(!state.is_usable());
        }

        let wrong = ImportClassIndexState {
            definition: Some(format!("{IMPORT_CLASS_INDEX_DEFINITION} WHERE true")),
            ..exact
        };
        assert!(!wrong.has_exact_definition());
        assert!(!wrong.is_usable());
    }
}
