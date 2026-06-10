mod tests {
    use crate::executor::{
        execute_job_command, execute_job_command_with_config_and_output_sink,
        execute_job_command_with_output_sink,
    };
    use std::{io::Cursor, os::unix::fs::PermissionsExt};
    use tokio::sync::mpsc;
    use vpsman_common::{
        payload_hash, AgentConfig, AgentExecutionConfig, AgentExecutionEnvironmentPolicy,
        AgentExecutionProcessCleanupPolicy, AgentExecutionPtyPolicy, AgentProcessInventorySource,
        AgentUserSessionsSource, FileExistingPolicy, FileOwnershipPolicy, FilePushChunk,
        JobCommand, OutputStream, RuntimeTunnelCommand,
    };

    #[tokio::test]
    async fn execute_argv_command_captures_output_and_status() {
        let job_id = uuid::Uuid::new_v4();
        let outputs = execute_job_command(
            job_id,
            &JobCommand::Shell {
                argv: vec!["/bin/echo".to_string(), "hello".to_string()],
                pty: false,
            },
            5,
        )
        .await
        .unwrap();

        assert!(outputs
            .iter()
            .any(|output| output.stream == OutputStream::Stdout && output.data == b"hello\n"));
        assert!(outputs
            .iter()
            .any(|output| output.done && output.job_id == job_id && output.exit_code == Some(0)));
    }

    #[tokio::test]
    async fn execute_pty_argv_command_uses_pty_stream() {
        let outputs = execute_job_command(
            uuid::Uuid::new_v4(),
            &JobCommand::Shell {
                argv: vec![
                    "/bin/sh".to_string(),
                    "-lc".to_string(),
                    "test -t 1 && printf tty".to_string(),
                ],
                pty: true,
            },
            5,
        )
        .await
        .unwrap();

        assert!(outputs
            .iter()
            .any(|output| output.stream == OutputStream::Pty && output.data == b"tty"));
        let status = outputs.iter().find(|output| output.done).unwrap();
        let status: serde_json::Value = serde_json::from_slice(&status.data).unwrap();
        assert_eq!(status["type"], "shell_pty");
    }

    #[tokio::test]
    async fn terminal_session_accepts_input_resize_and_close() {
        let session_id = uuid::Uuid::new_v4();
        let open_job_id = uuid::Uuid::new_v4();
        let outputs = execute_job_command(
            open_job_id,
            &JobCommand::TerminalOpen {
                session_id,
                argv: vec![
                    "/bin/sh".to_string(),
                    "-lc".to_string(),
                    "read line; printf 'got:%s\\n' \"$line\"".to_string(),
                ],
                cwd: None,
                user: None,
                user_policy: vpsman_common::TerminalUserPolicy::Fail,
                cols: 120,
                rows: 40,
                replay_from_seq: None,
                idle_timeout_secs: 1800,
                flow_window_bytes: 65_536,
            },
            5,
        )
        .await
        .unwrap();

        let status = outputs
            .iter()
            .find(|output| output.done && output.stream == OutputStream::Status)
            .expect("terminal open status");
        assert_eq!(status.job_id, open_job_id);
        assert_eq!(status.exit_code, Some(0));
        let payload: serde_json::Value = serde_json::from_slice(&status.data).unwrap();
        assert_eq!(payload["type"], "terminal_open");
        assert_eq!(payload["status"], "opened");

        let resize_outputs = execute_job_command(
            uuid::Uuid::new_v4(),
            &JobCommand::TerminalResize {
                session_id,
                cols: 100,
                rows: 30,
            },
            5,
        )
        .await
        .unwrap();
        let resize_status = status_payload(&resize_outputs);
        assert_eq!(resize_status["type"], "terminal_resize");
        assert_eq!(resize_status["status"], "resized");

        let input_outputs = execute_job_command(
            uuid::Uuid::new_v4(),
            &JobCommand::TerminalInput {
                session_id,
                input_seq: 1,
                data_base64: vpsman_common::encode_inline_file_payload(b"hello\n").unwrap(),
            },
            5,
        )
        .await
        .unwrap();
        let pty_text = pty_text(&input_outputs);
        assert!(pty_text.contains("got:hello"), "{pty_text:?}");
        let input_status = status_payload(&input_outputs);
        assert_eq!(input_status["type"], "terminal_input");
        assert_eq!(input_status["status"], "accepted");
        assert_eq!(input_status["written_bytes"], 6);

        let close_outputs = execute_job_command(
            uuid::Uuid::new_v4(),
            &JobCommand::TerminalClose {
                session_id,
                reason: Some("test complete".to_string()),
            },
            5,
        )
        .await
        .unwrap();
        let close_status = status_payload(&close_outputs);
        assert_eq!(close_status["type"], "terminal_close");
        assert_eq!(close_status["status"], "closed");
        assert_eq!(close_status["cleanup"]["target_kind"], "process_group");
        assert_eq!(close_status["cleanup"]["final_running"], false);
    }

    #[tokio::test]
    async fn terminal_session_attach_replays_from_requested_cursor() {
        let session_id = uuid::Uuid::new_v4();
        let argv = vec![
            "/bin/sh".to_string(),
            "-lc".to_string(),
            "printf 'first-line\\nsecond-line\\n'; sleep 10".to_string(),
        ];
        let open_outputs = execute_job_command(
            uuid::Uuid::new_v4(),
            &JobCommand::TerminalOpen {
                session_id,
                argv: argv.clone(),
                cwd: None,
                user: None,
                user_policy: vpsman_common::TerminalUserPolicy::Fail,
                cols: 120,
                rows: 40,
                replay_from_seq: None,
                idle_timeout_secs: 30,
                flow_window_bytes: 65_536,
            },
            5,
        )
        .await
        .unwrap();
        assert!(pty_text(&open_outputs).contains("first-line"));
        let open_status = status_payload(&open_outputs);
        assert_eq!(open_status["status"], "opened");
        assert_eq!(open_status["output_replay_truncated"], false);
        assert!(open_status["output_retained_bytes"].as_u64().unwrap() > 0);

        let attach_outputs = execute_job_command(
            uuid::Uuid::new_v4(),
            &JobCommand::TerminalOpen {
                session_id,
                argv,
                cwd: None,
                user: None,
                user_policy: vpsman_common::TerminalUserPolicy::Fail,
                cols: 120,
                rows: 40,
                replay_from_seq: Some(1),
                idle_timeout_secs: 30,
                flow_window_bytes: 65_536,
            },
            5,
        )
        .await
        .unwrap();
        let attach_text = pty_text(&attach_outputs);
        assert!(attach_text.contains("first-line"), "{attach_text:?}");
        assert!(attach_text.contains("second-line"), "{attach_text:?}");
        let attach_status = status_payload(&attach_outputs);
        assert_eq!(attach_status["type"], "terminal_open");
        assert_eq!(attach_status["status"], "attached");
        assert_eq!(attach_status["output_first_seq"], 1);
        assert_eq!(attach_status["output_replay_truncated"], false);

        let _ = execute_job_command(
            uuid::Uuid::new_v4(),
            &JobCommand::TerminalClose {
                session_id,
                reason: Some("test cleanup".to_string()),
            },
            5,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn terminal_poll_collects_idle_output_without_input() {
        let session_id = uuid::Uuid::new_v4();
        let open_outputs = execute_job_command(
            uuid::Uuid::new_v4(),
            &JobCommand::TerminalOpen {
                session_id,
                argv: vec![
                    "/bin/sh".to_string(),
                    "-lc".to_string(),
                    "sleep 0.2; printf 'idle-terminal-output\\n'; sleep 10".to_string(),
                ],
                cwd: None,
                user: None,
                user_policy: vpsman_common::TerminalUserPolicy::Fail,
                cols: 120,
                rows: 40,
                replay_from_seq: None,
                idle_timeout_secs: 30,
                flow_window_bytes: 65_536,
            },
            5,
        )
        .await
        .unwrap();
        assert_eq!(status_payload(&open_outputs)["status"], "opened");

        tokio::time::sleep(std::time::Duration::from_millis(350)).await;
        let poll_outputs = execute_job_command(
            uuid::Uuid::new_v4(),
            &JobCommand::TerminalPoll {
                session_id,
                replay_from_seq: Some(1),
            },
            5,
        )
        .await
        .unwrap();
        let poll_text = pty_text(&poll_outputs);
        assert!(poll_text.contains("idle-terminal-output"), "{poll_text:?}");
        let poll_status = status_payload(&poll_outputs);
        assert_eq!(poll_status["type"], "terminal_poll");
        assert_eq!(poll_status["status"], "polled");
        assert_eq!(poll_status["replay_from_seq"], 1);
        assert_eq!(poll_status["output_replay_truncated"], false);

        let _ = execute_job_command(
            uuid::Uuid::new_v4(),
            &JobCommand::TerminalClose {
                session_id,
                reason: Some("test cleanup".to_string()),
            },
            5,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn terminal_flow_window_reports_retention_loss() {
        let session_id = uuid::Uuid::new_v4();
        let outputs = execute_job_command(
            uuid::Uuid::new_v4(),
            &JobCommand::TerminalOpen {
                session_id,
                argv: vec![
                    "/bin/sh".to_string(),
                    "-lc".to_string(),
                    "i=0; while [ \"$i\" -lt 700 ]; do printf 'terminal-window-line-%04d\\n' \"$i\"; i=$((i+1)); done; sleep 10".to_string(),
                ],
                cwd: None,
                user: None,
                user_policy: vpsman_common::TerminalUserPolicy::Fail,
                cols: 120,
                rows: 40,
                replay_from_seq: Some(1),
                idle_timeout_secs: 30,
                flow_window_bytes: 4096,
            },
            5,
        )
        .await
        .unwrap();
        let status = status_payload(&outputs);
        assert_eq!(status["type"], "terminal_open");
        assert_eq!(status["status"], "opened");
        assert!(status["output_retained_bytes"].as_u64().unwrap() <= 4096);
        assert!(status["output_dropped_bytes"].as_u64().unwrap() > 0);
        assert_eq!(status["output_replay_truncated"], true);

        let _ = execute_job_command(
            uuid::Uuid::new_v4(),
            &JobCommand::TerminalClose {
                session_id,
                reason: Some("test cleanup".to_string()),
            },
            5,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn terminal_idle_timeout_removes_session() {
        let session_id = uuid::Uuid::new_v4();
        let outputs = execute_job_command(
            uuid::Uuid::new_v4(),
            &JobCommand::TerminalOpen {
                session_id,
                argv: vec![
                    "/bin/sh".to_string(),
                    "-lc".to_string(),
                    "sleep 30".to_string(),
                ],
                cwd: None,
                user: None,
                user_policy: vpsman_common::TerminalUserPolicy::Fail,
                cols: 120,
                rows: 40,
                replay_from_seq: None,
                idle_timeout_secs: 1,
                flow_window_bytes: 65_536,
            },
            5,
        )
        .await
        .unwrap();
        assert_eq!(status_payload(&outputs)["status"], "opened");

        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        let input_outputs = execute_job_command(
            uuid::Uuid::new_v4(),
            &JobCommand::TerminalInput {
                session_id,
                input_seq: 1,
                data_base64: vpsman_common::encode_inline_file_payload(b"hello\n").unwrap(),
            },
            5,
        )
        .await
        .unwrap();
        let status = status_payload(&input_outputs);
        assert_eq!(status["type"], "terminal_input");
        assert_eq!(status["status"], "missing");
    }

    #[tokio::test]
    async fn terminal_input_missing_session_reports_typed_status() {
        let session_id = uuid::Uuid::new_v4();
        let outputs = execute_job_command(
            uuid::Uuid::new_v4(),
            &JobCommand::TerminalInput {
                session_id,
                input_seq: 1,
                data_base64: vpsman_common::encode_inline_file_payload(b"hello\n").unwrap(),
            },
            5,
        )
        .await
        .unwrap();

        let status = outputs
            .iter()
            .find(|output| output.done && output.stream == OutputStream::Status)
            .expect("terminal missing status");
        assert_eq!(status.exit_code, Some(125));
        let payload: serde_json::Value = serde_json::from_slice(&status.data).unwrap();
        assert_eq!(payload["type"], "terminal_input");
        assert_eq!(payload["status"], "missing");
    }

    #[tokio::test]
    async fn terminal_poll_missing_session_reports_typed_status() {
        let session_id = uuid::Uuid::new_v4();
        let outputs = execute_job_command(
            uuid::Uuid::new_v4(),
            &JobCommand::TerminalPoll {
                session_id,
                replay_from_seq: Some(1),
            },
            5,
        )
        .await
        .unwrap();

        let status = status_payload(&outputs);
        assert_eq!(status["type"], "terminal_poll");
        assert_eq!(status["status"], "missing");
        assert_eq!(status["session_id"], session_id.to_string());
    }

    #[tokio::test]
    async fn execute_argv_command_reports_typed_timeout_status() {
        let job_id = uuid::Uuid::new_v4();
        let outputs = execute_job_command(
            job_id,
            &JobCommand::Shell {
                argv: vec![
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    "sleep 2".to_string(),
                ],
                pty: false,
            },
            1,
        )
        .await
        .unwrap();

        let status = outputs
            .iter()
            .find(|output| output.done && output.stream == OutputStream::Status)
            .expect("timeout status output");
        assert_eq!(status.exit_code, Some(124));
        let payload: serde_json::Value = serde_json::from_slice(&status.data).unwrap();
        assert_eq!(payload["type"], "command_timeout");
        assert_eq!(payload["timeout_secs"], 1);
        assert_eq!(payload["cleanup"]["target_kind"], "process_group");
        assert_eq!(payload["cleanup"]["graceful_signal"], "SIGTERM");
        assert_eq!(payload["cleanup"]["final_running"], false);
    }

    #[tokio::test]
    async fn execute_shell_script_runs_through_system_shell() {
        let job_id = uuid::Uuid::new_v4();
        let outputs = execute_job_command(
            job_id,
            &JobCommand::ShellScript {
                script: "printf '%s' vpsman-shell-script".to_string(),
            },
            5,
        )
        .await
        .unwrap();

        assert!(outputs.iter().any(|output| {
            output.stream == OutputStream::Stdout && output.data == b"vpsman-shell-script"
        }));
        let status = outputs
            .iter()
            .find(|output| output.done && output.stream == OutputStream::Status)
            .unwrap();
        assert_eq!(status.exit_code, Some(0));
        let payload: serde_json::Value = serde_json::from_slice(&status.data).unwrap();
        assert_eq!(payload["type"], "shell_script");
        assert_eq!(payload["shell"], "/bin/sh");
    }

    #[tokio::test]
    async fn execute_shell_script_uses_configured_shell_prefix() {
        let mut config = AgentConfig::default();
        config.execution.shell_script_argv = vec!["/bin/sh".to_string(), "-c".to_string()];
        let outputs = execute_job_command_with_config_and_output_sink(
            &config,
            uuid::Uuid::new_v4(),
            &JobCommand::ShellScript {
                script: "printf configured-shell".to_string(),
            },
            5,
            None,
        )
        .await
        .unwrap();

        assert!(outputs.iter().any(|output| {
            output.stream == OutputStream::Stdout && output.data == b"configured-shell"
        }));
        let status = status_payload(&outputs);
        assert_eq!(status["type"], "shell_script");
        assert_eq!(status["shell"], "/bin/sh");
        assert!(status["shell_argv_prefix_sha256_hex"].as_str().is_some());
    }

    #[tokio::test]
    async fn execute_shell_script_applies_execution_environment_policy() {
        let job_id = uuid::Uuid::new_v4();
        let root = std::env::temp_dir().join(format!("vpsman-exec-policy-{job_id}"));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let mut config = AgentConfig::default();
        config.execution.working_directory = Some(root.to_string_lossy().to_string());
        config.execution.environment_policy = AgentExecutionEnvironmentPolicy::Clean;
        config
            .execution
            .environment_set
            .insert("VPSMAN_EXECUTION_MODE".to_string(), "batch".to_string());

        let outputs = execute_job_command_with_config_and_output_sink(
            &config,
            job_id,
            &JobCommand::ShellScript {
                script: "printf '%s:%s' \"$PWD\" \"$VPSMAN_EXECUTION_MODE\"".to_string(),
            },
            5,
            None,
        )
        .await
        .unwrap();

        let expected = format!("{}:batch", root.display());
        assert!(outputs.iter().any(|output| {
            output.stream == OutputStream::Stdout && output.data == expected.as_bytes()
        }));
        let status = status_payload(&outputs);
        assert_eq!(status["working_directory"], root.to_string_lossy().as_ref());
        assert_eq!(status["environment_policy"], "clean");
    }

    #[tokio::test]
    async fn execute_pty_argv_respects_disabled_pty_policy() {
        let mut config = AgentConfig::default();
        config.execution.pty_policy = AgentExecutionPtyPolicy::Disabled;
        let error = execute_job_command_with_config_and_output_sink(
            &config,
            uuid::Uuid::new_v4(),
            &JobCommand::Shell {
                argv: vec!["/bin/echo".to_string(), "blocked".to_string()],
                pty: true,
            },
            5,
            None,
        )
        .await
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("execution PTY policy is disabled"));
    }

    #[tokio::test]
    async fn execute_argv_timeout_can_use_direct_child_cleanup_policy() {
        let mut config = AgentConfig::default();
        config.execution.process_cleanup = AgentExecutionProcessCleanupPolicy::DirectChild;
        let outputs = execute_job_command_with_config_and_output_sink(
            &config,
            uuid::Uuid::new_v4(),
            &JobCommand::Shell {
                argv: vec![
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    "sleep 2".to_string(),
                ],
                pty: false,
            },
            1,
            None,
        )
        .await
        .unwrap();
        let status = status_payload(&outputs);
        assert_eq!(status["type"], "command_timeout");
        assert_eq!(status["cleanup"]["target_kind"], "process");
    }

    #[tokio::test]
    async fn execute_shell_script_reports_typed_timeout_status() {
        let job_id = uuid::Uuid::new_v4();
        let outputs = execute_job_command(
            job_id,
            &JobCommand::ShellScript {
                script: "sleep 2".to_string(),
            },
            1,
        )
        .await
        .unwrap();

        let status = outputs
            .iter()
            .find(|output| output.done && output.stream == OutputStream::Status)
            .expect("timeout status output");
        assert_eq!(status.exit_code, Some(124));
        let payload: serde_json::Value = serde_json::from_slice(&status.data).unwrap();
        assert_eq!(payload["type"], "command_timeout");
        assert_eq!(payload["mode"], "shell_script");
        assert_eq!(payload["timeout_secs"], 1);
        assert_eq!(payload["cleanup"]["target_kind"], "process_group");
        assert_eq!(payload["cleanup"]["graceful_signal"], "SIGTERM");
        assert_eq!(payload["cleanup"]["final_running"], false);
    }

    #[tokio::test]
    async fn execute_file_pull_returns_chunks_and_hash_status() {
        let job_id = uuid::Uuid::new_v4();
        let path = std::env::temp_dir().join(format!("vpsman-agent-test-{job_id}"));
        tokio::fs::write(&path, b"file contents").await.unwrap();

        let outputs = execute_job_command(
            job_id,
            &JobCommand::FilePull {
                path: path.to_string_lossy().to_string(),
            },
            5,
        )
        .await
        .unwrap();
        let _ = tokio::fs::remove_file(&path).await;

        assert!(
            outputs
                .iter()
                .any(|output| output.stream == OutputStream::Stdout
                    && output.data == b"file contents")
        );
        let status = outputs.iter().find(|output| output.done).unwrap();
        let status: serde_json::Value = serde_json::from_slice(&status.data).unwrap();
        assert_eq!(status["type"], "file_pull");
        assert_eq!(status["size_bytes"], 13);
        assert_eq!(status["sha256_hex"], payload_hash(b"file contents"));
    }

    #[tokio::test]
    async fn execute_file_pull_streams_chunks_when_sink_is_available() {
        let job_id = uuid::Uuid::new_v4();
        let path = std::env::temp_dir().join(format!("vpsman-agent-stream-pull-{job_id}"));
        let data = vec![b'x'; 70 * 1024];
        tokio::fs::write(&path, &data).await.unwrap();
        let (tx, mut rx) = mpsc::channel(4);

        let outputs = execute_job_command_with_output_sink(
            job_id,
            &JobCommand::FilePull {
                path: path.to_string_lossy().to_string(),
            },
            5,
            Some(tx),
        )
        .await
        .unwrap();
        let _ = tokio::fs::remove_file(&path).await;

        let mut streamed = Vec::new();
        while let Some(output) = rx.recv().await {
            assert_eq!(output.stream, OutputStream::Stdout);
            assert!(!output.done);
            streamed.extend_from_slice(&output.data);
        }
        assert_eq!(streamed, data);
        assert!(outputs
            .iter()
            .all(|output| output.stream == OutputStream::Status));
        let status: serde_json::Value = serde_json::from_slice(&outputs[0].data).unwrap();
        assert_eq!(status["type"], "file_pull");
        assert_eq!(status["size_bytes"], 70 * 1024);
        assert_eq!(status["sha256_hex"], payload_hash(&data));
        assert_eq!(status["chunk_count"], 2);
        assert_eq!(status["streamed"], true);
    }

    #[tokio::test]
    async fn execute_file_pull_rejects_relative_paths() {
        let error = execute_job_command(
            uuid::Uuid::new_v4(),
            &JobCommand::FilePull {
                path: "relative/path".to_string(),
            },
            5,
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("file path must be absolute"));
    }

    #[tokio::test]
    async fn execute_file_download_regular_file_returns_bytes_and_status() {
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-download-file-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("download.txt");
        let data = b"download me";
        tokio::fs::write(&path, data).await.unwrap();

        let outputs = execute_job_command(
            job_id,
            &JobCommand::FileDownload {
                path: path.to_string_lossy().to_string(),
                max_bytes: 1024,
            },
            5,
        )
        .await
        .unwrap();

        assert_eq!(stdout_bytes(&outputs), data);
        let status = status_payload(&outputs);
        assert_eq!(status["type"], "file_download");
        assert_eq!(status["source_kind"], "file");
        assert_eq!(status["filename"], "download.txt");
        assert_eq!(status["content_type"], "application/octet-stream");
        assert_eq!(status["size_bytes"], data.len());
        assert_eq!(status["sha256_hex"], payload_hash(data));
        assert_eq!(status["archive"], false);
        assert!(status.get("hierarchy_sha256_hex").is_none());

        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn execute_file_download_directory_returns_tar_archive() {
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-download-dir-{job_id}"));
        tokio::fs::create_dir_all(dir.join("nested")).await.unwrap();
        tokio::fs::write(dir.join("nested/app.conf"), b"listen=443\n")
            .await
            .unwrap();

        let outputs = execute_job_command(
            job_id,
            &JobCommand::FileDownload {
                path: dir.to_string_lossy().to_string(),
                max_bytes: 1024 * 1024,
            },
            5,
        )
        .await
        .unwrap();

        let status = status_payload(&outputs);
        assert_eq!(status["type"], "file_download");
        assert_eq!(status["source_kind"], "directory");
        assert_eq!(status["content_type"], "application/x-tar");
        assert_eq!(status["archive"], true);
        assert_eq!(status["file_count"], 1);
        assert_eq!(status["directory_count"], 1);
        assert_eq!(status["manifest_truncated"], false);
        assert_eq!(status["manifest_entry_count"], 2);
        assert!(status["hierarchy_sha256_hex"]
            .as_str()
            .is_some_and(|value| value.len() == 64));
        assert!(status["content_manifest_sha256_hex"]
            .as_str()
            .is_some_and(|value| value.len() == 64));
        let manifest_entries = status["manifest_entries"].as_array().unwrap();
        assert!(manifest_entries
            .iter()
            .any(|entry| { entry["path"] == "nested" && entry["kind"] == "directory" }));
        assert!(manifest_entries.iter().any(|entry| {
            entry["path"] == "nested/app.conf"
                && entry["kind"] == "file"
                && entry["sha256_hex"] == payload_hash(b"listen=443\n")
        }));
        let archive_bytes = stdout_bytes(&outputs);
        let mut archive = tar::Archive::new(Cursor::new(archive_bytes));
        let names: Vec<String> = archive
            .entries()
            .unwrap()
            .map(|entry| entry.unwrap().path().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.iter().any(|name| name.ends_with("nested/app.conf")));

        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn execute_file_push_writes_hash_verified_payload_atomically() {
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-file-push-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("pushed.txt");
        let data = b"pushed contents";
        let outputs = execute_job_command(
            job_id,
            &JobCommand::FilePush {
                path: path.to_string_lossy().to_string(),
                mode: 0o640,
                size_bytes: data.len() as u64,
                sha256_hex: payload_hash(data),
                data_base64: vpsman_common::encode_inline_file_payload(data).unwrap(),
                existing_policy: FileExistingPolicy::Replace,
                owner: None,
                group: None,
                uid: None,
                gid: None,
                ownership_policy: FileOwnershipPolicy::Fail,
            },
            5,
        )
        .await
        .unwrap();

        assert_eq!(tokio::fs::read(&path).await.unwrap(), data);
        assert_eq!(
            tokio::fs::metadata(&path)
                .await
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
        let status = outputs.iter().find(|output| output.done).unwrap();
        let status: serde_json::Value = serde_json::from_slice(&status.data).unwrap();
        assert_eq!(status["type"], "file_push");
        assert_eq!(status["size_bytes"], data.len());
        assert_eq!(status["sha256_hex"], payload_hash(data));

        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn execute_file_push_can_refuse_existing_destination() {
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-file-push-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("pushed.txt");
        tokio::fs::write(&path, b"original").await.unwrap();
        let data = b"replacement";
        let outputs = execute_job_command(
            job_id,
            &JobCommand::FilePush {
                path: path.to_string_lossy().to_string(),
                mode: 0o640,
                size_bytes: data.len() as u64,
                sha256_hex: payload_hash(data),
                data_base64: vpsman_common::encode_inline_file_payload(data).unwrap(),
                existing_policy: FileExistingPolicy::Skip,
                owner: None,
                group: None,
                uid: None,
                gid: None,
                ownership_policy: FileOwnershipPolicy::Fail,
            },
            5,
        )
        .await
        .unwrap();

        assert_eq!(tokio::fs::read(&path).await.unwrap(), b"original");
        let status = outputs.iter().find(|output| output.done).unwrap();
        let status: serde_json::Value = serde_json::from_slice(&status.data).unwrap();
        assert_eq!(status["type"], "file_push");
        assert_eq!(status["status"], "skipped");
        assert_eq!(status["reason"], "destination_exists");
        assert_eq!(status["overwrite_policy"], "skip");
        assert_eq!(status["ownership_status"], "unchanged");

        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn execute_file_push_missing_owner_fail_policy_fails_before_placement() {
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-file-push-owner-fail-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("pushed.txt");
        let data = b"owned contents";

        let error = execute_job_command(
            job_id,
            &JobCommand::FilePush {
                path: path.to_string_lossy().to_string(),
                mode: 0o600,
                size_bytes: data.len() as u64,
                sha256_hex: payload_hash(data),
                data_base64: vpsman_common::encode_inline_file_payload(data).unwrap(),
                existing_policy: FileExistingPolicy::Replace,
                owner: Some(format!("missing-vpsman-user-{job_id}")),
                group: None,
                uid: None,
                gid: None,
                ownership_policy: FileOwnershipPolicy::Fail,
            },
            5,
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("missing owner/group"));
        assert!(!path.exists());
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn execute_file_push_missing_owner_ignore_policy_uploads_without_chown() {
        let job_id = uuid::Uuid::new_v4();
        let dir =
            std::env::temp_dir().join(format!("vpsman-agent-file-push-owner-ignore-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("pushed.txt");
        let data = b"owned contents";

        let outputs = execute_job_command(
            job_id,
            &JobCommand::FilePush {
                path: path.to_string_lossy().to_string(),
                mode: 0o600,
                size_bytes: data.len() as u64,
                sha256_hex: payload_hash(data),
                data_base64: vpsman_common::encode_inline_file_payload(data).unwrap(),
                existing_policy: FileExistingPolicy::Replace,
                owner: Some(format!("missing-vpsman-user-{job_id}")),
                group: None,
                uid: None,
                gid: None,
                ownership_policy: FileOwnershipPolicy::Ignore,
            },
            5,
        )
        .await
        .unwrap();

        assert_eq!(tokio::fs::read(&path).await.unwrap(), data);
        let status = status_payload(&outputs);
        assert_eq!(status["status"], "completed");
        assert_eq!(status["ownership_status"], "skipped");
        assert_eq!(status["ownership_reason"], "missing_owner_or_group");
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn execute_chunked_file_push_validates_chunks_and_writes_atomically() {
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-file-push-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("pushed.bin");
        let data = vec![7_u8; vpsman_common::MAX_INLINE_FILE_PUSH_BYTES + 17];
        let outputs = execute_job_command(
            job_id,
            &JobCommand::FilePushChunked {
                path: path.to_string_lossy().to_string(),
                mode: 0o600,
                size_bytes: data.len() as u64,
                sha256_hex: payload_hash(&data),
                chunks: vpsman_common::encode_chunked_file_payload(&data).unwrap(),
                existing_policy: FileExistingPolicy::Replace,
                owner: None,
                group: None,
                uid: None,
                gid: None,
                ownership_policy: FileOwnershipPolicy::Fail,
            },
            5,
        )
        .await
        .unwrap();

        assert_eq!(tokio::fs::read(&path).await.unwrap(), data);
        assert_eq!(
            tokio::fs::metadata(&path)
                .await
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let status = outputs.iter().find(|output| output.done).unwrap();
        let status: serde_json::Value = serde_json::from_slice(&status.data).unwrap();
        assert_eq!(status["type"], "file_push_chunked");
        assert_eq!(status["size_bytes"], data.len());
        assert_eq!(status["sha256_hex"], payload_hash(&data));
        assert!(status["chunk_count"].as_u64().unwrap() > 1);

        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn execute_resumable_file_transfer_ack_resume_and_commit() {
        let session_id = uuid::Uuid::new_v4();
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-resume-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("resumed.bin");
        let data = b"resumable transfer contents";
        let token_hash = payload_hash(b"resume-token");

        let start_outputs = execute_job_command(
            job_id,
            &JobCommand::FileTransferStart {
                session_id,
                path: path.to_string_lossy().to_string(),
                mode: 0o640,
                size_bytes: data.len() as u64,
                sha256_hex: payload_hash(data),
                chunk_size_bytes: 16,
                rate_limit_kbps: 0,
                existing_policy: FileExistingPolicy::Replace,
                resume_token_hash: token_hash.clone(),
            },
            5,
        )
        .await
        .unwrap();
        assert_transfer_next_offset(&start_outputs, "file_transfer_start", 0);

        let first = transfer_chunk(0, &data[..16]);
        let chunk_outputs = execute_job_command(
            job_id,
            &JobCommand::FileTransferChunk {
                session_id,
                offset: 0,
                chunk: first.clone(),
                resume_token_hash: token_hash.clone(),
            },
            5,
        )
        .await
        .unwrap();
        assert_transfer_next_offset(&chunk_outputs, "file_transfer_chunk_ack", 16);

        let duplicate_outputs = execute_job_command(
            job_id,
            &JobCommand::FileTransferChunk {
                session_id,
                offset: 0,
                chunk: first,
                resume_token_hash: token_hash.clone(),
            },
            5,
        )
        .await
        .unwrap();
        assert_transfer_next_offset(&duplicate_outputs, "file_transfer_chunk_ack", 16);

        let resumed_start = execute_job_command(
            job_id,
            &JobCommand::FileTransferStart {
                session_id,
                path: path.to_string_lossy().to_string(),
                mode: 0o640,
                size_bytes: data.len() as u64,
                sha256_hex: payload_hash(data),
                chunk_size_bytes: 16,
                rate_limit_kbps: 0,
                existing_policy: FileExistingPolicy::Replace,
                resume_token_hash: token_hash.clone(),
            },
            5,
        )
        .await
        .unwrap();
        assert_transfer_next_offset(&resumed_start, "file_transfer_start", 16);

        let second = transfer_chunk(16, &data[16..]);
        let second_outputs = execute_job_command(
            job_id,
            &JobCommand::FileTransferChunk {
                session_id,
                offset: 16,
                chunk: second,
                resume_token_hash: token_hash.clone(),
            },
            5,
        )
        .await
        .unwrap();
        assert_transfer_next_offset(
            &second_outputs,
            "file_transfer_chunk_ack",
            data.len() as u64,
        );

        let commit_outputs = execute_job_command(
            job_id,
            &JobCommand::FileTransferCommit {
                session_id,
                resume_token_hash: token_hash,
            },
            5,
        )
        .await
        .unwrap();
        assert_transfer_next_offset(&commit_outputs, "file_transfer_commit", data.len() as u64);

        assert_eq!(tokio::fs::read(&path).await.unwrap(), data);
        assert_eq!(
            tokio::fs::metadata(&path)
                .await
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o640
        );

        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn execute_resumable_file_transfer_skip_policy_does_not_replace_existing_file() {
        let session_id = uuid::Uuid::new_v4();
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-resume-skip-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("existing.bin");
        tokio::fs::write(&path, b"keep").await.unwrap();
        let data = b"replacement";
        let token_hash = payload_hash(b"resume-token");

        let start_outputs = execute_job_command(
            job_id,
            &JobCommand::FileTransferStart {
                session_id,
                path: path.to_string_lossy().to_string(),
                mode: 0o640,
                size_bytes: data.len() as u64,
                sha256_hex: payload_hash(data),
                chunk_size_bytes: 16,
                rate_limit_kbps: 0,
                existing_policy: FileExistingPolicy::Skip,
                resume_token_hash: token_hash.clone(),
            },
            5,
        )
        .await
        .unwrap();
        let status = status_payload(&start_outputs);
        assert_eq!(status["type"], "file_transfer_start");
        assert_eq!(status["next_offset"], data.len() as u64);
        assert_eq!(status["extra"]["skipped"], true);
        assert_eq!(tokio::fs::read(&path).await.unwrap(), b"keep");

        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn execute_resumable_file_transfer_skip_policy_refuses_commit_race() {
        let session_id = uuid::Uuid::new_v4();
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-resume-race-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("race.bin");
        let data = b"replacement";
        let token_hash = payload_hash(b"resume-token");

        let start_outputs = execute_job_command(
            job_id,
            &JobCommand::FileTransferStart {
                session_id,
                path: path.to_string_lossy().to_string(),
                mode: 0o640,
                size_bytes: data.len() as u64,
                sha256_hex: payload_hash(data),
                chunk_size_bytes: 16,
                rate_limit_kbps: 0,
                existing_policy: FileExistingPolicy::Skip,
                resume_token_hash: token_hash.clone(),
            },
            5,
        )
        .await
        .unwrap();
        assert_transfer_next_offset(&start_outputs, "file_transfer_start", 0);

        let chunk_outputs = execute_job_command(
            job_id,
            &JobCommand::FileTransferChunk {
                session_id,
                offset: 0,
                chunk: transfer_chunk(0, data),
                resume_token_hash: token_hash.clone(),
            },
            5,
        )
        .await
        .unwrap();
        assert_transfer_next_offset(&chunk_outputs, "file_transfer_chunk_ack", data.len() as u64);
        tokio::fs::write(&path, b"raced").await.unwrap();

        let commit_result = execute_job_command(
            job_id,
            &JobCommand::FileTransferCommit {
                session_id,
                resume_token_hash: token_hash,
            },
            5,
        )
        .await;
        assert!(commit_result.unwrap_err().to_string().contains("move file"));
        assert_eq!(tokio::fs::read(&path).await.unwrap(), b"raced");

        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn execute_resumable_file_download_start_and_chunks() {
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-file-download-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("source.txt");
        let data = b"download chunks across a resumable session";
        tokio::fs::write(&path, data).await.unwrap();
        let session_id = uuid::Uuid::new_v4();
        let token_hash = payload_hash(b"download-token");

        let start_outputs = execute_job_command(
            job_id,
            &JobCommand::FileTransferDownloadStart {
                session_id,
                path: path.to_string_lossy().to_string(),
                chunk_size_bytes: 64,
                rate_limit_kbps: 0,
                resume_token_hash: token_hash.clone(),
            },
            5,
        )
        .await
        .unwrap();
        assert_transfer_next_offset(&start_outputs, "file_transfer_download_start", 0);
        let start_status = status_payload(&start_outputs);
        assert_eq!(start_status["extra"]["sha256_hex"], payload_hash(data));

        let first_outputs = execute_job_command(
            job_id,
            &JobCommand::FileTransferDownloadChunk {
                session_id,
                offset: 0,
                max_bytes: 12,
                resume_token_hash: token_hash.clone(),
            },
            5,
        )
        .await
        .unwrap();
        assert_transfer_next_offset(&first_outputs, "file_transfer_download_chunk", 12);
        assert_eq!(stdout_bytes(&first_outputs), data[..12]);

        let second_outputs = execute_job_command(
            job_id,
            &JobCommand::FileTransferDownloadChunk {
                session_id,
                offset: 12,
                max_bytes: 64,
                resume_token_hash: token_hash,
            },
            5,
        )
        .await
        .unwrap();
        assert_transfer_next_offset(
            &second_outputs,
            "file_transfer_download_chunk",
            data.len() as u64,
        );
        assert_eq!(stdout_bytes(&second_outputs), data[12..]);
        let second_status = status_payload(&second_outputs);
        assert_eq!(second_status["extra"]["complete"], true);
        assert_eq!(
            second_status["extra"]["file_sha256_hex"],
            payload_hash(data)
        );

        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn execute_file_push_rejects_hash_mismatch_without_writing() {
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-file-push-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("pushed.txt");
        let error = execute_job_command(
            job_id,
            &JobCommand::FilePush {
                path: path.to_string_lossy().to_string(),
                mode: 0o600,
                size_bytes: 4,
                sha256_hex: "00".repeat(32),
                data_base64: vpsman_common::encode_inline_file_payload(b"data").unwrap(),
                existing_policy: FileExistingPolicy::Replace,
                owner: None,
                group: None,
                uid: None,
                gid: None,
                ownership_policy: FileOwnershipPolicy::Fail,
            },
            5,
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("hash mismatch"));
        assert!(!path.exists());

        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    fn transfer_chunk(offset: u64, data: &[u8]) -> FilePushChunk {
        FilePushChunk {
            offset,
            size_bytes: data.len() as u32,
            sha256_hex: payload_hash(data),
            data_base64: vpsman_common::encode_inline_file_payload(data).unwrap(),
        }
    }

    fn assert_transfer_next_offset(
        outputs: &[vpsman_common::CommandOutput],
        kind: &str,
        offset: u64,
    ) {
        let status = outputs.iter().find(|output| output.done).unwrap();
        let status: serde_json::Value = serde_json::from_slice(&status.data).unwrap();
        assert_eq!(status["type"], kind);
        assert_eq!(status["next_offset"], offset);
    }

    fn status_payload(outputs: &[vpsman_common::CommandOutput]) -> serde_json::Value {
        let status = outputs
            .iter()
            .find(|output| output.done && output.stream == OutputStream::Status)
            .expect("status output");
        serde_json::from_slice(&status.data).unwrap()
    }

    fn stdout_bytes(outputs: &[vpsman_common::CommandOutput]) -> Vec<u8> {
        outputs
            .iter()
            .filter(|output| output.stream == OutputStream::Stdout)
            .flat_map(|output| output.data.clone())
            .collect()
    }

    fn pty_text(outputs: &[vpsman_common::CommandOutput]) -> String {
        outputs
            .iter()
            .filter(|output| output.stream == OutputStream::Pty)
            .map(|output| String::from_utf8_lossy(&output.data))
            .collect::<String>()
    }

    #[tokio::test]
    async fn execute_user_sessions_returns_status_metadata() {
        let job_id = uuid::Uuid::new_v4();
        let outputs = execute_job_command(job_id, &JobCommand::UserSessions, 5)
            .await
            .unwrap();
        let status = outputs.iter().find(|output| output.done).unwrap();
        let status: serde_json::Value = serde_json::from_slice(&status.data).unwrap();

        assert_eq!(status["type"], "user_sessions");
        assert!(status["source"]
            .as_str()
            .is_some_and(|source| { source == "/usr/bin/w" || source == "/usr/bin/who" }));
    }

    #[tokio::test]
    async fn execute_user_sessions_uses_custom_command_source() {
        let job_id = uuid::Uuid::new_v4();
        let root = std::env::temp_dir().join(format!("vpsman-user-source-{job_id}"));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("users.sh");
        std::fs::write(&source, "#!/bin/sh\nprintf 'custom-user tty1\\n'\n").unwrap();
        make_executable(&source);
        let config = AgentConfig {
            execution: AgentExecutionConfig {
                user_sessions_source: AgentUserSessionsSource::CustomCommand,
                user_sessions_command: Some(RuntimeTunnelCommand {
                    argv: vec![source.to_string_lossy().to_string()],
                    timeout_secs: 2,
                    max_output_bytes: 1024,
                }),
                ..AgentExecutionConfig::default()
            },
            ..AgentConfig::default()
        };

        let outputs = execute_job_command_with_config_and_output_sink(
            &config,
            job_id,
            &JobCommand::UserSessions,
            5,
            None,
        )
        .await
        .unwrap();

        assert!(String::from_utf8_lossy(&stdout_bytes(&outputs)).contains("custom-user"));
        let status = status_payload(&outputs);
        assert_eq!(status["type"], "user_sessions");
        assert_eq!(status["command_source"], "custom_command");
        assert!(status["command_sha256_hex"].as_str().is_some());
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn execute_process_list_returns_bounded_snapshot() {
        let job_id = uuid::Uuid::new_v4();
        let outputs = execute_job_command(job_id, &JobCommand::ProcessList { limit: 8 }, 5)
            .await
            .unwrap();
        let stdout = outputs
            .iter()
            .find(|output| output.stream == OutputStream::Stdout)
            .unwrap();
        let snapshot: serde_json::Value = serde_json::from_slice(&stdout.data).unwrap();
        let status = outputs.iter().find(|output| output.done).unwrap();
        let status: serde_json::Value = serde_json::from_slice(&status.data).unwrap();

        assert_eq!(snapshot["type"], "process_list");
        assert!(snapshot["processes"].as_array().unwrap().len() <= 8);
        assert_eq!(status["type"], "process_list");
        assert!(status["count"].as_u64().unwrap() <= 8);
    }

    #[tokio::test]
    async fn execute_process_list_uses_custom_json_source() {
        let job_id = uuid::Uuid::new_v4();
        let root = std::env::temp_dir().join(format!("vpsman-process-source-{job_id}"));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("processes.sh");
        std::fs::write(
            &source,
            "#!/bin/sh\nprintf '%s\\n' '{\"processes\":[{\"pid\":2,\"ppid\":1,\"uid\":0,\"state\":\"S\",\"name\":\"small\",\"command\":\"small\",\"rss_kib\":10},{\"pid\":1,\"ppid\":0,\"uid\":0,\"state\":\"R\",\"name\":\"large\",\"command\":\"large\",\"rss_kib\":99}],\"truncated\":false}'\n",
        )
        .unwrap();
        make_executable(&source);
        let config = AgentConfig {
            execution: AgentExecutionConfig {
                process_inventory_source: AgentProcessInventorySource::CustomCommand,
                process_inventory_command: Some(RuntimeTunnelCommand {
                    argv: vec![source.to_string_lossy().to_string(), "{limit}".to_string()],
                    timeout_secs: 2,
                    max_output_bytes: 4096,
                }),
                ..AgentExecutionConfig::default()
            },
            ..AgentConfig::default()
        };

        let outputs = execute_job_command_with_config_and_output_sink(
            &config,
            job_id,
            &JobCommand::ProcessList { limit: 1 },
            5,
            None,
        )
        .await
        .unwrap();
        let snapshot: serde_json::Value = serde_json::from_slice(&stdout_bytes(&outputs)).unwrap();
        let status = status_payload(&outputs);

        assert_eq!(snapshot["type"], "process_list");
        assert_eq!(snapshot["source"], "custom_command");
        assert_eq!(snapshot["processes"].as_array().unwrap().len(), 1);
        assert_eq!(snapshot["processes"][0]["name"], "large");
        assert_eq!(snapshot["truncated"], true);
        assert_eq!(status["source"], "custom_command");
        std::fs::remove_dir_all(root).ok();
    }

    fn make_executable(path: &std::path::Path) {
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }
}
