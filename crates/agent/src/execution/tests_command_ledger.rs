use super::*;

#[tokio::test]
async fn command_ledger_records_and_loads_terminal_result() {
    let root = std::env::temp_dir().join(format!("vpsman-agent-command-ledger-{}", Uuid::new_v4()));
    let ledger = CommandLedger::open_at(root.clone()).await.unwrap();
    let job_id = Uuid::new_v4();
    let output = SequencedCommandOutput {
        seq: 3,
        output: CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: b"{\"type\":\"ok\"}".to_vec(),
            exit_code: Some(0),
            done: true,
        },
    };
    ledger
        .record(
            job_id,
            "a".repeat(64),
            compact_ledger_terminal_output(Some(output)),
            true,
        )
        .await
        .unwrap();
    let loaded = ledger.lookup(job_id).await.unwrap().unwrap();
    assert_eq!(loaded.job_id, job_id);
    assert_eq!(loaded.payload_hash, "a".repeat(64));
    let terminal = loaded.terminal_output.unwrap().output;
    assert!(terminal.done);
    assert_eq!(terminal.exit_code, Some(75));
    let status: serde_json::Value = serde_json::from_slice(&terminal.data).unwrap();
    assert_eq!(status["type"], "duplicate_job_replay_unavailable");
    assert_eq!(status["status"], "failed");
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn command_ledger_record_produces_cleanup_for_the_owned_consumer() {
    let root = std::env::temp_dir().join(format!("vpsman-agent-command-ledger-{}", Uuid::new_v4()));
    let ledger = CommandLedger::open_at(root.clone()).await.unwrap();
    let expired_path = root.join(format!("{}.json", Uuid::new_v4()));
    tokio::fs::write(&expired_path, b"expired").await.unwrap();
    std::fs::File::options()
        .write(true)
        .open(&expired_path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(UNIX_EPOCH))
        .unwrap();

    ledger
        .record(Uuid::new_v4(), "b".repeat(64), None, false)
        .await
        .unwrap();
    assert!(
        expired_path.exists(),
        "record must not consume global cleanup"
    );

    let consumer_ledger = ledger.clone();
    let consumer = tokio::spawn(async move { consumer_ledger.run_cleanup_consumer().await });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while expired_path.exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    consumer.abort();
    let _ = consumer.await;
    let _ = tokio::fs::remove_dir_all(root).await;
}
