use std::{path::Path, str::FromStr};

use axum::http::{header::AUTHORIZATION, HeaderMap};
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    PgPool, Row,
};
use tokio::sync::broadcast;
use uuid::Uuid;
use vpsman_common::{
    payload_hash, plan_tunnel, render_tunnel_endpoint_config, AgentCapabilitySnapshot, AgentHello,
    AgentUpdateHeartbeat, BandwidthTier, CommandOutput, GatewayAgentHelloIngest, JobCommand,
    OspfCostPolicy, OutputStream, TunnelEndpointSide, TunnelKind, TunnelPlanInput,
};
use vpsman_server_core::{
    JOB_STATUS_COMPLETED, JOB_STATUS_FAILED, TARGET_STATUS_AGENT_LOST, TARGET_STATUS_COMPLETED,
    TARGET_STATUS_FAILED,
};

use crate::{
    gateway_client::GatewayDispatchClient,
    model::{
        AuthContext, BootstrapOperatorRequest, JobOutputView, LoginRequest, NewServerArtifact,
    },
    repository::Repository,
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
            request_fingerprint, timeout_secs
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

fn postgres_test_plan_input() -> TunnelPlanInput {
    TunnelPlanInput {
        name: "pg-edge-a-edge-b".to_string(),
        interface_name: "pgab".to_string(),
        kind: TunnelKind::Gre,
        runtime_control: Default::default(),
        runtime_topology: Default::default(),
        left_client_id: "pg-left-a".to_string(),
        right_client_id: "pg-right-b".to_string(),
        left_underlay: "198.51.100.10".to_string(),
        right_underlay: "203.0.113.20".to_string(),
        address_pool_cidr: "10.250.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(vpsman_common::TunnelAddressPair {
            left: "10.250.0.0".to_string(),
            right: "10.250.0.1".to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: Default::default(),
        bandwidth: BandwidthTier::M100,
        latency_ms: 18.0,
        packet_loss_ratio: 0.0,
        preference: 1.0,
        ospf_policy: OspfCostPolicy::default(),
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
        .preview_artifact_cleanup(r#"artifact.domain = "job_output""#)
        .await
        .unwrap();
    assert_eq!(preview.matched_count, 1);
    let job = db
        .repo
        .create_artifact_cleanup_job(&preview.expression, &preview.preview_hash, &operator)
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
async fn postgres_completed_ospf_cost_update_syncs_canonical_tunnel_plan_cost_once() {
    let Some(db) = PgReliabilityTestDb::maybe_new().await else {
        return;
    };
    let operator = postgres_network_operator(&db.repo).await;
    let input = postgres_test_plan_input();
    let plan = plan_tunnel(&input).unwrap();
    insert_client(&db.pool, &input.left_client_id, Some(Uuid::new_v4())).await;
    insert_client(&db.pool, &input.right_client_id, Some(Uuid::new_v4())).await;
    db.repo
        .record_tunnel_plan(&input, &plan, &operator)
        .await
        .unwrap();

    let current_ospf_cost = plan.recommended_ospf_cost;
    let recommended_ospf_cost = current_ospf_cost + 10;
    let mut proposed_plan = plan.clone();
    proposed_plan.recommended_ospf_cost = recommended_ospf_cost;
    let endpoint = render_tunnel_endpoint_config(&proposed_plan, TunnelEndpointSide::Left).unwrap();
    let operation = JobCommand::NetworkOspfCostUpdate {
        plan: Box::new(proposed_plan.clone()),
        side: TunnelEndpointSide::Left,
        current_ospf_cost,
        recommended_ospf_cost,
        bird2_sha256_hex: payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
    };
    let job_id = Uuid::new_v4();

    db.repo
        .record_tunnel_plan_execution(job_id, &operation, "failed")
        .await
        .unwrap();
    let plans = db.repo.list_tunnel_plans().await.unwrap();
    assert_eq!(plans[0].recommended_ospf_cost, i32::from(current_ospf_cost));
    assert_eq!(plans[0].plan.recommended_ospf_cost, current_ospf_cost);

    db.repo
        .record_tunnel_plan_execution(job_id, &operation, "completed")
        .await
        .unwrap();
    db.repo
        .record_tunnel_plan_execution(job_id, &operation, "completed")
        .await
        .unwrap();
    let plans = db.repo.list_tunnel_plans().await.unwrap();
    assert_eq!(
        plans[0].recommended_ospf_cost,
        i32::from(recommended_ospf_cost)
    );
    assert_eq!(plans[0].plan.recommended_ospf_cost, recommended_ospf_cost);
    assert_eq!(plans[0].left_status, "planned");
    assert_eq!(plans[0].right_status, "planned");
    assert_eq!(plans[0].status, "planned");
    assert_eq!(plans[0].last_apply_job_id, None);
    assert_eq!(plans[0].last_rollback_job_id, None);

    let job_id_string = job_id.to_string();
    let first_audit_count: i64 = sqlx::query_scalar(
        r#"
        SELECT count(*)
        FROM audit_logs
        WHERE action = 'network.tunnel_plan_ospf_cost_updated'
          AND metadata->>'job_id' = $1
        "#,
    )
    .bind(&job_id_string)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(first_audit_count, 1);

    let stale_job_id = Uuid::new_v4();
    let stale_recommended = recommended_ospf_cost + 10;
    let mut stale_plan = proposed_plan;
    stale_plan.recommended_ospf_cost = stale_recommended;
    let stale_endpoint =
        render_tunnel_endpoint_config(&stale_plan, TunnelEndpointSide::Left).unwrap();
    let stale_operation = JobCommand::NetworkOspfCostUpdate {
        plan: Box::new(stale_plan),
        side: TunnelEndpointSide::Left,
        current_ospf_cost,
        recommended_ospf_cost: stale_recommended,
        bird2_sha256_hex: payload_hash(stale_endpoint.bird2_interface_snippet.as_bytes()),
    };
    db.repo
        .record_tunnel_plan_execution(stale_job_id, &stale_operation, "completed")
        .await
        .unwrap();
    let plans = db.repo.list_tunnel_plans().await.unwrap();
    assert_eq!(
        plans[0].recommended_ospf_cost,
        i32::from(recommended_ospf_cost)
    );
    assert_eq!(plans[0].plan.recommended_ospf_cost, recommended_ospf_cost);

    let stale_job_id_string = stale_job_id.to_string();
    let stale_audit_count: i64 = sqlx::query_scalar(
        r#"
        SELECT count(*)
        FROM audit_logs
        WHERE action = 'network.tunnel_plan_ospf_cost_updated'
          AND metadata->>'job_id' = $1
          AND metadata->>'result' = 'stale_ignored'
        "#,
    )
    .bind(&stale_job_id_string)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(stale_audit_count, 1);

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
    insert_client(&db.pool, client_id, Some(incarnation)).await;
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
