use super::*;

const TEST_SHELL: &str = "/bin/sh";

#[tokio::test]
async fn missing_runtime_command_returns_failed_report() {
    let executable = std::env::current_dir().unwrap().join(format!(
        "vpsman-missing-runtime-command-{}",
        uuid::Uuid::new_v4()
    ));
    let argv = vec![executable.to_string_lossy().into_owned()];

    for (mutates, required) in [(true, false), (false, true)] {
        let report = run_runtime_command_cancelable(
            "test_runtime_spawn_failure",
            &argv,
            mutates,
            required,
            5,
            1024,
            CommandCancelToken::default(),
        )
        .await
        .expect("a launch failure should be reported as a failed command");

        assert_eq!(report["label"], "test_runtime_spawn_failure");
        assert_eq!(report["argv"], serde_json::json!(argv));
        assert_eq!(report["mutates"], mutates);
        assert_eq!(report["required"], required);
        assert_eq!(report["skipped"], false);
        assert_eq!(report["success"], false);
        assert!(report["exit_code"].is_null());
        assert_eq!(report["timed_out"], false);
        assert_eq!(report["killed_for_output_limit"], false);
        assert!(report["error"]
            .as_str()
            .and_then(|error| error
                .strip_prefix("failed to run runtime tunnel command test_runtime_spawn_failure: "))
            .is_some_and(|cause| !cause.is_empty()));
        for stream in ["stdout", "stderr"] {
            assert_eq!(
                report[stream],
                serde_json::json!({
                    "text": "",
                    "base64": null,
                    "bytes": 0,
                    "truncated": false,
                })
            );
        }
    }
}

#[tokio::test]
async fn nonzero_runtime_command_returns_failed_report_with_exit_code() {
    let argv = vec![
        TEST_SHELL.to_string(),
        "-c".to_string(),
        "printf 'command output'; printf 'command error' >&2; exit 7".to_string(),
    ];
    let report = run_runtime_command_cancelable(
        "test_runtime_nonzero_exit",
        &argv,
        true,
        true,
        5,
        1024,
        CommandCancelToken::default(),
    )
    .await
    .expect("a nonzero exit should remain a failed command report");

    assert_eq!(report["label"], "test_runtime_nonzero_exit");
    assert_eq!(report["success"], false);
    assert_eq!(report["exit_code"], 7);
    assert_eq!(report["timed_out"], false);
    assert_eq!(report["killed_for_output_limit"], false);
    assert!(report["error"].is_null());
    assert_eq!(report["stdout"]["text"], "command output");
    assert_eq!(report["stderr"]["text"], "command error");
}

#[tokio::test]
async fn cancellation_before_spawn_remains_typed_error() {
    let executable = std::env::current_dir().unwrap().join(format!(
        "vpsman-canceled-runtime-command-{}",
        uuid::Uuid::new_v4()
    ));
    let argv = vec![executable.to_string_lossy().into_owned()];
    let cancel_token = CommandCancelToken::default();
    cancel_token.cancel("operator canceled before launch".to_string());

    let error = run_runtime_command_cancelable(
        "test_runtime_pre_canceled",
        &argv,
        true,
        true,
        5,
        1024,
        cancel_token,
    )
    .await
    .expect_err("cancellation must not become a launch failure report");
    let canceled = error
        .downcast_ref::<CommandCanceled>()
        .expect("pre-launch cancellation should return CommandCanceled");
    assert_eq!(canceled.reason(), "operator canceled before launch");
}

#[tokio::test]
async fn cancellation_kills_runtime_command_process_group_children() {
    let root = std::env::temp_dir().join(format!(
        "vpsman-runtime-command-cancel-{}",
        uuid::Uuid::new_v4()
    ));
    tokio::fs::create_dir_all(&root).await.unwrap();
    let pid_file = root.join("child.pid");
    let argv = vec![
        TEST_SHELL.to_string(),
        "-lc".to_string(),
        format!("sleep 30 & echo $! > '{}'; wait", pid_file.display()),
    ];
    let cancel_token = CommandCancelToken::default();
    let task_cancel_token = cancel_token.clone();
    let task = tokio::spawn(async move {
        run_runtime_command_cancelable(
            "test_runtime_cancel",
            &argv,
            true,
            true,
            60,
            1024,
            task_cancel_token,
        )
        .await
    });
    let child_pid = wait_for_pid_file(&pid_file).await;
    assert!(process_running(child_pid));

    cancel_token.cancel("operator requested cancellation".to_string());
    let error = task.await.unwrap().unwrap_err();
    let canceled = error
        .downcast_ref::<CommandCanceled>()
        .expect("runtime command should return CommandCanceled");
    assert_eq!(canceled.reason(), "operator requested cancellation");

    for _ in 0..40 {
        if !process_running(child_pid) {
            break;
        }
        time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        !process_running(child_pid),
        "runtime child pid {child_pid} survived cancellation"
    );
    let _ = tokio::fs::remove_dir_all(root).await;
}

async fn wait_for_pid_file(path: &std::path::Path) -> u32 {
    for _ in 0..40 {
        if let Ok(contents) = tokio::fs::read_to_string(path).await {
            if let Ok(pid) = contents.trim().parse::<u32>() {
                return pid;
            }
        }
        time::sleep(Duration::from_millis(25)).await;
    }
    panic!("pid file {} was not written", path.display());
}

fn process_running(pid: u32) -> bool {
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}
