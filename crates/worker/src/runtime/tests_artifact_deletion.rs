#[tokio::test]
async fn artifact_deletion_claim_commits_before_work_and_stale_tokens_cannot_finish() {
    use serde_json::json;
    use uuid::Uuid;

    use crate::test_support::PgWorkerTestDb;

    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let artifact_id = Uuid::new_v4();
    let source_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO server_artifacts (
            id, domain, object_key, sha256_hex, size_bytes, status
        )
        VALUES ($1, 'job_output', $2, $3, 9, 'active')
        "#,
    )
    .bind(artifact_id)
    .bind("job-outputs/exact-owner")
    .bind("a".repeat(64))
    .execute(&db.pool)
    .await
    .unwrap();
    super::enqueue_artifact_deletion(
        &db.pool,
        &super::ArtifactDeletionReview {
            artifact_id,
            object_key: "job-outputs/exact-owner".to_string(),
            sha256_hex: "a".repeat(64),
            size_bytes: 9,
            source_kind: "manual_cleanup",
            source_id,
            source_revision: 1,
            source_identity: json!({"artifact_id": artifact_id}),
        },
    )
    .await
    .unwrap();
    let first = super::claim_artifact_deletion(
        &db.pool,
        Some("manual_cleanup"),
        Some(source_id),
        Some(artifact_id),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(super::claim_artifact_deletion(
        &db.pool,
        Some("manual_cleanup"),
        Some(source_id),
        Some(artifact_id),
    )
    .await
    .unwrap()
    .is_none());

    // A committed claim must not retain a transaction/row lock while object
    // I/O is in flight.
    let observer = db.additional_pool(1).await.unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        sqlx::query(
            "UPDATE server_artifacts SET metadata = metadata || '{\"probe\":true}'::jsonb WHERE id = $1",
        )
        .bind(artifact_id)
        .execute(&observer),
    )
    .await
    .expect("artifact claim retained a database lock")
    .unwrap();

    sqlx::query(
        "UPDATE server_artifact_deletion_intents SET lease_until = now() - interval '1 second' WHERE artifact_id = $1",
    )
    .bind(artifact_id)
    .execute(&db.pool)
    .await
    .unwrap();
    let second = super::claim_artifact_deletion(
        &db.pool,
        Some("manual_cleanup"),
        Some(source_id),
        Some(artifact_id),
    )
    .await
    .unwrap()
    .unwrap();
    assert_ne!(first.lease_id, second.lease_id);
    assert!(!super::fail_artifact_deletion(&db.pool, &first, "stale")
        .await
        .unwrap());

    let mut tx = db.pool.begin().await.unwrap();
    assert!(super::lock_owned_artifact_deletion_in_tx(&mut tx, &second)
        .await
        .unwrap());
    assert!(super::finish_artifact_deletion_in_tx(&mut tx, &second)
        .await
        .unwrap());
    tx.commit().await.unwrap();
    observer.close().await;
    db.cleanup().await;
}

#[tokio::test]
async fn manual_completion_notification_is_commit_coupled_across_connections() {
    use std::time::Duration;

    use sqlx::postgres::PgListener;
    use uuid::Uuid;

    use crate::test_support::PgWorkerTestDb;

    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let listener_pool = db.additional_pool(1).await.unwrap();
    let mut listener = PgListener::connect_with(&listener_pool).await.unwrap();
    listener
        .listen(super::ARTIFACT_DELETION_COMPLETED_CHANNEL)
        .await
        .unwrap();

    let source_id = Uuid::new_v4();
    let mut tx = db.pool.begin().await.unwrap();
    super::publish_artifact_deletion_completion_in_tx(&mut tx, source_id)
        .await
        .unwrap();

    let notification = {
        let received = listener.recv();
        tokio::pin!(received);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut received)
                .await
                .is_err()
        );
        tx.commit().await.unwrap();

        tokio::time::timeout(Duration::from_secs(2), &mut received)
            .await
            .expect("committed manual completion did not wake another connection")
            .unwrap()
    };
    assert_eq!(
        notification.channel(),
        super::ARTIFACT_DELETION_COMPLETED_CHANNEL
    );
    assert_eq!(notification.payload(), source_id.to_string());

    drop(listener);
    listener_pool.close().await;
    db.cleanup().await;
}

#[tokio::test]
async fn cleanup_job_claim_atomically_recovers_and_fences_the_stale_worker() {
    use uuid::Uuid;

    use crate::test_support::PgWorkerTestDb;

    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let job_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO server_jobs (
            id, job_type, status, expression, preview_hash, metadata
        )
        VALUES ($1, 'artifact_cleanup', 'queued', 'artifact.status = "active"', $2, $3)
        "#,
    )
    .bind(job_id)
    .bind("a".repeat(64))
    .bind(serde_json::json!({"domains":["job_output"]}))
    .execute(&db.pool)
    .await
    .unwrap();
    let first = crate::claim_artifact_cleanup_job(&db.pool)
        .await
        .unwrap()
        .unwrap();
    assert!(crate::claim_artifact_cleanup_job(&db.pool)
        .await
        .unwrap()
        .is_none());
    sqlx::query("UPDATE server_jobs SET lease_until = now() - interval '1 second' WHERE id = $1")
        .bind(job_id)
        .execute(&db.pool)
        .await
        .unwrap();
    let second = crate::claim_artifact_cleanup_job(&db.pool)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(first.lease_id, second.lease_id);
    let recovered: bool =
        sqlx::query_scalar("SELECT metadata ? 'owner_recovered_at' FROM server_jobs WHERE id = $1")
            .bind(job_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(recovered);
    assert!(
        !crate::mark_artifact_cleanup_job_failed(&db.pool, &first, "stale")
            .await
            .unwrap()
    );
    assert!(
        crate::mark_artifact_cleanup_job_failed(&db.pool, &second, "expected")
            .await
            .unwrap()
    );
    let status: String = sqlx::query_scalar("SELECT status FROM server_jobs WHERE id = $1")
        .bind(job_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(status, "failed");
    db.cleanup().await;
}

#[tokio::test]
async fn cleanup_job_round_claims_each_starting_owner_once_and_excludes_later_appends() {
    use uuid::Uuid;

    use crate::test_support::PgWorkerTestDb;

    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let first_ids = [Uuid::new_v4(), Uuid::new_v4()];
    for job_id in first_ids {
        sqlx::query(
            r#"
            INSERT INTO server_jobs (
                id, job_type, status, expression, preview_hash, metadata
            )
            VALUES ($1, 'artifact_cleanup', 'queued',
                    'artifact.status = "active"', $2, $3)
            "#,
        )
        .bind(job_id)
        .bind("d".repeat(64))
        .bind(serde_json::json!({"domains":["job_output"]}))
        .execute(&db.pool)
        .await
        .unwrap();
    }
    let frontier = crate::artifact_cleanup_round_frontier(&db.pool)
        .await
        .unwrap()
        .unwrap();
    let later_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO server_jobs (
            id, job_type, status, expression, preview_hash, metadata, created_at
        )
        VALUES ($1, 'artifact_cleanup', 'queued',
                'artifact.status = "active"', $2, $3, $4 + interval '1 second')
        "#,
    )
    .bind(later_id)
    .bind("e".repeat(64))
    .bind(serde_json::json!({"domains":["job_output"]}))
    .bind(frontier.created_at)
    .execute(&db.pool)
    .await
    .unwrap();

    let mut claimed_ids = Vec::new();
    while let Some(job) = crate::claim_artifact_cleanup_job_through(&db.pool, Some(frontier))
        .await
        .unwrap()
    {
        claimed_ids.push(job.id);
        assert!(
            crate::mark_artifact_cleanup_job_failed(&db.pool, &job, "test-terminal")
                .await
                .unwrap()
        );
    }
    claimed_ids.sort();
    let mut expected = first_ids.to_vec();
    expected.sort();
    assert_eq!(claimed_ids, expected);
    let later_status: String = sqlx::query_scalar("SELECT status FROM server_jobs WHERE id=$1")
        .bind(later_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(later_status, "queued");
    db.cleanup().await;
}

#[tokio::test]
async fn claimed_artifact_failure_keeps_later_independent_intents_claimable() {
    use uuid::Uuid;
    use vpsman_object_store::BackupObjectStore;

    use crate::test_support::PgWorkerTestDb;

    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let root = std::env::temp_dir().join(format!("vpsman-artifact-fairness-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(&root).await.unwrap();
    let store = BackupObjectStore::filesystem(root.clone()).unwrap();
    for ordinal in 0..2 {
        let artifact_id = Uuid::new_v4();
        let object_key = format!("job-outputs/invalid-history-{ordinal}-{artifact_id}");
        sqlx::query(
            r#"
            INSERT INTO server_artifacts (
                id, domain, object_key, sha256_hex, size_bytes, status
            ) VALUES ($1, 'job_output', $2, $3, 1, 'active')
            "#,
        )
        .bind(artifact_id)
        .bind(&object_key)
        .bind("f".repeat(64))
        .execute(&db.pool)
        .await
        .unwrap();
        assert!(super::enqueue_artifact_deletion(
            &db.pool,
            &super::ArtifactDeletionReview {
                artifact_id,
                object_key,
                sha256_hex: "f".repeat(64),
                size_bytes: 1,
                source_kind: "history_retention",
                source_id: Uuid::new_v4(),
                source_revision: 1,
                source_identity: serde_json::json!({}),
            },
        )
        .await
        .unwrap());
    }

    for _ in 0..2 {
        let error = crate::process_next_artifact_deletion_intent(&db.pool, &store)
            .await
            .unwrap_err();
        assert!(error.is::<crate::ClaimedArtifactDeletionError>());
    }
    assert!(
        !crate::process_next_artifact_deletion_intent(&db.pool, &store)
            .await
            .unwrap()
    );
    let leased: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM server_artifact_deletion_intents WHERE lease_id IS NOT NULL",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(leased, 2);
    tokio::fs::remove_dir_all(&root).await.unwrap();
    db.cleanup().await;
}

#[tokio::test]
async fn manual_cleanup_producer_commits_intent_before_named_consumer_deletes() {
    use uuid::Uuid;
    use vpsman_object_store::BackupObjectStore;

    use crate::test_support::PgWorkerTestDb;

    let Some(db) = PgWorkerTestDb::maybe_new().await else {
        return;
    };
    let job_id = Uuid::new_v4();
    let artifact_id = Uuid::new_v4();
    let object_key = format!("job-outputs/{artifact_id}");
    let root = std::env::temp_dir().join(format!("vpsman-artifact-owner-{artifact_id}"));
    tokio::fs::create_dir_all(root.join("job-outputs"))
        .await
        .unwrap();
    tokio::fs::write(root.join(&object_key), b"owned")
        .await
        .unwrap();
    let store = BackupObjectStore::filesystem(root.clone()).unwrap();

    sqlx::query(
        r#"
        INSERT INTO server_jobs (
            id, job_type, status, expression, preview_hash, metadata
        ) VALUES ($1, 'artifact_cleanup', 'queued', 'artifact.status = "active"', $2, $3)
        "#,
    )
    .bind(job_id)
    .bind("b".repeat(64))
    .bind(serde_json::json!({"domains":["job_output"]}))
    .execute(&db.pool)
    .await
    .unwrap();
    let job = crate::claim_artifact_cleanup_job(&db.pool)
        .await
        .unwrap()
        .unwrap();
    sqlx::query(
        r#"
        INSERT INTO server_artifacts (
            id, domain, object_key, sha256_hex, size_bytes, status
        ) VALUES ($1, 'job_output', $2, $3, 5, 'active')
        "#,
    )
    .bind(artifact_id)
    .bind(&object_key)
    .bind("c".repeat(64))
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO server_job_artifact_cleanup_targets (
            server_job_id, artifact_id, domain, object_key,
            sha256_hex, size_bytes, status_at_review
        ) VALUES ($1, $2, 'job_output', $3, $4, 5, 'active')
        "#,
    )
    .bind(job_id)
    .bind(artifact_id)
    .bind(&object_key)
    .bind("c".repeat(64))
    .execute(&db.pool)
    .await
    .unwrap();

    assert!(super::enqueue_artifact_deletion(
        &db.pool,
        &super::ArtifactDeletionReview {
            artifact_id,
            object_key: object_key.clone(),
            sha256_hex: "c".repeat(64),
            size_bytes: 5,
            source_kind: "manual_cleanup",
            source_id: job_id,
            source_revision: 1,
            source_identity: serde_json::json!({
                "server_job_id": job_id,
                "artifact_id": artifact_id,
                "domain": "job_output",
                "object_key": object_key,
                "sha256_hex": "c".repeat(64),
                "size_bytes": 5,
            }),
        },
    )
    .await
    .unwrap());
    assert!(tokio::fs::try_exists(root.join(&object_key)).await.unwrap());

    assert!(
        crate::process_next_artifact_deletion_intent(&db.pool, &store)
            .await
            .unwrap()
    );
    assert!(!tokio::fs::try_exists(root.join(&object_key)).await.unwrap());
    let outcome: String = sqlx::query_scalar(
        "SELECT outcome FROM server_job_artifact_cleanup_targets WHERE server_job_id=$1 AND artifact_id=$2",
    )
    .bind(job.id)
    .bind(artifact_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(outcome, "deleted");
    let intents: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM server_artifact_deletion_intents WHERE artifact_id=$1",
    )
    .bind(artifact_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(intents, 0);

    tokio::fs::remove_dir_all(&root).await.unwrap();
    db.cleanup().await;
}
