use std::{path::Path, str::FromStr};

use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    PgPool, Row,
};
use uuid::Uuid;
use vpsman_common::{
    AgentCapabilitySnapshot, AgentHello, AgentUpdateHeartbeat, CommandOutput,
    GatewayAgentHelloIngest, JobCommand, OutputStream,
};
use vpsman_server_core::{TARGET_STATUS_AGENT_LOST, TARGET_STATUS_COMPLETED};

use crate::{
    model::{BootstrapOperatorRequest, JobOutputView, LoginRequest},
    repository::Repository,
    repository_job_outputs::{JobOutputPersistConfig, JobOutputWriteResult},
};

struct PgReliabilityTestDb {
    repo: Repository,
    pool: PgPool,
    admin_pool: PgPool,
    db_name: String,
}

impl PgReliabilityTestDb {
    async fn maybe_new() -> Option<Self> {
        let base_url = match std::env::var("VPSMAN_TEST_POSTGRES_URL") {
            Ok(value) if !value.trim().is_empty() => value,
            _ => {
                eprintln!("skipping Postgres reliability test: VPSMAN_TEST_POSTGRES_URL is unset");
                return None;
            }
        };
        Some(
            Self::new(&base_url)
                .await
                .expect("failed to create Postgres reliability test database"),
        )
    }

    async fn new(base_url: &str) -> anyhow::Result<Self> {
        let base_options = PgConnectOptions::from_str(base_url)?;
        let admin_pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(base_options.clone().database("postgres"))
            .await?;
        let db_name = format!("vpsman_reliability_{}", Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE DATABASE {}", quote_ident(&db_name)))
            .execute(&admin_pool)
            .await?;
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect_with(base_options.database(&db_name))
            .await?;
        let migrator = sqlx::migrate::Migrator::new(workspace_migrations_dir()).await?;
        migrator.run(&pool).await?;
        let repo = Repository::Postgres(pool.clone());
        Ok(Self {
            repo,
            pool,
            admin_pool,
            db_name,
        })
    }

    async fn cleanup(self) {
        let Self {
            repo,
            pool,
            admin_pool,
            db_name,
        } = self;
        drop(repo);
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

async fn insert_client(pool: &PgPool, client_id: &str, incarnation: Option<Uuid>) {
    sqlx::query(
        r#"
        INSERT INTO clients (
            id, display_name, public_key, status, internal_build_number,
            process_incarnation_id, capabilities
        )
        VALUES ($1, $1, decode('', 'hex'), 'online', 1, $2, '{}'::jsonb)
        "#,
    )
    .bind(client_id)
    .bind(incarnation)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_job_target(
    pool: &PgPool,
    job_id: Uuid,
    client_id: &str,
    status: &str,
    started: bool,
    target_incarnation: Option<Uuid>,
) {
    let operation = JobCommand::Shell {
        argv: vec!["true".to_string()],
        pty: false,
    };
    sqlx::query(
        r#"
        INSERT INTO jobs (
            id, command_type, status, target_count, payload_hash, operation,
            request_fingerprint, timeout_secs
        )
        VALUES ($1, 'shell', 'queued', 1, $2, $3, $4, 30)
        "#,
    )
    .bind(job_id)
    .bind(format!("hash-{job_id}"))
    .bind(sqlx::types::Json(operation))
    .bind(format!("fingerprint-{job_id}"))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO job_targets (
            job_id, client_id, status, started_at, process_incarnation_id,
            dispatch_lease_until, deadline_at
        )
        VALUES (
            $1,
            $2,
            $3,
            CASE WHEN $4 THEN now() - interval '5 seconds' ELSE NULL END,
            $5,
            now() - interval '1 second',
            CASE WHEN $4 THEN now() + interval '5 minutes' ELSE NULL END
        )
        "#,
    )
    .bind(job_id)
    .bind(client_id)
    .bind(status)
    .bind(started)
    .bind(target_incarnation)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_update_activation_target(
    pool: &PgPool,
    job_id: Uuid,
    client_id: &str,
    client_incarnation: Uuid,
    staged_sha256_hex: &str,
    deadline_elapsed: bool,
) {
    let operation = JobCommand::AgentUpdateActivate {
        staged_sha256_hex: staged_sha256_hex.to_string(),
        restart_agent: true,
    };
    sqlx::query(
        r#"
        INSERT INTO jobs (
            id, command_type, status, target_count, payload_hash, operation,
            request_fingerprint, timeout_secs
        )
        VALUES ($1, 'agent_update_activate', 'running', 1, $2, $3, $4, 1)
        "#,
    )
    .bind(job_id)
    .bind(format!("hash-{job_id}"))
    .bind(sqlx::types::Json(operation))
    .bind(format!("fingerprint-{job_id}"))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO job_targets (
            job_id, client_id, status, started_at, process_incarnation_id,
            dispatch_lease_until, deadline_at
        )
        VALUES (
            $1,
            $2,
            'running',
            now() - interval '10 seconds',
            $3,
            now() - interval '1 second',
            CASE WHEN $4 THEN now() - interval '1 second' ELSE now() + interval '5 minutes' END
        )
        "#,
    )
    .bind(job_id)
    .bind(client_id)
    .bind(client_incarnation)
    .bind(deadline_elapsed)
    .execute(pool)
    .await
    .unwrap();
}

fn hello_event(
    client_id: &str,
    process_incarnation_id: Uuid,
    update_heartbeat: Option<AgentUpdateHeartbeat>,
) -> GatewayAgentHelloIngest {
    GatewayAgentHelloIngest {
        gateway_id: "pg-test-gateway".to_string(),
        remote_ip: None,
        noise_public_key_hex: None,
        hello: AgentHello {
            client_id: client_id.to_string(),
            process_incarnation_id,
            agent_version: "pg-test-agent".to_string(),
            internal_build_number: 1,
            os_release: "test".to_string(),
            arch: "x86_64".to_string(),
            update_heartbeat,
            capabilities: AgentCapabilitySnapshot::default(),
        },
    }
}

async fn output_rows(pool: &PgPool, job_id: Uuid, client_id: &str) -> Vec<JobOutputView> {
    sqlx::query(
        r#"
        SELECT
            job_id,
            client_id,
            seq,
            stream,
            encode(data, 'base64') AS data_base64,
            storage,
            object_key AS artifact_object_key,
            data_sha256_hex AS artifact_sha256_hex,
            data_size_bytes AS artifact_size_bytes,
            exit_code,
            done,
            received_at::text AS received_at,
            created_at::text AS created_at
        FROM job_outputs
        WHERE job_id = $1 AND client_id = $2
        ORDER BY seq
        "#,
    )
    .bind(job_id)
    .bind(client_id)
    .fetch_all(pool)
    .await
    .unwrap()
    .into_iter()
    .map(|row| JobOutputView {
        job_id: row.try_get("job_id").unwrap(),
        client_id: row.try_get("client_id").unwrap(),
        seq: row.try_get("seq").unwrap(),
        stream: row.try_get("stream").unwrap(),
        data_base64: row.try_get("data_base64").unwrap(),
        storage: row.try_get("storage").unwrap(),
        artifact_object_key: row.try_get("artifact_object_key").unwrap(),
        artifact_sha256_hex: row.try_get("artifact_sha256_hex").unwrap(),
        artifact_size_bytes: row.try_get("artifact_size_bytes").unwrap(),
        exit_code: row.try_get("exit_code").unwrap(),
        done: row.try_get("done").unwrap(),
        received_at: row.try_get("received_at").unwrap(),
        created_at: row.try_get("created_at").unwrap(),
    })
    .collect()
}

async fn target_status(pool: &PgPool, job_id: Uuid, client_id: &str) -> String {
    sqlx::query_scalar("SELECT status FROM job_targets WHERE job_id = $1 AND client_id = $2")
        .bind(job_id)
        .bind(client_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn latest_status_output_json(
    pool: &PgPool,
    job_id: Uuid,
    client_id: &str,
) -> serde_json::Value {
    let value: String = sqlx::query_scalar(
        r#"
        SELECT convert_from(data, 'UTF8')
        FROM job_outputs
        WHERE job_id = $1 AND client_id = $2 AND stream = 'status'
        ORDER BY seq DESC
        LIMIT 1
        "#,
    )
    .bind(job_id)
    .bind(client_id)
    .fetch_one(pool)
    .await
    .unwrap();
    serde_json::from_str(&value).unwrap()
}

#[tokio::test]
async fn postgres_operator_login_throttle_persists_locked_username_bucket() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let throttle = crate::state::OperatorAuthThrottleConfig {
        username_failed_attempt_limit: 2,
        ip_failed_attempt_limit: 100,
        failed_attempt_window_secs: 60,
        lockout_secs: 60,
    };
    db.repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "admin".to_string(),
            password: "admin-password-123".to_string(),
        })
        .await
        .unwrap();

    for _ in 0..2 {
        assert!(matches!(
            db.repo
                .login_operator_with_throttle(
                    &LoginRequest {
                        username: "admin".to_string(),
                        password: "wrong-password-123".to_string(),
                        totp_code: None,
                    },
                    "203.0.113.30",
                    &throttle,
                )
                .await
                .unwrap(),
            crate::repository_auth::OperatorLoginAttempt::InvalidCredentials
        ));
    }
    let second_repo = Repository::Postgres(db.pool.clone());
    assert!(matches!(
        second_repo
            .login_operator_with_throttle(
                &LoginRequest {
                    username: "admin".to_string(),
                    password: "admin-password-123".to_string(),
                    totp_code: None,
                },
                "203.0.113.30",
                &throttle,
            )
            .await
            .unwrap(),
        crate::repository_auth::OperatorLoginAttempt::Throttled
    ));

    let row = sqlx::query(
        r#"
        SELECT failed_attempts,
               locked_until IS NOT NULL AND locked_until > now() AS locked
        FROM operator_auth_throttle
        WHERE scope_kind = 'username'
          AND scope_key = 'admin'
        "#,
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let failed_attempts: i64 = row.try_get("failed_attempts").unwrap();
    let locked: bool = row.try_get("locked").unwrap();
    assert_eq!(failed_attempts, 2);
    assert!(locked);
    let audit_count: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_logs WHERE action = $1")
        .bind("operator_auth.lockout_created")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(audit_count, 1);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_dispatch_claim_binds_incarnation_and_keeps_deadline_immutable() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "pg-client-a";
    let incarnation = Uuid::new_v4();
    let job_id = Uuid::new_v4();
    let stale_null_job_id = Uuid::new_v4();
    insert_client(&db.pool, client_id, Some(incarnation)).await;
    insert_job_target(&db.pool, job_id, client_id, "queued", false, None).await;
    insert_job_target(
        &db.pool,
        stale_null_job_id,
        client_id,
        "dispatching",
        true,
        None,
    )
    .await;

    let claimed = db.repo.claim_due_job_targets(10, 1, 0).await.unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].job_id, job_id);
    assert_eq!(claimed[0].process_incarnation_id, incarnation);
    let first_deadline: String = sqlx::query_scalar(
        "SELECT deadline_at::text FROM job_targets WHERE job_id = $1 AND client_id = $2",
    )
    .bind(job_id)
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let bound_incarnation: Uuid = sqlx::query_scalar(
        "SELECT process_incarnation_id FROM job_targets WHERE job_id = $1 AND client_id = $2",
    )
    .bind(job_id)
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(bound_incarnation, incarnation);

    sqlx::query(
        "UPDATE job_targets SET dispatch_lease_until = now() - interval '1 second' WHERE job_id = $1 AND client_id = $2",
    )
    .bind(job_id)
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let reclaimed = db.repo.claim_due_job_targets(10, 1, 0).await.unwrap();
    assert_eq!(reclaimed.len(), 1);
    assert_eq!(reclaimed[0].job_id, job_id);
    let second_deadline: String = sqlx::query_scalar(
        "SELECT deadline_at::text FROM job_targets WHERE job_id = $1 AND client_id = $2",
    )
    .bind(job_id)
    .bind(client_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(second_deadline, first_deadline);

    sqlx::query(
        "UPDATE job_targets SET dispatch_lease_until = now() - interval '1 second' WHERE job_id = $1 AND client_id = $2",
    )
    .bind(stale_null_job_id)
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let stale_null_claim = db.repo.claim_due_job_targets(10, 1, 0).await.unwrap();
    assert!(stale_null_claim.is_empty());
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_batch_output_conflict_poison_prevents_later_final_insert() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let job_id = Uuid::new_v4();
    let client_id = "pg-client-output";
    insert_client(&db.pool, client_id, Some(Uuid::new_v4())).await;
    insert_job_target(
        &db.pool,
        job_id,
        client_id,
        "running",
        true,
        Some(Uuid::new_v4()),
    )
    .await;
    let first = CommandOutput {
        job_id,
        stream: OutputStream::Stdout,
        data: b"first".to_vec(),
        exit_code: None,
        done: false,
    };
    db.repo
        .record_job_output_chunk_checked_with_config(
            job_id,
            client_id,
            0,
            &first,
            None,
            JobOutputPersistConfig {
                object_store: None,
                artifact_min_bytes: usize::MAX,
            },
        )
        .await
        .unwrap();

    let conflicting = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: br#"{"type":"different"}"#.to_vec(),
        exit_code: Some(1),
        done: false,
    };
    let later_final = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: br#"{"type":"completed"}"#.to_vec(),
        exit_code: Some(0),
        done: true,
    };
    let results = db
        .repo
        .record_job_outputs_checked_with_config(
            job_id,
            client_id,
            &[conflicting, later_final],
            JobOutputPersistConfig {
                object_store: None,
                artifact_min_bytes: usize::MAX,
            },
        )
        .await
        .unwrap();
    assert!(results.contains(&JobOutputWriteResult::DuplicateConflict));
    let outputs = output_rows(&db.pool, job_id, client_id).await;
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].seq, 0);
    assert!(!outputs[0].done);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_changed_incarnation_matching_update_heartbeat_completes_activation() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "pg-client-update-heartbeat";
    let old_incarnation = Uuid::new_v4();
    let new_incarnation = Uuid::new_v4();
    let job_id = Uuid::new_v4();
    let staged_sha256_hex = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    insert_client(&db.pool, client_id, Some(old_incarnation)).await;
    insert_update_activation_target(
        &db.pool,
        job_id,
        client_id,
        old_incarnation,
        staged_sha256_hex,
        false,
    )
    .await;

    db.repo
        .upsert_agent_hello(&hello_event(
            client_id,
            new_incarnation,
            Some(AgentUpdateHeartbeat {
                activation_job_id: job_id,
                sha256_hex: staged_sha256_hex.to_string(),
                marker_unix: 100,
                observed_unix: 101,
            }),
        ))
        .await
        .unwrap();

    assert_eq!(
        target_status(&db.pool, job_id, client_id).await,
        TARGET_STATUS_COMPLETED
    );
    let client_incarnation: Uuid =
        sqlx::query_scalar("SELECT process_incarnation_id FROM clients WHERE id = $1")
            .bind(client_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(client_incarnation, new_incarnation);
    let output = latest_status_output_json(&db.pool, job_id, client_id).await;
    assert_eq!(output["code"], "agent_update_restart_heartbeat_verified");
    assert_eq!(output["activation_job_id"], job_id.to_string());
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_missing_update_heartbeat_deadline_becomes_agent_lost() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "pg-client-update-timeout";
    let incarnation = Uuid::new_v4();
    let job_id = Uuid::new_v4();
    insert_client(&db.pool, client_id, Some(incarnation)).await;
    insert_update_activation_target(
        &db.pool,
        job_id,
        client_id,
        incarnation,
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        true,
    )
    .await;

    let expired = db.repo.expire_control_timeout_targets(10, 0).await.unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].job_id, job_id);
    assert_eq!(expired[0].status, TARGET_STATUS_AGENT_LOST);
    assert_eq!(
        target_status(&db.pool, job_id, client_id).await,
        TARGET_STATUS_AGENT_LOST
    );
    let output = latest_status_output_json(&db.pool, job_id, client_id).await;
    assert_eq!(output["code"], "agent_update_restart_missing_heartbeat");
    db.cleanup().await;
}
