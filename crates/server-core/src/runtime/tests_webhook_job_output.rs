use super::*;
use sqlx::Connection;

#[test]
fn output_selection_tracks_streams_clients_and_parent_references() {
    let (stdout, stderr) =
        output_selection("{job . output.stdout.a} [if job.output.stderr.b = failed]yes[endif]")
            .unwrap();
    assert!(!stdout.all_clients && stdout.includes("a") && !stdout.includes("b"));
    assert!(!stderr.all_clients && stderr.includes("b") && !stderr.includes("a"));
    let (stdout, stderr) = output_selection("{job.output}").unwrap();
    assert!(stdout.all_clients && stderr.all_clients);
    let (stdout, stderr) = output_selection("{job.output.stdout.length}").unwrap();
    assert!(stdout.all_clients && !stderr.needed());
}

#[test]
fn output_selection_ignores_comments_literals_and_unrelated_names() {
    let (stdout, stderr) = output_selection(
        "job.output.stdout {# {job.output} #} [if event.kind = job.output]literal[endif] {job.output.stdout_extra}",
    ).unwrap();
    assert!(!stdout.needed() && !stderr.needed());
}

async fn insert_inline(
    connection: &mut PgConnection,
    job_id: Uuid,
    client_id: &str,
    seq: i32,
    stream: &str,
    data: &[u8],
) {
    sqlx::query("INSERT INTO job_outputs(job_id,client_id,seq,stream,data,data_size_bytes,data_sha256_hex) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(job_id).bind(client_id).bind(seq).bind(stream).bind(data)
        .bind(data.len() as i64).bind(payload_hash(data))
        .execute(connection).await.unwrap();
}

#[tokio::test]
async fn postgres_webhook_output_reads_complete_selected_streams_and_verified_artifacts() {
    let Ok(url) = std::env::var("VPSMAN_TEST_POSTGRES_URL") else {
        eprintln!("skipping output PostgreSQL test: VPSMAN_TEST_POSTGRES_URL is unset");
        return;
    };
    let mut connection = PgConnection::connect(&url).await.unwrap();
    sqlx::raw_sql(
        r#"
        CREATE TEMP TABLE job_outputs (
            job_id uuid NOT NULL, client_id text NOT NULL, seq integer NOT NULL,
            stream text NOT NULL, data bytea NOT NULL, storage text NOT NULL DEFAULT 'inline',
            data_size_bytes bigint, data_sha256_hex text, object_key text,
            PRIMARY KEY(job_id,client_id,seq)
        );
        "#,
    )
    .execute(&mut connection)
    .await
    .unwrap();

    let project = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let artifact_root = project
        .join(".tmp")
        .join(format!("webhook-output-reader-{}", Uuid::new_v4()));
    let store = BackupObjectStore::filesystem(artifact_root.clone()).unwrap();
    eprintln!(
        "output artifact fixture: {}",
        artifact_root.strip_prefix(project).unwrap().display()
    );

    let job_id = Uuid::new_v4();
    let inline_tail = vec![b'x'; 8192];
    for (client, seq, stream, bytes) in [
        ("a", 0, "stdout", vec![0xe2]),
        ("a", 1, "status", b"status".to_vec()),
        ("a", 2, "stdout", vec![0x82, 0xac]),
        ("a", 3, "stderr", b"problem".to_vec()),
        ("a", 4, "stdout", inline_tail.clone()),
        ("b", 0, "stderr", b"b-error".to_vec()),
        ("b", 1, "stdout", b"b-output".to_vec()),
        ("binary", 0, "stdout", vec![0xff, b'x', 0xf0]),
    ] {
        insert_inline(&mut connection, job_id, client, seq, stream, &bytes).await;
    }
    let mut external = vec![b'z'; 40_000];
    external.extend_from_slice(b"after-preview-tail");
    let object_key = "job-outputs/full-source.bin";
    store.put_new(object_key, &external).await.unwrap();
    insert_inline(
        &mut connection,
        job_id,
        "c",
        0,
        "stdout",
        &external[..32 * 1024],
    )
    .await;
    sqlx::query("UPDATE job_outputs SET storage='object_store',object_key=$2,data_size_bytes=$3,data_sha256_hex=$4 WHERE job_id=$1 AND client_id='c'")
        .bind(job_id).bind(object_key).bind(external.len() as i64).bind(payload_hash(&external))
        .execute(&mut connection).await.unwrap();

    let mut target = json!({"job":{"id":job_id,"target":{"client_id":"a"}}});
    enrich_webhook_job_output_context(&mut connection, &mut target, "{job.output}", None)
        .await
        .unwrap();
    let a_stdout = format!("€{}", String::from_utf8(inline_tail).unwrap());
    assert_eq!(
        target["job"]["output"],
        json!({"stdout":a_stdout,"stderr":"problem"})
    );
    assert!(target["job"]["output"]["stdout"].as_str().unwrap().len() > 4096);
    // A helper sees the complete stream, not a pre-truncated view.
    assert_eq!(
        vpsman_common::render_template("{job.output.stdout.length}", &target).unwrap(),
        "8193"
    );

    let mut whole = json!({"job":{"id":job_id}});
    enrich_webhook_job_output_context(&mut connection, &mut whole, "{job.output}", Some(&store))
        .await
        .unwrap();
    assert_eq!(whole["job"]["output"]["stdout"]["a"], a_stdout);
    assert_eq!(whole["job"]["output"]["stdout"]["b"], "b-output");
    assert_eq!(
        whole["job"]["output"]["stdout"]["binary"],
        "\u{fffd}x\u{fffd}"
    );
    assert_eq!(
        whole["job"]["output"]["stdout"]["c"],
        String::from_utf8(external.clone()).unwrap()
    );
    assert_eq!(
        whole["job"]["output"]["stderr"],
        json!({"a":"problem","b":"b-error"})
    );

    let mut external_target = json!({"job":{"id":job_id,"target":{"client_id":"c"}}});
    enrich_webhook_job_output_context(
        &mut connection,
        &mut external_target,
        "{job.output.stdout}",
        Some(&store),
    )
    .await
    .unwrap();
    assert!(external_target["job"]["output"]["stdout"]
        .as_str()
        .unwrap()
        .ends_with("after-preview-tail"));
    assert_eq!(external_target["job"]["output"]["stderr"], "");
    let mut missing_store = json!({"job":{"id":job_id,"target":{"client_id":"c"}}});
    assert!(enrich_webhook_job_output_context(
        &mut connection,
        &mut missing_store,
        "{job.output.stdout}",
        None
    )
    .await
    .is_err());
    assert!(missing_store["job"].get("output").is_none());

    // Unrequested streams and targets must not fetch deleted artifacts.
    let selection_job = Uuid::new_v4();
    insert_inline(
        &mut connection,
        selection_job,
        "wanted",
        0,
        "stdout",
        b"selected",
    )
    .await;
    insert_inline(
        &mut connection,
        selection_job,
        "wanted",
        1,
        "stderr",
        b"preview",
    )
    .await;
    insert_inline(
        &mut connection,
        selection_job,
        "other",
        0,
        "stdout",
        b"preview",
    )
    .await;
    sqlx::query("UPDATE job_outputs SET storage='artifact_deleted' WHERE job_id=$1 AND (stream='stderr' OR client_id='other')")
        .bind(selection_job).execute(&mut connection).await.unwrap();
    let mut selected = json!({"job":{"id":selection_job}});
    enrich_webhook_job_output_context(
        &mut connection,
        &mut selected,
        "{job.output.stdout.wanted}",
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        selected["job"]["output"],
        json!({"stdout":{"wanted":"selected"},"stderr":{}})
    );
    let mut isolated = json!({"job":{"id":selection_job,"target":{"client_id":"wanted"}}});
    enrich_webhook_job_output_context(&mut connection, &mut isolated, "{job.output.stdout}", None)
        .await
        .unwrap();
    assert_eq!(
        isolated["job"]["output"],
        json!({"stdout":"selected","stderr":""})
    );
    let mut deleted = json!({"job":{"id":selection_job,"target":{"client_id":"wanted"}}});
    let error = enrich_webhook_job_output_context(
        &mut connection,
        &mut deleted,
        "{job.output.stderr}",
        Some(&store),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("deleted"));
    assert!(deleted["job"].get("output").is_none());

    let gap_job = Uuid::new_v4();
    insert_inline(&mut connection, gap_job, "gap", 0, "stdout", b"before").await;
    insert_inline(&mut connection, gap_job, "gap", 2, "stderr", b"after").await;
    let mut gap = json!({"job":{"id":gap_job}});
    assert!(enrich_webhook_job_output_context(
        &mut connection,
        &mut gap,
        "{job.output.stdout}",
        None
    )
    .await
    .unwrap_err()
    .to_string()
    .contains("incomplete"));
    assert!(gap["job"].get("output").is_none());

    sqlx::query("UPDATE job_outputs SET data_sha256_hex=$2 WHERE job_id=$1 AND client_id='c'")
        .bind(job_id)
        .bind(payload_hash(b"not-the-object"))
        .execute(&mut connection)
        .await
        .unwrap();
    let mut corrupt = json!({"job":{"id":job_id,"target":{"client_id":"c"}}});
    assert!(enrich_webhook_job_output_context(
        &mut connection,
        &mut corrupt,
        "{job.output.stdout}",
        Some(&store)
    )
    .await
    .unwrap_err()
    .to_string()
    .contains("hash mismatch"));
    assert!(corrupt["job"].get("output").is_none());

    let empty_job = Uuid::new_v4();
    for (event, expected) in [
        (
            json!({"job":{"id":empty_job}}),
            json!({"stdout":{},"stderr":{}}),
        ),
        (
            json!({"job":{"id":empty_job,"target":{"client_id":"missing"}}}),
            json!({"stdout":"","stderr":""}),
        ),
    ] {
        let mut event = event;
        enrich_webhook_job_output_context(&mut connection, &mut event, "{job.output}", None)
            .await
            .unwrap();
        assert_eq!(event["job"]["output"], expected);
    }
    let mut invalid_target = json!({"job":{"id":job_id,"target":{}}});
    assert!(enrich_webhook_job_output_context(
        &mut connection,
        &mut invalid_target,
        "{job.output}",
        None
    )
    .await
    .is_err());

    // These calls must not touch output storage, even with a job identity.
    sqlx::query("DROP TABLE pg_temp.job_outputs")
        .execute(&mut connection)
        .await
        .unwrap();
    let mut non_job = json!({"alert":{"id":"alert-only"}});
    enrich_webhook_job_output_context(&mut connection, &mut non_job, "{job.output}", None)
        .await
        .unwrap();
    assert_eq!(non_job, json!({"alert":{"id":"alert-only"}}));
    let mut metadata = json!({"job":{"id":job_id}});
    enrich_webhook_job_output_context(
        &mut connection,
        &mut metadata,
        "{job.id} {# {job.output} #}",
        None,
    )
    .await
    .unwrap();
    assert_eq!(metadata, json!({"job":{"id":job_id}}));
}
