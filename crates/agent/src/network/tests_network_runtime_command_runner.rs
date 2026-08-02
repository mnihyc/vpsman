use super::*;

const TEST_SHELL: &str = "/bin/sh";

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
