use std::sync::{
    atomic::{AtomicBool, Ordering},
    OnceLock,
};

use anyhow::{Context, Result};
use sqlx::{
    postgres::{PgListener, PgPoolOptions},
    Connection, PgConnection, Postgres, Row, Transaction,
};
use tokio::sync::{watch, Notify};
use uuid::Uuid;

use crate::{
    repository::Repository,
    repository_ingest::materialize_combined_telemetry_policy_baseline_sample_in_tx,
    repository_key_lifecycle::lock_postgres_definition_lifecycles_in_tx,
};

// Start conservatively until the supervised consumer has inspected durable
// state once.  Afterwards the normal inactive/effective state makes telemetry
// acceptance and projection pay only an atomic load.
static ACTIVATION_MAY_BE_PENDING: AtomicBool = AtomicBool::new(true);
static ACTIVATION_WAKE: OnceLock<Notify> = OnceLock::new();

pub(crate) const TELEMETRY_POLICY_ACTIVATION_CHANNEL: &str = "vpsman_telemetry_policy_activation";
pub(crate) const TELEMETRY_POLICY_ACTIVATION_SEED_OWNER: &str =
    "vpsman:telemetry-policy-activation-seed";

fn activation_wake() -> &'static Notify {
    ACTIVATION_WAKE.get_or_init(Notify::new)
}

/// Transfers ownership from a committed definition/ingest mutation to the
/// supervised durable consumer.  Notify is only an accelerator; PostgreSQL is
/// the authority and the consumer always inspects it once at startup.
pub(crate) fn wake_telemetry_policy_activation() {
    ACTIVATION_MAY_BE_PENDING.store(true, Ordering::Release);
    activation_wake().notify_one();
}

/// Closes the commit-to-notify race for a policy transition.  Call this after
/// the durable state mutation but before committing; a rollback causes only a
/// temporary conservative SQL check, never an incorrect effective boundary.
pub(crate) fn mark_telemetry_policy_activation_may_be_pending() {
    ACTIVATION_MAY_BE_PENDING.store(true, Ordering::Release);
}

/// A projected suffix can make an exact sample ready.  The steady state pays
/// only this atomic load; no policy query or background poll is added while no
/// activation generation is pending.
pub(crate) fn wake_telemetry_policy_activation_after_projection() {
    if ACTIVATION_MAY_BE_PENDING.load(Ordering::Acquire) {
        activation_wake().notify_one();
    }
}

/// Linearizes a canonical acceptance against the short effective-boundary
/// update. Shared row locks are mutually compatible, so unrelated clients
/// never serialize; only the rare finalizer waits for already-running
/// acceptance transactions to publish their exact work rows.
pub(crate) async fn claim_telemetry_policy_activation_generation_in_tx(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<Option<i64>> {
    #[cfg(not(test))]
    if !ACTIVATION_MAY_BE_PENDING.load(Ordering::Acquire) {
        return Ok(None);
    }
    sqlx::query_scalar::<_, i64>(
        r#"
        SELECT generation
        FROM alert_telemetry_policy_activation
        WHERE singleton
          AND desired_enabled
          AND effective_generation IS NULL
        FOR SHARE
        "#,
    )
    .fetch_optional(&mut **tx)
    .await
    .context("failed to claim telemetry policy activation generation")
}

/// Publishes the exact accepted sample under the generation already shared-
/// locked by this transaction.
pub(crate) async fn enqueue_telemetry_policy_activation_sample_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    generation: i64,
    client_id: &str,
    accepted_seq: i64,
    sample_id: Uuid,
) -> Result<bool> {
    let queued = sqlx::query_scalar::<_, i64>(
        r#"
        INSERT INTO alert_telemetry_policy_activation_work (
            activation_generation, client_id, target_accepted_seq,
            target_sample_id
        ) VALUES ($1, $2, $3, $4)
        ON CONFLICT (activation_generation, client_id) DO UPDATE SET
            target_accepted_seq = EXCLUDED.target_accepted_seq,
            target_sample_id = EXCLUDED.target_sample_id,
            work_revision = alert_telemetry_policy_activation_work.work_revision + 1,
            claim_token = NULL,
            claim_revision = NULL,
            updated_at = clock_timestamp()
        WHERE EXCLUDED.target_accepted_seq
                > alert_telemetry_policy_activation_work.target_accepted_seq
        RETURNING work_revision
        "#,
    )
    .bind(generation)
    .bind(client_id)
    .bind(accepted_seq)
    .bind(sample_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(queued.is_some())
}

/// Reconciles the durable desired/effective state after a telemetry-policy
/// definition transaction has reached its final database-only step.  Callers
/// already own the namespaced telemetry definition advisory lock.
pub(crate) async fn reconcile_telemetry_policy_activation_request_in_tx(
    tx: &mut Transaction<'_, Postgres>,
) -> Result<bool> {
    let any_enabled: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM policy_rules rule
            JOIN policy_groups policy ON policy.id=rule.group_id
            WHERE policy.enabled
              AND rule.enabled
              AND rule.evidence_source='telemetry.combined'
        )
        "#,
    )
    .fetch_one(&mut **tx)
    .await?;
    let state = sqlx::query(
        r#"
        SELECT generation, desired_enabled
        FROM alert_telemetry_policy_activation
        WHERE singleton
        FOR UPDATE
        "#,
    )
    .fetch_one(&mut **tx)
    .await?;
    let generation: i64 = state.try_get("generation")?;
    let desired_enabled: bool = state.try_get("desired_enabled")?;
    if any_enabled == desired_enabled {
        return Ok(false);
    }

    let transitioned_generation = if any_enabled {
        sqlx::query_scalar::<_, i64>(
            r#"
            UPDATE alert_telemetry_policy_activation
            SET generation = generation + 1,
                desired_enabled = TRUE,
                seeded_generation = NULL,
                effective_generation = NULL,
                boundary_evidence_seq = 0,
                requested_at = clock_timestamp(),
                seeded_at = NULL,
                effective_at = NULL,
                updated_at = clock_timestamp()
            WHERE singleton AND generation=$1 AND NOT desired_enabled
            RETURNING generation
            "#,
        )
        .bind(generation)
        .fetch_optional(&mut **tx)
        .await?
    } else {
        sqlx::query_scalar::<_, i64>(
            r#"
            UPDATE alert_telemetry_policy_activation
            SET generation = generation + 1,
                desired_enabled = FALSE,
                seeded_generation = NULL,
                effective_generation = NULL,
                boundary_evidence_seq = 0,
                requested_at = NULL,
                seeded_at = NULL,
                effective_at = NULL,
                updated_at = clock_timestamp()
            WHERE singleton AND generation=$1 AND desired_enabled
            RETURNING generation
            "#,
        )
        .bind(generation)
        .fetch_optional(&mut **tx)
        .await?
    };
    let transitioned_generation = transitioned_generation
        .context("telemetry policy activation transition lost its singleton owner")?;

    // PostgreSQL delivers NOTIFY only when this transaction commits.  Every
    // API replica therefore observes the same durable desired-generation
    // transition; an unchanged definition returns above and emits nothing.
    sqlx::query("SELECT pg_notify($1, $2)")
        .bind(TELEMETRY_POLICY_ACTIVATION_CHANNEL)
        .bind(transitioned_generation.to_string())
        .execute(&mut **tx)
        .await?;
    Ok(true)
}

#[derive(Clone, Copy, Debug, Default)]
struct ActivationStep {
    did_work: bool,
    pending: bool,
}

/// Main-owned consumer.  There is no retry timer or permanent poll: a failed
/// database operation exits to its supervisor, while durable work is woken by
/// definition, ingest, and projection handoffs and is inspected at startup
/// and after every transparent listener reconnect.
pub(crate) async fn run_telemetry_policy_activation_consumer(
    repo: Repository,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let Repository::Postgres(pool) = repo;
    // Register before the first durable inspection.  This closes both sides
    // of the startup race: an earlier transition is found in the table and a
    // later transition is retained by this listener.
    let listener_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy_with((*pool.connect_options()).clone());
    let mut listener = PgListener::connect_with(&listener_pool)
        .await
        .context("failed to connect telemetry policy activation listener")?;
    listener
        .listen(TELEMETRY_POLICY_ACTIVATION_CHANNEL)
        .await
        .context("failed to register telemetry policy activation listener")?;
    loop {
        if *shutdown.borrow() {
            return Ok(());
        }
        let step = drive_telemetry_policy_activation_once(&pool).await?;
        ACTIVATION_MAY_BE_PENDING.store(step.pending, Ordering::Release);
        if step.did_work {
            continue;
        }
        tokio::select! {
            _ = activation_wake().notified() => {}
            notification = listener.try_recv() => {
                // Some(...) is a committed generation transition.  None is
                // SQLx's explicit lost-connection/re-subscribed signal.  Both
                // require the same durable inspection, so no notification
                // can be lost across a reconnect and no healthy-state poll is
                // needed.
                let _ = notification.context(
                    "telemetry policy activation listener receive failed",
                )?;
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return Ok(());
                }
            }
        }
    }
}

async fn drive_telemetry_policy_activation_once(pool: &sqlx::PgPool) -> Result<ActivationStep> {
    let state = sqlx::query(
        r#"
        SELECT generation, desired_enabled,
               seeded_generation = generation AS seeded,
               effective_generation = generation AS effective
        FROM alert_telemetry_policy_activation
        WHERE singleton
        "#,
    )
    .fetch_one(pool)
    .await?;
    let generation: i64 = state.try_get("generation")?;
    let desired_enabled: bool = state.try_get("desired_enabled")?;
    let seeded: Option<bool> = state.try_get("seeded")?;
    let effective: Option<bool> = state.try_get("effective")?;
    let pending = desired_enabled && effective != Some(true);
    if !pending {
        return Ok(ActivationStep {
            did_work: remove_stale_telemetry_policy_activation_work(pool, generation, false)
                .await?
                > 0,
            pending: false,
        });
    }

    if seeded != Some(true) {
        if remove_stale_telemetry_policy_activation_work(pool, generation, true).await? > 0 {
            return Ok(ActivationStep {
                did_work: true,
                pending: true,
            });
        }
        let seeded = seed_telemetry_policy_activation_generation(pool, generation).await?;
        return Ok(ActivationStep {
            did_work: seeded,
            pending: true,
        });
    }

    if consume_one_telemetry_policy_activation_sample(pool, generation).await? {
        return Ok(ActivationStep {
            did_work: true,
            pending: true,
        });
    }

    let finalized = finalize_telemetry_policy_activation_generation(pool, generation).await?;
    Ok(ActivationStep {
        did_work: finalized,
        pending: !finalized,
    })
}

async fn remove_stale_telemetry_policy_activation_work(
    pool: &sqlx::PgPool,
    generation: i64,
    retain_current_generation: bool,
) -> Result<u64> {
    Ok(sqlx::query(
        r#"
        DELETE FROM alert_telemetry_policy_activation_work work
        USING alert_telemetry_policy_activation activation
        WHERE activation.singleton
          AND activation.generation=$1
          AND (NOT $2::boolean OR work.activation_generation<>$1)
        "#,
    )
    .bind(generation)
    .bind(retain_current_generation)
    .execute(pool)
    .await?
    .rows_affected())
}

#[cfg(test)]
pub(crate) async fn settle_telemetry_policy_activation_for_test(repo: &Repository) -> Result<bool> {
    let Repository::Postgres(pool) = repo;
    loop {
        let step = drive_telemetry_policy_activation_once(pool).await?;
        if !step.did_work {
            return Ok(!step.pending);
        }
    }
}

/// Direct-SQL policy fixtures bypass the repository mutation that normally
/// advances the desired activation generation. Re-enter that exact definition
/// owner, publish the desired boundary, then drain the real activation
/// consumer; tests must not make telemetry policies effective by mutating the
/// singleton directly.
#[cfg(test)]
pub(crate) async fn reconcile_and_settle_telemetry_policy_activation_for_test(
    repo: &Repository,
) -> Result<bool> {
    let Repository::Postgres(pool) = repo;
    let mut tx = pool.begin().await?;
    lock_postgres_definition_lifecycles_in_tx(
        &mut tx,
        &["alert-policy-telemetry-consumer".to_string()],
    )
    .await?;
    reconcile_telemetry_policy_activation_request_in_tx(&mut tx).await?;
    tx.commit().await?;
    settle_telemetry_policy_activation_for_test(repo).await
}

/// Seeds exact immutable sample identities, commits that fleet-shaped work,
/// and only then marks the scalar generation. Acceptance orders singleton ->
/// exact work; keeping the two phases in separate transactions prevents the
/// inverse work -> singleton edge without holding a fleet-wide acceptance
/// fence during population.
async fn seed_telemetry_policy_activation_generation(
    pool: &sqlx::PgPool,
    generation: i64,
) -> Result<bool> {
    // A generation has exactly one fleet-population owner across API
    // replicas.  This rare unpooled connection cannot leak a session lock
    // into the application pool: normal completion explicitly unlocks and
    // closes it, while cancellation drops its socket and PostgreSQL releases
    // the session owner.  The owner intentionally spans the population commit
    // and scalar marker commit; telemetry acceptance never acquires it.
    let mut seed_owner = PgConnection::connect_with(pool.connect_options().as_ref())
        .await
        .context("failed to acquire telemetry policy activation seed connection")?;
    sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))")
        .bind(TELEMETRY_POLICY_ACTIVATION_SEED_OWNER)
        .execute(&mut seed_owner)
        .await
        .context("failed to acquire telemetry policy activation seed owner")?;

    // Another replica may have completed (or obsoleted) this generation while
    // we waited for the owner.  Reinspect before any fleet-shaped write.  A
    // true return asks the caller to re-drive immediately from current state.
    let current = sqlx::query_as::<_, (bool, Option<bool>, Option<bool>)>(
        r#"
        SELECT desired_enabled,
               seeded_generation=generation AS seeded,
               effective_generation=generation AS effective
        FROM alert_telemetry_policy_activation
        WHERE singleton AND generation=$1
        "#,
    )
    .bind(generation)
    .fetch_optional(&mut seed_owner)
    .await?;
    if let Some((true, seeded, effective)) = current {
        if seeded != Some(true) && effective != Some(true) {
            populate_telemetry_policy_activation_generation(&mut seed_owner, generation).await?;
            // A concurrent definition transition may obsolete this generation
            // after the recheck.  The exact conditional marker then writes
            // nothing; either way the caller immediately re-drives from the
            // authoritative singleton.
            mark_telemetry_policy_activation_generation_seeded(&mut seed_owner, generation).await?;
        }
    }
    let unlocked: bool = sqlx::query_scalar("SELECT pg_advisory_unlock(hashtextextended($1, 0))")
        .bind(TELEMETRY_POLICY_ACTIVATION_SEED_OWNER)
        .fetch_one(&mut seed_owner)
        .await
        .context("failed to release telemetry policy activation seed owner")?;
    anyhow::ensure!(unlocked, "telemetry policy activation seed owner was lost");
    seed_owner
        .close()
        .await
        .context("failed to close telemetry policy activation seed connection")?;
    Ok(true)
}

#[cfg(test)]
pub(crate) async fn seed_telemetry_policy_activation_generation_for_test(
    repo: &Repository,
    generation: i64,
) -> Result<bool> {
    let Repository::Postgres(pool) = repo;
    seed_telemetry_policy_activation_generation(pool, generation).await
}

async fn populate_telemetry_policy_activation_generation(
    connection: &mut sqlx::PgConnection,
    generation: i64,
) -> Result<()> {
    let mut population_tx = connection.begin().await?;
    sqlx::query(
        r#"
        INSERT INTO alert_telemetry_policy_activation_work (
            activation_generation, client_id, target_accepted_seq,
            target_sample_id
        )
        SELECT $1, head.client_id, head.accepted_seq, sample.id
        FROM telemetry_projection_heads head
        JOIN telemetry_samples sample
          ON sample.client_id=head.client_id
         AND sample.accepted_seq=head.accepted_seq
        WHERE head.accepted_seq>0
        ORDER BY head.client_id COLLATE "C"
        ON CONFLICT (activation_generation, client_id) DO UPDATE SET
            target_accepted_seq = EXCLUDED.target_accepted_seq,
            target_sample_id = EXCLUDED.target_sample_id,
            work_revision = alert_telemetry_policy_activation_work.work_revision + 1,
            claim_token = NULL,
            claim_revision = NULL,
            updated_at = clock_timestamp()
        WHERE EXCLUDED.target_accepted_seq
                > alert_telemetry_policy_activation_work.target_accepted_seq
        "#,
    )
    .bind(generation)
    .execute(&mut *population_tx)
    .await?;
    population_tx.commit().await?;
    Ok(())
}

async fn mark_telemetry_policy_activation_generation_seeded(
    connection: &mut sqlx::PgConnection,
    generation: i64,
) -> Result<bool> {
    let mut marker_tx = connection.begin().await?;
    let marked = sqlx::query(
        r#"
        UPDATE alert_telemetry_policy_activation
        SET seeded_generation=generation,
            seeded_at=clock_timestamp(),
            updated_at=clock_timestamp()
        WHERE singleton
          AND generation=$1
          AND desired_enabled
          AND effective_generation IS NULL
          AND seeded_generation IS DISTINCT FROM generation
        "#,
    )
    .bind(generation)
    .execute(&mut *marker_tx)
    .await?
    .rows_affected()
        == 1;
    marker_tx.commit().await?;
    Ok(marked)
}

async fn consume_one_telemetry_policy_activation_sample(
    pool: &sqlx::PgPool,
    generation: i64,
) -> Result<bool> {
    let mut tx = pool.begin().await?;
    let claim_token = Uuid::new_v4();
    let claim = sqlx::query(
        r#"
        WITH candidate AS (
            SELECT work.activation_generation, work.client_id,
                   work.target_accepted_seq, work.target_sample_id,
                   work.work_revision
            FROM alert_telemetry_policy_activation_work work
            JOIN telemetry_projection_heads head ON head.client_id=work.client_id
            WHERE work.activation_generation=$1
              AND head.projected_seq>=work.target_accepted_seq
            ORDER BY work.client_id
            FOR UPDATE OF work SKIP LOCKED
            LIMIT 1
        )
        UPDATE alert_telemetry_policy_activation_work work
        SET claim_token=$2,
            claim_revision=candidate.work_revision,
            updated_at=clock_timestamp()
        FROM candidate
        WHERE work.activation_generation=candidate.activation_generation
          AND work.client_id=candidate.client_id
          AND work.work_revision=candidate.work_revision
        RETURNING work.client_id, work.target_accepted_seq,
                  work.target_sample_id, work.work_revision
        "#,
    )
    .bind(generation)
    .bind(claim_token)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(claim) = claim else {
        tx.rollback().await?;
        return Ok(false);
    };
    let client_id: String = claim.try_get("client_id")?;
    let target_accepted_seq: i64 = claim.try_get("target_accepted_seq")?;
    let target_sample_id: Uuid = claim.try_get("target_sample_id")?;
    let work_revision: i64 = claim.try_get("work_revision")?;

    materialize_combined_telemetry_policy_baseline_sample_in_tx(
        &mut tx,
        &client_id,
        target_accepted_seq,
        target_sample_id,
    )
    .await?;
    let acknowledged = sqlx::query(
        r#"
        DELETE FROM alert_telemetry_policy_activation_work
        WHERE activation_generation=$1
          AND client_id=$2
          AND target_accepted_seq=$3
          AND target_sample_id=$4
          AND work_revision=$5
          AND claim_token=$6
          AND claim_revision=$5
        "#,
    )
    .bind(generation)
    .bind(&client_id)
    .bind(target_accepted_seq)
    .bind(target_sample_id)
    .bind(work_revision)
    .bind(claim_token)
    .execute(&mut *tx)
    .await?;
    anyhow::ensure!(
        acknowledged.rows_affected() == 1,
        "telemetry policy activation claim fence changed"
    );
    tx.commit().await?;
    Ok(true)
}

/// Publishes only a scalar evidence boundary and effective generation while
/// holding the global definition arm.  The fleet/sample work has already been
/// acknowledged, so this transaction never reconstructs traffic or scans the
/// client fleet.
async fn finalize_telemetry_policy_activation_generation(
    pool: &sqlx::PgPool,
    generation: i64,
) -> Result<bool> {
    let mut tx = pool.begin().await?;
    lock_postgres_definition_lifecycles_in_tx(
        &mut tx,
        &["alert-policy-telemetry-consumer".to_string()],
    )
    .await?;
    let state = sqlx::query(
        r#"
        SELECT desired_enabled,
               seeded_generation=generation AS seeded,
               effective_generation=generation AS effective
        FROM alert_telemetry_policy_activation
        WHERE singleton AND generation=$1
        FOR UPDATE
        "#,
    )
    .bind(generation)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(state) = state else {
        tx.rollback().await?;
        return Ok(false);
    };
    if !state.try_get::<bool, _>("desired_enabled")?
        || state.try_get::<Option<bool>, _>("seeded")? != Some(true)
        || state.try_get::<Option<bool>, _>("effective")? == Some(true)
    {
        tx.rollback().await?;
        return Ok(false);
    }
    let outstanding: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM alert_telemetry_policy_activation_work
            WHERE activation_generation=$1
        )
        "#,
    )
    .bind(generation)
    .fetch_one(&mut *tx)
    .await?;
    if outstanding {
        tx.rollback().await?;
        return Ok(false);
    }

    let boundary: i64 =
        sqlx::query_scalar("SELECT COALESCE(max(evidence_seq), 0) FROM alert_policy_evidence")
            .fetch_one(&mut *tx)
            .await?;
    sqlx::query(
        r#"
        UPDATE policy_rules rule
        SET armed_after_evidence_seq=GREATEST(rule.armed_after_evidence_seq, $1),
            armed_at=clock_timestamp(),
            updated_at=clock_timestamp()
        FROM policy_groups policy
        WHERE policy.id=rule.group_id
          AND policy.enabled
          AND rule.enabled
          AND rule.evidence_source='telemetry.combined'
        "#,
    )
    .bind(boundary)
    .execute(&mut *tx)
    .await?;
    let finalized = sqlx::query(
        r#"
        UPDATE alert_telemetry_policy_activation
        SET effective_generation=generation,
            boundary_evidence_seq=$2,
            effective_at=clock_timestamp(),
            updated_at=clock_timestamp()
        WHERE singleton
          AND generation=$1
          AND desired_enabled
          AND seeded_generation=generation
          AND effective_generation IS NULL
        "#,
    )
    .bind(generation)
    .bind(boundary)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    tx.commit().await?;
    Ok(finalized)
}

#[cfg(test)]
mod tests {
    #[test]
    fn activation_schema_owns_exact_samples_and_has_no_lease_or_poll_threshold() {
        let schema = include_str!("../../../../../migrations/0012_alert_lifecycle.sql");
        assert!(schema.contains("PRIMARY KEY (\n        activation_generation, client_id\n    )"));
        assert!(schema.contains(
            "FOREIGN KEY (target_sample_id) REFERENCES public.telemetry_samples(id) ON DELETE RESTRICT"
        ));
        assert!(!schema.contains("alert_telemetry_policy_activation_work_lease"));

        let source = include_str!("repository_telemetry_policy_activation.rs");
        let production = source
            .split_once("#[cfg(test)]\nmod tests")
            .expect("activation production boundary")
            .0;
        assert!(production.contains("FOR UPDATE OF work SKIP LOCKED"));
        assert!(production.contains("AND claim_revision=$5"));
        assert!(production.contains("FOR SHARE"));
        assert!(production.contains("ORDER BY head.client_id COLLATE \"C\""));
        assert!(production.contains("connect_lazy_with((*pool.connect_options()).clone())"));
        assert!(production.contains("PgListener::connect_with(&listener_pool)"));
        assert!(production.contains(".listen(TELEMETRY_POLICY_ACTIVATION_CHANNEL)"));
        assert!(production.contains("listener.try_recv()"));
        assert!(production.contains("SELECT pg_notify($1, $2)"));
        assert!(production.contains("SELECT pg_advisory_lock(hashtextextended($1, 0))"));
        assert!(production.contains("PgConnection::connect_with(pool.connect_options().as_ref())"));
        assert!(production.contains("SELECT pg_advisory_unlock(hashtextextended($1, 0))"));
        let seed_owner = production
            .find("SELECT pg_advisory_lock(hashtextextended($1, 0))")
            .expect("cross-replica seed owner");
        let seed_recheck = production
            .find("SELECT desired_enabled,\n               seeded_generation=generation AS seeded")
            .expect("post-owner generation recheck");
        let population = production
            .find("populate_telemetry_policy_activation_generation(&mut seed_owner")
            .expect("fleet population");
        assert!(seed_owner < seed_recheck && seed_recheck < population);
        let population_commit = production
            .find("population_tx.commit().await?")
            .expect("fleet population commit");
        let marker_begin = production
            .find("let mut marker_tx = connection.begin().await?")
            .expect("scalar marker transaction");
        assert!(population_commit < marker_begin);
        let seed_marker = production
            .find("mark_telemetry_policy_activation_generation_seeded(&mut seed_owner")
            .expect("seed marker publication");
        let seed_unlock = production
            .find("SELECT pg_advisory_unlock(hashtextextended($1, 0))")
            .expect("seed owner release");
        assert!(seed_marker < seed_unlock);
        assert!(!production.contains("tokio::time::interval"));
        assert!(!production.contains("sleep("));
    }
}
