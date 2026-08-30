mod tests {
    use crate::command_worker::{CommandCancelToken, CommandCanceled};
    use crate::executor::{
        acquire_file_transfer_session_owner, execute_job_command,
        execute_job_command_with_config_and_output_sink, execute_job_command_with_output_sink,
    };
    use crate::file_download::{
        execute_file_transfer_download_chunk, execute_file_transfer_download_start,
    };
    use crate::file_push::{
        execute_file_transfer_abort, execute_file_transfer_chunk, execute_file_transfer_start,
    };
    use crate::terminal::{
        control_terminal_session, execute_terminal_command_with_stream_sink,
        terminal_session_is_registered,
    };
    use std::{io::Cursor, os::unix::fs::PermissionsExt, time::Duration};
    use tokio::sync::mpsc;
    use vpsman_common::{
        payload_hash, AgentConfig, AgentExecutionConfig, AgentExecutionEnvironmentPolicy,
        AgentExecutionProcessCleanupPolicy, AgentExecutionPtyPolicy, AgentProcessInventorySource,
        AgentUserSessionsSource, FileActionPolicy, FileExistingPolicy, FileOwnershipPolicy,
        FilePushChunk, JobCommand, OutputStream, RuntimeTunnelCommand, TerminalControlAck,
        TerminalControlAction, TerminalControlRequest, TerminalStreamOutput,
    };

    #[tokio::test]
    async fn file_transfer_session_owner_serializes_only_the_exact_session_and_cleans_up() {
        let session_id = uuid::Uuid::new_v4();
        let other_session_id = uuid::Uuid::new_v4();
        let first = acquire_file_transfer_session_owner(session_id).await;

        let same_session = tokio::spawn(acquire_file_transfer_session_owner(session_id));
        tokio::task::yield_now().await;
        assert!(!same_session.is_finished());

        let other_session = tokio::time::timeout(
            Duration::from_secs(1),
            acquire_file_transfer_session_owner(other_session_id),
        )
        .await
        .expect("an unrelated transfer session must not wait");
        drop(other_session);

        drop(first);
        let same_session = tokio::time::timeout(Duration::from_secs(1), same_session)
            .await
            .expect("the next exact-session command must run after its owner releases")
            .expect("exact-session owner task must complete");
        drop(same_session);

        // A canceled waiter must not retain a registry entry or prevent a
        // later exact-session command from acquiring normally.
        let first = acquire_file_transfer_session_owner(session_id).await;
        let waiting = tokio::spawn(acquire_file_transfer_session_owner(session_id));
        tokio::task::yield_now().await;
        waiting.abort();
        let _ = waiting.await;
        drop(first);
        let final_owner = tokio::time::timeout(
            Duration::from_secs(1),
            acquire_file_transfer_session_owner(session_id),
        )
        .await
        .expect("a canceled waiter must not leak exact-session ownership");
        drop(final_owner);
    }

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
    async fn execute_argv_command_reports_truncated_stdout_in_status() {
        let outputs = execute_job_command(
            uuid::Uuid::new_v4(),
            &JobCommand::Shell {
                argv: vec![
                    "/bin/sh".to_string(),
                    "-lc".to_string(),
                    "head -c 70000 /dev/zero | tr '\\0' x".to_string(),
                ],
                pty: false,
            },
            5,
        )
        .await
        .unwrap();

        assert_eq!(stdout_bytes(&outputs).len(), 64 * 1024);
        let status = outputs
            .iter()
            .find(|output| output.done && output.stream == OutputStream::Status)
            .expect("status output");
        assert_eq!(status.exit_code, Some(0));
        let payload: serde_json::Value = serde_json::from_slice(&status.data).unwrap();
        assert_eq!(payload["type"], "shell_argv");
        assert_eq!(payload["output_truncated"], true);
        assert_eq!(payload["stdout_truncated"], true);
        assert_eq!(payload["stderr_truncated"], false);
        assert_eq!(payload["output_limit_bytes"], 64 * 1024);
        assert!(payload["message"]
            .as_str()
            .unwrap()
            .contains("command output truncated"));
    }

    #[tokio::test]
    async fn execute_argv_command_reports_truncated_stderr_in_status() {
        let outputs = execute_job_command(
            uuid::Uuid::new_v4(),
            &JobCommand::Shell {
                argv: vec![
                    "/bin/sh".to_string(),
                    "-lc".to_string(),
                    "head -c 70000 /dev/zero | tr '\\0' x >&2".to_string(),
                ],
                pty: false,
            },
            5,
        )
        .await
        .unwrap();

        assert_eq!(stderr_bytes(&outputs).len(), 64 * 1024);
        let status = outputs
            .iter()
            .find(|output| output.done && output.stream == OutputStream::Status)
            .expect("status output");
        assert_eq!(status.exit_code, Some(0));
        let payload: serde_json::Value = serde_json::from_slice(&status.data).unwrap();
        assert_eq!(payload["type"], "shell_argv");
        assert_eq!(payload["output_truncated"], true);
        assert_eq!(payload["stdout_truncated"], false);
        assert_eq!(payload["stderr_truncated"], true);
        assert_eq!(payload["output_limit_bytes"], 64 * 1024);
    }

    #[tokio::test]
    async fn execute_pty_command_reports_truncation_in_status() {
        let outputs = execute_job_command(
            uuid::Uuid::new_v4(),
            &JobCommand::Shell {
                argv: vec![
                    "/bin/sh".to_string(),
                    "-lc".to_string(),
                    "head -c 70000 /dev/zero | tr '\\0' x".to_string(),
                ],
                pty: true,
            },
            5,
        )
        .await
        .unwrap();

        assert_eq!(pty_bytes(&outputs).len(), 64 * 1024);
        let status = outputs
            .iter()
            .find(|output| output.done && output.stream == OutputStream::Status)
            .expect("status output");
        assert_eq!(status.exit_code, Some(0));
        let payload: serde_json::Value = serde_json::from_slice(&status.data).unwrap();
        assert_eq!(payload["type"], "shell_pty");
        assert_eq!(payload["output_truncated"], true);
        assert_eq!(payload["pty_truncated"], true);
        assert_eq!(payload["output_limit_bytes"], 64 * 1024);
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
    async fn terminal_session_accepts_exact_input_resize_and_close() {
        let session_id = uuid::Uuid::new_v4();
        let open_job_id = uuid::Uuid::new_v4();
        let (stream_tx, mut stream_rx) = mpsc::channel(16);
        let outputs = execute_terminal_command_with_stream_sink(
            &AgentConfig::default(),
            open_job_id,
            &JobCommand::TerminalOpen {
                session_id,
                argv: vec![
                    "/bin/sh".to_string(),
                    "-lc".to_string(),
                    "printf 'ready\\n'; stty raw -echo; dd bs=1 count=4 2>/dev/null | od -An -t x1; sleep 30".to_string(),
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
            Some(stream_tx),
        )
        .await
        .unwrap();

        let status = outputs
            .iter()
            .find(|output| output.stream == OutputStream::Status)
            .expect("terminal open status");
        assert_eq!(status.job_id, open_job_id);
        assert_eq!(status.exit_code, Some(0));
        assert!(!status.done);
        let payload: serde_json::Value = serde_json::from_slice(&status.data).unwrap();
        assert_eq!(payload["type"], "terminal_open");
        assert_eq!(payload["status"], "opened");

        let resize = issue_terminal_control(
            session_id,
            TerminalControlAction::Resize {
                cols: 100,
                rows: 30,
            },
        )
        .await;
        assert!(resize.accepted);
        assert_eq!(resize.action, "resize");
        assert_eq!(resize.status, "resized");
        assert_eq!(resize.cols, Some(100));
        assert_eq!(resize.rows, Some(30));

        let exact_input = [b'A', 0x03, 0x7f, b'\r'];
        let input = issue_terminal_control(
            session_id,
            TerminalControlAction::Input {
                data_base64: vpsman_common::encode_inline_file_payload(&exact_input).unwrap(),
            },
        )
        .await;
        assert!(input.accepted);
        assert_eq!(input.action, "input");
        assert_eq!(input.status, "accepted");
        assert_eq!(input.input_seq, Some(1));
        assert_eq!(input.written_bytes, Some(exact_input.len() as u64));
        let streamed =
            terminal_stream_text_until(&mut stream_rx, open_job_id, session_id, "41 03 7f 0d")
                .await;
        assert!(streamed.contains("41 03 7f 0d"), "{streamed:?}");

        let close = issue_terminal_control(
            session_id,
            TerminalControlAction::Close {
                reason: Some("test complete".to_string()),
            },
        )
        .await;
        assert!(close.accepted);
        assert_eq!(close.action, "close");
        assert_eq!(close.status, "closed");

        let after_close = issue_terminal_control(
            session_id,
            TerminalControlAction::Input {
                data_base64: vpsman_common::encode_inline_file_payload(b"ignored").unwrap(),
            },
        )
        .await;
        assert!(!after_close.accepted);
        assert_eq!(after_close.status, "missing");
        assert_eq!(after_close.message, "terminal_session_not_open");
    }

    #[tokio::test]
    async fn terminal_input_sequence_is_assigned_by_registry() {
        let session_id = uuid::Uuid::new_v4();
        let outputs = execute_job_command(
            uuid::Uuid::new_v4(),
            &JobCommand::TerminalOpen {
                session_id,
                argv: vec![
                    "/bin/sh".to_string(),
                    "-lc".to_string(),
                    "cat >/dev/null".to_string(),
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
        assert_eq!(terminal_open_status_payload(&outputs)["status"], "opened");

        let first = issue_terminal_control(
            session_id,
            TerminalControlAction::Input {
                data_base64: vpsman_common::encode_inline_file_payload(b"first\n").unwrap(),
            },
        )
        .await;
        let second = issue_terminal_control(
            session_id,
            TerminalControlAction::Input {
                data_base64: vpsman_common::encode_inline_file_payload(b"second\n").unwrap(),
            },
        )
        .await;

        assert!(first.accepted);
        assert!(second.accepted);
        assert_eq!(first.input_seq, Some(1));
        assert_eq!(second.input_seq, Some(2));
        assert_eq!(first.written_bytes, Some(6));
        assert_eq!(second.written_bytes, Some(7));

        let close = close_test_terminal(session_id).await;
        assert!(close.accepted);
    }

    #[tokio::test]
    async fn terminal_session_id_cannot_be_rebound_to_another_job() {
        let session_id = uuid::Uuid::new_v4();
        let command = JobCommand::TerminalOpen {
            session_id,
            argv: vec![
                "/bin/sh".to_string(),
                "-lc".to_string(),
                "cat >/dev/null".to_string(),
            ],
            cwd: None,
            user: None,
            user_policy: vpsman_common::TerminalUserPolicy::Fail,
            cols: 120,
            rows: 40,
            replay_from_seq: None,
            idle_timeout_secs: 30,
            flow_window_bytes: 65_536,
        };
        let first_job_id = uuid::Uuid::new_v4();
        let first = execute_job_command(first_job_id, &command, 5)
            .await
            .unwrap();
        assert_eq!(terminal_open_status_payload(&first)["status"], "opened");

        let second_job_id = uuid::Uuid::new_v4();
        let second = execute_job_command(second_job_id, &command, 5)
            .await
            .unwrap();
        let status = second
            .iter()
            .find(|output| output.stream == OutputStream::Status)
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&status.data).unwrap();
        assert_eq!(payload["status"], "rejected");
        assert_eq!(payload["reason"], "terminal_session_id_in_use");
        assert_eq!(status.job_id, second_job_id);
        assert!(status.done);

        let close = close_test_terminal(session_id).await;
        assert!(close.accepted);
    }

    #[tokio::test]
    async fn terminal_stream_reports_idle_output_without_input() {
        let session_id = uuid::Uuid::new_v4();
        let open_job_id = uuid::Uuid::new_v4();
        let (stream_tx, mut stream_rx) = mpsc::channel(16);
        let open_outputs = execute_terminal_command_with_stream_sink(
            &AgentConfig::default(),
            open_job_id,
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
            Some(stream_tx),
        )
        .await
        .unwrap();
        assert_eq!(
            terminal_open_status_payload(&open_outputs)["status"],
            "opened"
        );

        let streamed = terminal_stream_text_until(
            &mut stream_rx,
            open_job_id,
            session_id,
            "idle-terminal-output",
        )
        .await;
        assert!(streamed.contains("idle-terminal-output"), "{streamed:?}");

        let close = close_test_terminal(session_id).await;
        assert!(close.accepted);
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
        let status = terminal_open_status_payload(&outputs);
        assert_eq!(status["type"], "terminal_open");
        assert_eq!(status["status"], "opened");
        assert!(status["output_retained_bytes"].as_u64().unwrap() <= 4096);
        assert!(status["output_dropped_bytes"].as_u64().unwrap() > 0);
        assert_eq!(status["output_replay_truncated"], true);

        let close = close_test_terminal(session_id).await;
        assert!(close.accepted);
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
        assert_eq!(terminal_open_status_payload(&outputs)["status"], "opened");

        tokio::time::timeout(Duration::from_secs(5), async {
            while terminal_session_is_registered(session_id).await {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("idle terminal session was not removed");
        let input = issue_terminal_control(
            session_id,
            TerminalControlAction::Input {
                data_base64: vpsman_common::encode_inline_file_payload(b"hello\n").unwrap(),
            },
        )
        .await;
        assert!(!input.accepted);
        assert_eq!(input.action, "input");
        assert_eq!(input.status, "missing");
        assert_eq!(input.message, "terminal_session_not_open");
    }

    #[tokio::test]
    async fn terminal_controls_report_missing_session() {
        let session_id = uuid::Uuid::new_v4();
        let input = issue_terminal_control(
            session_id,
            TerminalControlAction::Input {
                data_base64: vpsman_common::encode_inline_file_payload(b"hello\n").unwrap(),
            },
        )
        .await;
        let resize = issue_terminal_control(
            session_id,
            TerminalControlAction::Resize {
                cols: 100,
                rows: 30,
            },
        )
        .await;
        let close = close_test_terminal(session_id).await;

        for ack in [input, resize, close] {
            assert!(!ack.accepted);
            assert_eq!(ack.status, "missing");
            assert_eq!(ack.message, "terminal_session_not_open");
        }
    }

    #[tokio::test]
    async fn terminal_controls_report_actual_exited_session_state() {
        let session_id = uuid::Uuid::new_v4();
        let outputs = execute_job_command(
            uuid::Uuid::new_v4(),
            &JobCommand::TerminalOpen {
                session_id,
                argv: vec!["/bin/true".to_string()],
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

        assert_eq!(terminal_open_status_payload(&outputs)["status"], "opened");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let input = issue_terminal_control(
            session_id,
            TerminalControlAction::Input {
                data_base64: vpsman_common::encode_inline_file_payload(b"ignored").unwrap(),
            },
        )
        .await;
        assert!(!input.accepted);
        assert_eq!(input.action, "input");
        assert_eq!(input.status, "exited");
        assert_eq!(input.message, "terminal_session_exited");

        let resize = issue_terminal_control(
            session_id,
            TerminalControlAction::Resize {
                cols: 100,
                rows: 30,
            },
        )
        .await;
        assert!(!resize.accepted);
        assert_eq!(resize.action, "resize");
        assert_eq!(resize.status, "exited");
        assert_eq!(resize.message, "terminal_session_exited");

        let close = close_test_terminal(session_id).await;
        assert!(!close.accepted);
        assert_eq!(close.action, "close");
        assert_eq!(close.status, "exited");
        assert_eq!(close.message, "terminal_session_exited");
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
        assert_eq!(payload["max_timeout_secs"], 1);
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
        assert_eq!(payload["max_timeout_secs"], 1);
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
                follow_symlinks: false,
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
    async fn execute_file_pull_rejects_symlink_by_default() {
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-pull-link-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let target = dir.join("target.txt");
        let link = dir.join("source-link.txt");
        tokio::fs::write(&target, b"target contents").await.unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let error = execute_job_command(
            job_id,
            &JobCommand::FilePull {
                path: link.to_string_lossy().to_string(),
                follow_symlinks: false,
            },
            5,
        )
        .await
        .unwrap_err();

        assert!(error_chain_contains(&error, "file pull path is a symlink"));
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn execute_file_pull_allows_explicit_symlink_follow() {
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-pull-follow-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let target = dir.join("target.txt");
        let link = dir.join("source-link.txt");
        tokio::fs::write(&target, b"target contents").await.unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let outputs = execute_job_command(
            job_id,
            &JobCommand::FilePull {
                path: link.to_string_lossy().to_string(),
                follow_symlinks: true,
            },
            5,
        )
        .await
        .unwrap();

        assert_eq!(stdout_bytes(&outputs), b"target contents");
        let status = status_payload(&outputs);
        assert_eq!(status["sha256_hex"], payload_hash(b"target contents"));
        let _ = tokio::fs::remove_dir_all(dir).await;
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
                follow_symlinks: false,
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
                follow_symlinks: false,
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
                follow_symlinks: false,
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
                follow_symlinks: false,
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
    async fn execute_file_copy_overwrites_regular_file_without_temp_leak() {
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-file-copy-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let source = dir.join("source.txt");
        let destination = dir.join("destination.txt");
        tokio::fs::write(&source, b"new copied contents")
            .await
            .unwrap();
        tokio::fs::write(&destination, b"old contents")
            .await
            .unwrap();

        let outputs = execute_job_command(
            job_id,
            &JobCommand::FileCopy {
                path: source.to_string_lossy().to_string(),
                new_path: destination.to_string_lossy().to_string(),
                overwrite: true,
                recursive: false,
                follow_symlinks: false,
                policy: FileActionPolicy::Fail,
            },
            5,
        )
        .await
        .unwrap();

        assert_eq!(
            tokio::fs::read(&destination).await.unwrap(),
            b"new copied contents"
        );
        let status = status_payload(&outputs);
        assert_eq!(status["type"], "file_copy");
        assert_eq!(status["status"], "copied");
        assert_eq!(status["overwrite"], true);
        let mut entries = tokio::fs::read_dir(&dir).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            assert!(
                !entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".vpsman-copy-"),
                "temporary copy file leaked: {:?}",
                entry.path()
            );
        }

        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn execute_file_copy_rejects_destination_symlink_without_following_target() {
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-file-copy-symlink-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let source = dir.join("source.txt");
        let link_target = dir.join("target.txt");
        let destination_link = dir.join("destination-link.txt");
        tokio::fs::write(&source, b"new copied contents")
            .await
            .unwrap();
        tokio::fs::write(&link_target, b"protected target")
            .await
            .unwrap();
        std::os::unix::fs::symlink(&link_target, &destination_link).unwrap();

        let error = execute_job_command(
            job_id,
            &JobCommand::FileCopy {
                path: source.to_string_lossy().to_string(),
                new_path: destination_link.to_string_lossy().to_string(),
                overwrite: true,
                recursive: false,
                follow_symlinks: false,
                policy: FileActionPolicy::Fail,
            },
            5,
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("copy destination is a symlink"));
        assert_eq!(
            tokio::fs::read(&link_target).await.unwrap(),
            b"protected target"
        );
        assert!(tokio::fs::symlink_metadata(&destination_link)
            .await
            .unwrap()
            .file_type()
            .is_symlink());

        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn execute_file_copy_can_explicitly_follow_destination_symlink_target() {
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-file-copy-follow-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let source = dir.join("source.txt");
        let link_target = dir.join("target.txt");
        let destination_link = dir.join("destination-link.txt");
        tokio::fs::write(&source, b"new copied contents")
            .await
            .unwrap();
        tokio::fs::write(&link_target, b"old target").await.unwrap();
        std::os::unix::fs::symlink(&link_target, &destination_link).unwrap();

        let outputs = execute_job_command(
            job_id,
            &JobCommand::FileCopy {
                path: source.to_string_lossy().to_string(),
                new_path: destination_link.to_string_lossy().to_string(),
                overwrite: true,
                recursive: false,
                follow_symlinks: true,
                policy: FileActionPolicy::Fail,
            },
            5,
        )
        .await
        .unwrap();

        assert_eq!(
            tokio::fs::read(&link_target).await.unwrap(),
            b"new copied contents"
        );
        assert!(tokio::fs::symlink_metadata(&destination_link)
            .await
            .unwrap()
            .file_type()
            .is_symlink());
        let status = status_payload(&outputs);
        assert_eq!(status["type"], "file_copy");
        assert_eq!(status["follow_symlinks"], true);

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
    async fn execute_resumable_file_transfer_chunk_completion_wins_after_write() {
        let session_id = uuid::Uuid::new_v4();
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-resume-cancel-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("cancel.bin");
        let temp_path = dir.join(format!(".vpsman-transfer-cancel.bin-{session_id}.part"));
        let data = vec![3_u8; 1024];
        let token_hash = payload_hash(b"resume-token");

        execute_file_transfer_start(
            job_id,
            session_id,
            path.to_str().unwrap(),
            0o640,
            data.len() as u64,
            &payload_hash(&data),
            data.len() as u32,
            1,
            FileExistingPolicy::Replace,
            &token_hash,
            CommandCancelToken::default(),
        )
        .await
        .unwrap();

        let cancel_token = CommandCancelToken::default();
        let task_token = cancel_token.clone();
        let task_token_hash = token_hash.clone();
        let task_chunk = transfer_chunk(0, &data);
        let handle = tokio::spawn(async move {
            execute_file_transfer_chunk(
                job_id,
                session_id,
                0,
                &task_chunk,
                &task_token_hash,
                task_token,
            )
            .await
        });

        for _ in 0..200 {
            if tokio::fs::metadata(&temp_path)
                .await
                .map(|metadata| metadata.len() == data.len() as u64)
                .unwrap_or(false)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            tokio::fs::metadata(&temp_path).await.unwrap().len(),
            data.len() as u64
        );

        cancel_token.cancel("operator canceled".to_string());
        let outputs = tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_transfer_next_offset(&outputs, "file_transfer_chunk_ack", data.len() as u64);

        let _ = execute_file_transfer_abort(
            job_id,
            session_id,
            &token_hash,
            CommandCancelToken::default(),
        )
        .await;
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
                follow_symlinks: false,
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
    async fn execute_resumable_file_download_rejects_symlink_by_default() {
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-download-link-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let target = dir.join("source.txt");
        let link = dir.join("source-link.txt");
        tokio::fs::write(&target, b"download target").await.unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let session_id = uuid::Uuid::new_v4();
        let token_hash = payload_hash(b"download-token");

        let error = execute_job_command(
            job_id,
            &JobCommand::FileTransferDownloadStart {
                session_id,
                path: link.to_string_lossy().to_string(),
                chunk_size_bytes: 64,
                rate_limit_kbps: 0,
                follow_symlinks: false,
                resume_token_hash: token_hash,
            },
            5,
        )
        .await
        .unwrap_err();

        assert!(error_chain_contains(
            &error,
            "file transfer download source is a symlink"
        ));
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn execute_resumable_file_download_allows_explicit_symlink_follow() {
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-download-follow-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let target = dir.join("source.txt");
        let link = dir.join("source-link.txt");
        let data = b"download target";
        tokio::fs::write(&target, data).await.unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let session_id = uuid::Uuid::new_v4();
        let token_hash = payload_hash(b"download-token");

        let start_outputs = execute_job_command(
            job_id,
            &JobCommand::FileTransferDownloadStart {
                session_id,
                path: link.to_string_lossy().to_string(),
                chunk_size_bytes: 64,
                rate_limit_kbps: 0,
                follow_symlinks: true,
                resume_token_hash: token_hash.clone(),
            },
            5,
        )
        .await
        .unwrap();
        let start_status = status_payload(&start_outputs);
        assert_eq!(start_status["extra"]["follow_symlinks"], true);
        assert_eq!(start_status["extra"]["sha256_hex"], payload_hash(data));

        let chunk_outputs = execute_job_command(
            job_id,
            &JobCommand::FileTransferDownloadChunk {
                session_id,
                offset: 0,
                max_bytes: 64,
                resume_token_hash: token_hash,
            },
            5,
        )
        .await
        .unwrap();

        assert_eq!(stdout_bytes(&chunk_outputs), data);
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn execute_resumable_file_download_chunk_rejects_changed_source_identity() {
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-download-change-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("source.txt");
        let old_path = dir.join("source-old.txt");
        tokio::fs::write(&path, b"original download contents")
            .await
            .unwrap();
        let session_id = uuid::Uuid::new_v4();
        let token_hash = payload_hash(b"download-token");

        execute_file_transfer_download_start(
            job_id,
            session_id,
            path.to_str().unwrap(),
            64,
            0,
            false,
            &token_hash,
            CommandCancelToken::default(),
        )
        .await
        .unwrap();
        tokio::fs::rename(&path, &old_path).await.unwrap();
        tokio::fs::write(&path, b"replacement").await.unwrap();

        let error = execute_file_transfer_download_chunk(
            job_id,
            session_id,
            0,
            64,
            &token_hash,
            CommandCancelToken::default(),
        )
        .await
        .unwrap_err();

        assert!(error_chain_contains(
            &error,
            "download source changed since session start"
        ));
        let _ = tokio::fs::remove_file(
            std::env::temp_dir().join(format!("vpsman-download-{session_id}.json")),
        )
        .await;
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn execute_resumable_file_download_chunk_observes_cancel_before_output() {
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-file-download-cancel-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("source.txt");
        let data = vec![5_u8; 1024];
        tokio::fs::write(&path, &data).await.unwrap();
        let session_id = uuid::Uuid::new_v4();
        let token_hash = payload_hash(b"download-token");

        execute_file_transfer_download_start(
            job_id,
            session_id,
            path.to_str().unwrap(),
            data.len() as u32,
            1,
            false,
            &token_hash,
            CommandCancelToken::default(),
        )
        .await
        .unwrap();

        let cancel_token = CommandCancelToken::default();
        let task_token = cancel_token.clone();
        let task_token_hash = token_hash.clone();
        let handle = tokio::spawn(async move {
            execute_file_transfer_download_chunk(
                job_id,
                session_id,
                0,
                data.len() as u32,
                &task_token_hash,
                task_token,
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel_token.cancel("operator canceled".to_string());

        let error = tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        let canceled = error
            .downcast_ref::<CommandCanceled>()
            .expect("command canceled error");
        assert_eq!(canceled.operation_type(), "file_transfer_download_chunk");

        let _ = tokio::fs::remove_file(
            std::env::temp_dir().join(format!("vpsman-download-{session_id}.json")),
        )
        .await;
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

    async fn issue_terminal_control(
        session_id: uuid::Uuid,
        action: TerminalControlAction,
    ) -> TerminalControlAck {
        let request_id = uuid::Uuid::new_v4();
        let action_kind = action.kind();
        let ack = control_terminal_session(TerminalControlRequest {
            request_id,
            session_id,
            action,
        })
        .await;
        assert_eq!(ack.request_id, request_id);
        assert_eq!(ack.session_id, session_id);
        assert_eq!(ack.action, action_kind);
        ack
    }

    async fn close_test_terminal(session_id: uuid::Uuid) -> TerminalControlAck {
        issue_terminal_control(
            session_id,
            TerminalControlAction::Close {
                reason: Some("test cleanup".to_string()),
            },
        )
        .await
    }

    async fn terminal_stream_text_until(
        receiver: &mut mpsc::Receiver<TerminalStreamOutput>,
        open_job_id: uuid::Uuid,
        session_id: uuid::Uuid,
        expected: &str,
    ) -> String {
        let bytes = tokio::time::timeout(Duration::from_secs(3), async {
            let mut bytes = Vec::new();
            loop {
                let event = receiver.recv().await.expect("terminal stream closed");
                assert_eq!(event.job_id, open_job_id);
                assert_eq!(event.output.job_id, open_job_id);
                assert_eq!(event.session_id, session_id);
                if event.output.stream != OutputStream::Pty {
                    continue;
                }
                bytes.extend_from_slice(&event.output.data);
                if String::from_utf8_lossy(&bytes).contains(expected) {
                    break bytes;
                }
            }
        })
        .await
        .expect("timed out waiting for terminal stream output");
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn terminal_open_status_payload(outputs: &[vpsman_common::CommandOutput]) -> serde_json::Value {
        let status = outputs
            .iter()
            .find(|output| output.stream == OutputStream::Status)
            .expect("terminal open status output");
        assert!(!status.done);
        serde_json::from_slice(&status.data).unwrap()
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

    fn error_chain_contains(error: &anyhow::Error, needle: &str) -> bool {
        error
            .chain()
            .any(|cause| cause.to_string().contains(needle))
    }

    fn stderr_bytes(outputs: &[vpsman_common::CommandOutput]) -> Vec<u8> {
        outputs
            .iter()
            .filter(|output| output.stream == OutputStream::Stderr)
            .flat_map(|output| output.data.clone())
            .collect()
    }

    fn pty_bytes(outputs: &[vpsman_common::CommandOutput]) -> Vec<u8> {
        outputs
            .iter()
            .filter(|output| output.stream == OutputStream::Pty)
            .flat_map(|output| output.data.clone())
            .collect()
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
        let config = AgentConfig {
            execution: AgentExecutionConfig {
                user_sessions_source: AgentUserSessionsSource::CustomCommand,
                user_sessions_command: Some(RuntimeTunnelCommand {
                    argv: vec!["/bin/sh".to_string(), source.to_string_lossy().to_string()],
                    max_timeout_secs: 2,
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
    async fn execute_user_sessions_preserves_command_timeout_status() {
        let job_id = uuid::Uuid::new_v4();
        let root = std::env::temp_dir().join(format!("vpsman-user-timeout-{job_id}"));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("users-timeout.sh");
        std::fs::write(
            &source,
            "#!/bin/sh\nprintf 'custom-user tty1\\n'\nexec 1>&-\nsleep 10\n",
        )
        .unwrap();
        let config = AgentConfig {
            execution: AgentExecutionConfig {
                user_sessions_source: AgentUserSessionsSource::CustomCommand,
                user_sessions_command: Some(RuntimeTunnelCommand {
                    argv: vec!["/bin/sh".to_string(), source.to_string_lossy().to_string()],
                    max_timeout_secs: 1,
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

        let status = status_payload(&outputs);
        assert_eq!(status["type"], "command_timeout");
        assert_eq!(status["mode"], "shell_argv");
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
        let config = AgentConfig {
            execution: AgentExecutionConfig {
                process_inventory_source: AgentProcessInventorySource::CustomCommand,
                process_inventory_command: Some(RuntimeTunnelCommand {
                    argv: vec![
                        "/bin/sh".to_string(),
                        source.to_string_lossy().to_string(),
                        "{limit}".to_string(),
                    ],
                    max_timeout_secs: 2,
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

    #[tokio::test]
    async fn custom_process_inventory_timeout_covers_wait_after_stdout_closes() {
        let job_id = uuid::Uuid::new_v4();
        let root = std::env::temp_dir().join(format!("vpsman-process-timeout-{job_id}"));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("processes-timeout.sh");
        std::fs::write(
            &source,
            "#!/bin/sh\nprintf '%s\\n' '{\"processes\":[]}'\nexec 1>&-\nsleep 10\n",
        )
        .unwrap();
        let config = AgentConfig {
            execution: AgentExecutionConfig {
                process_inventory_source: AgentProcessInventorySource::CustomCommand,
                process_inventory_command: Some(RuntimeTunnelCommand {
                    argv: vec!["/bin/sh".to_string(), source.to_string_lossy().to_string()],
                    max_timeout_secs: 1,
                    max_output_bytes: 4096,
                }),
                ..AgentExecutionConfig::default()
            },
            ..AgentConfig::default()
        };
        let started = std::time::Instant::now();

        let error = execute_job_command_with_config_and_output_sink(
            &config,
            job_id,
            &JobCommand::ProcessList { limit: 1 },
            5,
            None,
        )
        .await
        .unwrap_err();

        assert!(started.elapsed() < std::time::Duration::from_secs(4));
        assert!(error_chain_contains(
            &error,
            "process inventory source timed out"
        ));
        std::fs::remove_dir_all(root).ok();
    }
}
