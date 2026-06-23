use std::{path::Path, str::FromStr, time::Duration};

use axum::http::{header::AUTHORIZATION, HeaderMap};
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    PgPool, Row,
};
use tokio::sync::broadcast;
use uuid::Uuid;
use vpsman_common::{
    payload_hash, AgentCapabilitySnapshot, AgentHello, AgentUpdateHeartbeat, CommandOutput,
    GatewayAgentHelloIngest, JobCommand, OutputStream,
};
use vpsman_server_core::{
    JOB_STATUS_CANCELED, JOB_STATUS_COMPLETED, JOB_STATUS_CONTROL_TIMEOUT, JOB_STATUS_FAILED,
    JOB_STATUS_SKIPPED, TARGET_STATUS_AGENT_LOST, TARGET_STATUS_CANCELED, TARGET_STATUS_COMPLETED,
    TARGET_STATUS_CONTROL_TIMEOUT, TARGET_STATUS_FAILED, TARGET_STATUS_SKIPPED,
};

use crate::{
    gateway_client::GatewayDispatchClient,
    model::{
        AuthContext, BackupRequestStatus, BootstrapOperatorRequest, CreateBackupRequest,
        CreateScheduleRequest, DeleteAgentRequest, JobOutputView, LoginRequest, NewServerArtifact,
        WsEvent,
    },
    repository::Repository,
    repository_backups::BackupRequestSourceLink,
    repository_job_outputs::{JobOutputPersistConfig, JobOutputWriteResult},
    state::{AppState, DispatcherRuntimeConfig, DEFAULT_ARTIFACT_MAX_BYTES},
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

fn postgres_app_state(db: &PgReliabilityTestDb) -> AppState {
    let (events, _) = broadcast::channel(16);
    AppState {
        repo: db.repo.clone(),
        events,
        internal_token: Some("gateway-secret-at-least-32-characters".to_string()),
        gateway: GatewayDispatchClient::default(),
        backup_object_store: None,
        update_release_policy: Default::default(),
        fleet_alert_policy: Default::default(),
        job_output_artifact_min_bytes: 32768,
        artifact_max_bytes: DEFAULT_ARTIFACT_MAX_BYTES,
        require_registered_agent_updates: false,
        suite_config_path: std::path::PathBuf::from("config/vpsman.toml"),
        dispatcher_config: DispatcherRuntimeConfig::default(),
    }
}

fn internal_gateway_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        "Bearer gateway-secret-at-least-32-characters"
            .parse()
            .unwrap(),
    );
    headers
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
            request_fingerprint, max_timeout_secs
        )
        VALUES ($1, 'shell', 'queued', 1, $2, $3, $4, 30)
        "#,
    )
    .bind(job_id)
    .bind(payload_hash(format!("payload-{job_id}").as_bytes()))
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

#[allow(clippy::too_many_arguments)]
async fn insert_job_target_with_operation(
    pool: &PgPool,
    job_id: Uuid,
    client_id: &str,
    operation: JobCommand,
    command_type: &str,
    source_schedule_id: Option<Uuid>,
    status: &str,
    started: bool,
    target_incarnation: Option<Uuid>,
    max_timeout_secs: i64,
    deadline_elapsed: bool,
) {
    let job_status = if status == "queued" {
        "queued"
    } else {
        "running"
    };
    sqlx::query(
        r#"
        INSERT INTO jobs (
            id, command_type, status, target_count, payload_hash, operation,
            source_schedule_id, request_fingerprint, max_timeout_secs
        )
        VALUES ($1, $2, $3, 1, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(job_id)
    .bind(command_type)
    .bind(job_status)
    .bind(payload_hash(format!("payload-{job_id}").as_bytes()))
    .bind(sqlx::types::Json(operation))
    .bind(source_schedule_id)
    .bind(format!("fingerprint-{job_id}"))
    .bind(max_timeout_secs)
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
            CASE WHEN $4 THEN now() - interval '10 seconds' ELSE NULL END,
            $5,
            now() - interval '1 second',
            CASE
                WHEN $4 AND $6 THEN now() - interval '1 second'
                WHEN $4 THEN now() + interval '5 minutes'
                ELSE NULL
            END
        )
        "#,
    )
    .bind(job_id)
    .bind(client_id)
    .bind(status)
    .bind(started)
    .bind(target_incarnation)
    .bind(deadline_elapsed)
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
            request_fingerprint, max_timeout_secs
        )
        VALUES ($1, 'agent_update_activate', 'running', 1, $2, $3, $4, 1)
        "#,
    )
    .bind(job_id)
    .bind(payload_hash(format!("payload-{job_id}").as_bytes()))
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
        gateway_session_id: Uuid::new_v4(),
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

async fn job_status(pool: &PgPool, job_id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM jobs WHERE id = $1")
        .bind(job_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn job_payload_hash(pool: &PgPool, job_id: Uuid) -> String {
    sqlx::query_scalar("SELECT payload_hash FROM jobs WHERE id = $1")
        .bind(job_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn webhook_event_exists(pool: &PgPool, kind: &str, event_id: &str) -> bool {
    sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM webhook_events
            WHERE kind = $1 AND event_id = $2
        )
        "#,
    )
    .bind(kind)
    .bind(event_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn processed_terminal_event_count(pool: &PgPool, job_id: Uuid) -> i64 {
    sqlx::query_scalar(
        r#"
        SELECT COUNT(*)
        FROM job_terminal_events
        WHERE job_id = $1 AND processing_status = 'processed'
        "#,
    )
    .bind(job_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn backup_request_status(pool: &PgPool, backup_request_id: Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM backup_requests WHERE id = $1")
        .bind(backup_request_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn schedule_outcome_row(pool: &PgPool, schedule_id: Uuid) -> (i32, String, Option<Uuid>) {
    let row = sqlx::query(
        r#"
        SELECT failure_count, COALESCE(last_job_status, '') AS last_job_status, last_job_id
        FROM schedules
        WHERE id = $1
        "#,
    )
    .bind(schedule_id)
    .fetch_one(pool)
    .await
    .unwrap();
    (
        row.try_get("failure_count").unwrap(),
        row.try_get("last_job_status").unwrap(),
        row.try_get("last_job_id").unwrap(),
    )
}

async fn receive_job_finished(
    rx: &mut broadcast::Receiver<WsEvent>,
    job_id: Uuid,
) -> Option<String> {
    for _ in 0..6 {
        let Ok(Ok(event)) = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await else {
            continue;
        };
        if let WsEvent::JobFinished {
            job_id: event_job_id,
            status,
        } = event
        {
            if event_job_id == job_id {
                return Some(status);
            }
        }
    }
    None
}

fn postgres_shell_schedule_request(name: &str, client_id: &str) -> CreateScheduleRequest {
    CreateScheduleRequest {
        name: name.to_string(),
        operation: JobCommand::Shell {
            argv: vec![
                "/bin/sh".to_string(),
                "-lc".to_string(),
                "uptime".to_string(),
            ],
            pty: false,
        },
        selector_expression: String::new(),
        target_client_ids: vec![client_id.to_string()],
        cron_expr: "0 * * * *".to_string(),
        timezone: "UTC".to_string(),
        enabled: true,
        catch_up_policy: "skip_missed".to_string(),
        catch_up_limit: 1,
        retry_delay_secs: 120,
        max_failures: 2,
        privilege_assertion: None,
        confirmed: true,
    }
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

async fn postgres_network_operator(repo: &Repository) -> AuthContext {
    let auth = repo
        .bootstrap_operator(&BootstrapOperatorRequest {
            username: "network-operator".to_string(),
            password: "network-password-123".to_string(),
        })
        .await
        .unwrap();
    AuthContext {
        operator: auth.operator,
        session_id: Uuid::nil(),
    }
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
                    None,
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
                None,
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
async fn postgres_artifact_cleanup_job_persists_reviewed_artifact_identity() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let operator = postgres_network_operator(&db.repo).await;
    db.repo
        .register_server_artifact(NewServerArtifact {
            domain: "job_output".to_string(),
            object_key: "job-output/test-reviewed-artifact".to_string(),
            sha256_hex: "a".repeat(64),
            size_bytes: 12,
            job_id: Some(Uuid::new_v4()),
            client_id: Some("edge-reviewed".to_string()),
            stream: Some("stdout".to_string()),
            seq: Some(0),
            backup_request_id: None,
            backup_artifact_id: None,
            release_id: None,
            metadata: serde_json::json!({}),
        })
        .await
        .unwrap();

    let preview = db
        .repo
        .preview_artifact_cleanup(
            r#"artifact.domain = "job_output""#,
            &["job_output".to_string()],
        )
        .await
        .unwrap();
    assert_eq!(preview.matched_count, 1);
    let job = db
        .repo
        .create_artifact_cleanup_job(
            &preview.expression,
            &preview.domains,
            &preview.preview_hash,
            &operator,
        )
        .await
        .unwrap();

    let row = sqlx::query(
        r#"
        SELECT domain, object_key, sha256_hex, size_bytes
        FROM server_job_artifact_cleanup_targets
        WHERE server_job_id = $1
        "#,
    )
    .bind(job.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>("domain"), "job_output");
    assert_eq!(
        row.get::<String, _>("object_key"),
        "job-output/test-reviewed-artifact"
    );
    assert_eq!(row.get::<String, _>("sha256_hex"), "a".repeat(64));
    assert_eq!(row.get::<i64, _>("size_bytes"), 12);

    sqlx::query(
        r#"
        UPDATE server_artifacts
        SET sha256_hex = $2, size_bytes = $3
        WHERE object_key = $1
        "#,
    )
    .bind("job-output/test-reviewed-artifact")
    .bind("b".repeat(64))
    .bind(13_i64)
    .execute(&db.pool)
    .await
    .unwrap();
    let identity_matches_review: bool = sqlx::query_scalar(
        r#"
        SELECT (
            artifact.domain = target.domain
            AND artifact.object_key = target.object_key
            AND artifact.sha256_hex = target.sha256_hex
            AND artifact.size_bytes = target.size_bytes
        )
        FROM server_job_artifact_cleanup_targets target
        JOIN server_artifacts artifact ON artifact.id = target.artifact_id
        WHERE target.server_job_id = $1
        "#,
    )
    .bind(job.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(!identity_matches_review);
    sqlx::query("DELETE FROM server_artifacts WHERE object_key = $1")
        .bind("job-output/test-reviewed-artifact")
        .execute(&db.pool)
        .await
        .unwrap();
    let reviewed_target_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM server_job_artifact_cleanup_targets WHERE server_job_id = $1",
    )
    .bind(job.id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(reviewed_target_count, 1);
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
async fn postgres_command_output_ingest_rejects_late_new_output_after_terminal_target() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let job_id = Uuid::new_v4();
    let client_id = "pg-client-late-output";
    let incarnation = Uuid::new_v4();
    let gateway_session_id = Uuid::new_v4();
    insert_client(&db.pool, client_id, Some(incarnation)).await;
    sqlx::query(
        r#"
        INSERT INTO gateway_sessions (id, gateway_id, client_id, status)
        VALUES ($1, 'gateway-a', $2, 'active')
        "#,
    )
    .bind(gateway_session_id)
    .bind(client_id)
    .execute(&db.pool)
    .await
    .unwrap();
    insert_job_target(
        &db.pool,
        job_id,
        client_id,
        "running",
        true,
        Some(incarnation),
    )
    .await;
    let state = postgres_app_state(&db);
    let payload_hash = job_payload_hash(&db.pool, job_id).await;
    let final_output = CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: br#"{"type":"completed"}"#.to_vec(),
        exit_code: Some(0),
        done: true,
    };
    let final_event = vpsman_common::GatewayCommandOutputIngest {
        gateway_id: "gateway-a".to_string(),
        gateway_session_id,
        process_incarnation_id: incarnation,
        spooled_replay: false,
        client_id: client_id.to_string(),
        job_id,
        payload_hash: payload_hash.clone(),
        seq: 0,
        received_unix: Some(100),
        output: final_output,
    };
    let _ = crate::routes_ingest::ingest_command_output(
        axum::extract::State(state.clone()),
        internal_gateway_headers(),
        axum::Json(final_event.clone()),
    )
    .await
    .unwrap();
    assert_eq!(
        target_status(&db.pool, job_id, client_id).await,
        TARGET_STATUS_COMPLETED
    );
    assert_eq!(job_status(&db.pool, job_id).await, JOB_STATUS_COMPLETED);

    let _ = crate::routes_ingest::ingest_command_output(
        axum::extract::State(state.clone()),
        internal_gateway_headers(),
        axum::Json(final_event),
    )
    .await
    .unwrap();

    let late_output = CommandOutput {
        job_id,
        stream: OutputStream::Stdout,
        data: b"late data".to_vec(),
        exit_code: None,
        done: false,
    };
    let late_event = vpsman_common::GatewayCommandOutputIngest {
        gateway_id: "gateway-a".to_string(),
        gateway_session_id,
        process_incarnation_id: incarnation,
        spooled_replay: false,
        client_id: client_id.to_string(),
        job_id,
        payload_hash,
        seq: 1,
        received_unix: Some(101),
        output: late_output,
    };
    let error = crate::routes_ingest::ingest_command_output(
        axum::extract::State(state),
        internal_gateway_headers(),
        axum::Json(late_event),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "job_target_not_active");
    let outputs = output_rows(&db.pool, job_id, client_id).await;
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].seq, 0);
    assert!(outputs[0].done);
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
    assert_eq!(job_status(&db.pool, job_id).await, JOB_STATUS_COMPLETED);
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
async fn postgres_changed_incarnation_matching_job_but_wrong_hash_fails_activation() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "pg-client-update-heartbeat-mismatch";
    let old_incarnation = Uuid::new_v4();
    let new_incarnation = Uuid::new_v4();
    let job_id = Uuid::new_v4();
    let staged_sha256_hex = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let observed_sha256_hex = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
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
                sha256_hex: observed_sha256_hex.to_string(),
                marker_unix: 100,
                observed_unix: 101,
            }),
        ))
        .await
        .unwrap();

    assert_eq!(
        target_status(&db.pool, job_id, client_id).await,
        TARGET_STATUS_FAILED
    );
    assert_eq!(job_status(&db.pool, job_id).await, JOB_STATUS_FAILED);
    let output = latest_status_output_json(&db.pool, job_id, client_id).await;
    assert_eq!(
        output["code"],
        "agent_update_activation_heartbeat_hash_mismatch"
    );
    assert_eq!(output["activation_job_id"], job_id.to_string());
    assert_eq!(output["artifact_sha256_hex"], observed_sha256_hex);
    assert_eq!(output["staged_sha256_hex"], staged_sha256_hex);
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
    assert_eq!(job_status(&db.pool, job_id).await, JOB_STATUS_FAILED);
    let output = latest_status_output_json(&db.pool, job_id, client_id).await;
    assert_eq!(output["code"], "agent_update_restart_missing_heartbeat");
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_control_timeout_terminal_event_updates_schedule_and_webhooks() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "pg-client-scheduled-timeout";
    let job_id = Uuid::new_v4();
    insert_client(&db.pool, client_id, Some(Uuid::new_v4())).await;
    let operator = postgres_network_operator(&db.repo).await;
    let schedule = db
        .repo
        .create_schedule(
            postgres_shell_schedule_request("pg-timeout-schedule", client_id),
            &operator,
        )
        .await
        .unwrap();
    insert_job_target_with_operation(
        &db.pool,
        job_id,
        client_id,
        JobCommand::Shell {
            argv: vec![
                "/bin/sh".to_string(),
                "-lc".to_string(),
                "sleep 99".to_string(),
            ],
            pty: false,
        },
        "shell",
        Some(schedule.id),
        "running",
        true,
        Some(Uuid::new_v4()),
        1,
        true,
    )
    .await;

    let expired = db.repo.expire_control_timeout_targets(10, 0).await.unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].status, TARGET_STATUS_CONTROL_TIMEOUT);
    let state = postgres_app_state(&db);
    let batch = state.process_job_terminal_events(500).await.unwrap();
    assert!(batch
        .jobs
        .iter()
        .any(|event| event.job_id == job_id && event.status == JOB_STATUS_CONTROL_TIMEOUT));

    assert_eq!(
        job_status(&db.pool, job_id).await,
        JOB_STATUS_CONTROL_TIMEOUT
    );
    let (failure_count, last_job_status, last_job_id) =
        schedule_outcome_row(&db.pool, schedule.id).await;
    assert_eq!(failure_count, 1);
    assert_eq!(last_job_status, JOB_STATUS_CONTROL_TIMEOUT);
    assert_eq!(last_job_id, Some(job_id));
    assert!(
        webhook_event_exists(
            &db.pool,
            "job.status",
            &format!("job:{job_id}:status:{JOB_STATUS_CONTROL_TIMEOUT}")
        )
        .await
    );
    assert!(
        webhook_event_exists(
            &db.pool,
            "schedule.job_finished",
            &format!("schedule:{}:job:{job_id}:finished", schedule.id)
        )
        .await
    );
    assert!(
        webhook_event_exists(
            &db.pool,
            "schedule.failed",
            &format!("schedule:{}:job:{job_id}:failed", schedule.id)
        )
        .await
    );
    assert_eq!(processed_terminal_event_count(&db.pool, job_id).await, 2);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_queued_cancel_terminal_event_records_target_and_job_side_effects() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "pg-client-queued-cancel";
    let job_id = Uuid::new_v4();
    insert_client(&db.pool, client_id, Some(Uuid::new_v4())).await;
    insert_job_target(&db.pool, job_id, client_id, "queued", false, None).await;
    let operator = postgres_network_operator(&db.repo).await;

    let plan = db
        .repo
        .request_job_cancel(job_id, operator.operator.id, Some("test cancel"))
        .await
        .unwrap();
    assert_eq!(plan.pending_canceled, 1);
    let state = postgres_app_state(&db);
    let batch = state.process_job_terminal_events(500).await.unwrap();
    assert!(batch.targets.iter().any(|event| event.job_id == job_id
        && event.client_id == client_id
        && event.outcome.status == TARGET_STATUS_CANCELED));
    assert!(batch
        .jobs
        .iter()
        .any(|event| event.job_id == job_id && event.status == JOB_STATUS_CANCELED));

    assert_eq!(
        target_status(&db.pool, job_id, client_id).await,
        TARGET_STATUS_CANCELED
    );
    assert_eq!(job_status(&db.pool, job_id).await, JOB_STATUS_CANCELED);
    assert!(
        webhook_event_exists(
            &db.pool,
            "job.target.status",
            &format!("job:{job_id}:target:{client_id}:status:{TARGET_STATUS_CANCELED}")
        )
        .await
    );
    assert!(
        webhook_event_exists(
            &db.pool,
            "job.status",
            &format!("job:{job_id}:status:{JOB_STATUS_CANCELED}")
        )
        .await
    );
    assert_eq!(processed_terminal_event_count(&db.pool, job_id).await, 2);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_agent_hello_cleanup_processes_terminal_events_and_publishes_finish() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "pg-client-hello-terminal-events";
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
    let state = postgres_app_state(&db);
    let mut rx = state.events.subscribe();

    let _ = crate::routes_ingest::ingest_agent_hello(
        axum::extract::State(state.clone()),
        internal_gateway_headers(),
        axum::Json(hello_event(
            client_id,
            new_incarnation,
            Some(AgentUpdateHeartbeat {
                activation_job_id: job_id,
                sha256_hex: staged_sha256_hex.to_string(),
                marker_unix: 100,
                observed_unix: 101,
            }),
        )),
    )
    .await
    .unwrap();

    assert_eq!(
        receive_job_finished(&mut rx, job_id).await,
        Some(JOB_STATUS_COMPLETED.to_string())
    );
    assert_eq!(job_status(&db.pool, job_id).await, JOB_STATUS_COMPLETED);
    assert!(
        webhook_event_exists(
            &db.pool,
            "job.target.status",
            &format!("job:{job_id}:target:{client_id}:status:{TARGET_STATUS_COMPLETED}")
        )
        .await
    );
    assert!(
        webhook_event_exists(
            &db.pool,
            "job.status",
            &format!("job:{job_id}:status:{JOB_STATUS_COMPLETED}")
        )
        .await
    );
    assert_eq!(processed_terminal_event_count(&db.pool, job_id).await, 2);
    db.cleanup().await;
}

#[tokio::test]
async fn postgres_delete_agent_cleanup_terminal_events_cover_backup_and_queued_skip() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let client_id = "pg-client-delete-cleanup";
    let incarnation = Uuid::new_v4();
    let backup_job_id = Uuid::new_v4();
    let queued_job_id = Uuid::new_v4();
    insert_client(&db.pool, client_id, Some(incarnation)).await;
    let operator = postgres_network_operator(&db.repo).await;
    insert_job_target_with_operation(
        &db.pool,
        backup_job_id,
        client_id,
        JobCommand::Backup {
            paths: vec!["/etc".to_string()],
            include_config: false,
            follow_symlinks: false,
        },
        "backup",
        None,
        "running",
        true,
        Some(incarnation),
        30,
        false,
    )
    .await;
    insert_job_target(&db.pool, queued_job_id, client_id, "queued", false, None).await;
    let backup_request = db
        .repo
        .record_backup_request_with_source(
            &CreateBackupRequest {
                client_id: client_id.to_string(),
                paths: vec!["/etc".to_string()],
                include_config: false,
                follow_symlinks: false,
                confirmed: true,
                note: None,
                privilege_assertion: None,
            },
            "backup-request-payload",
            &format!("client:{client_id}"),
            &operator,
            BackupRequestStatus::RequestedMetadataOnly,
            BackupRequestSourceLink {
                job_id: Some(backup_job_id),
                schedule_id: None,
            },
        )
        .await
        .unwrap();

    db.repo
        .delete_agent(
            client_id,
            &DeleteAgentRequest {
                confirmed: true,
                reason: Some("test delete".to_string()),
                privilege_assertion: None,
            },
            &operator,
        )
        .await
        .unwrap();
    let state = postgres_app_state(&db);
    state.process_job_terminal_events(500).await.unwrap();

    assert_eq!(
        backup_request_status(&db.pool, backup_request.id).await,
        BackupRequestStatus::ExecutionFailed.as_str()
    );
    assert_eq!(
        target_status(&db.pool, backup_job_id, client_id).await,
        TARGET_STATUS_AGENT_LOST
    );
    assert_eq!(job_status(&db.pool, backup_job_id).await, JOB_STATUS_FAILED);
    assert_eq!(
        target_status(&db.pool, queued_job_id, client_id).await,
        TARGET_STATUS_SKIPPED
    );
    assert_eq!(
        job_status(&db.pool, queued_job_id).await,
        JOB_STATUS_SKIPPED
    );
    assert!(
        webhook_event_exists(
            &db.pool,
            "job.status",
            &format!("job:{backup_job_id}:status:{JOB_STATUS_FAILED}")
        )
        .await
    );
    assert!(
        webhook_event_exists(
            &db.pool,
            "job.target.status",
            &format!("job:{queued_job_id}:target:{client_id}:status:{TARGET_STATUS_SKIPPED}")
        )
        .await
    );
    assert!(
        webhook_event_exists(
            &db.pool,
            "job.status",
            &format!("job:{queued_job_id}:status:{JOB_STATUS_SKIPPED}")
        )
        .await
    );
    assert_eq!(
        processed_terminal_event_count(&db.pool, backup_job_id).await,
        2
    );
    assert_eq!(
        processed_terminal_event_count(&db.pool, queued_job_id).await,
        2
    );
    db.cleanup().await;
}
