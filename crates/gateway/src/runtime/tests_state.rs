use super::*;
use vpsman_common::OutputStream;

#[test]
fn pending_command_does_not_retain_output_after_ack_response_is_consumed() {
    let job_id = uuid::Uuid::new_v4();
    let (response, _receiver) = oneshot::channel();
    let mut pending = PendingCommand {
        client_id: "client-a".to_string(),
        job_id,
        command_version: 1,
        payload_hash: "payload-a".to_string(),
        ack: Some(JobAck {
            job_id,
            accepted: true,
            message: "accepted".to_string(),
        }),
        outputs: Vec::new(),
        response: Some(response),
    };

    finish_pending_command_response(&mut pending, None, Vec::new());
    let dropped = pending.retain_output_if_response_waiting(CommandOutput {
        job_id,
        stream: OutputStream::Stdout,
        data: b"noisy output after ack".to_vec(),
        exit_code: None,
        done: false,
    });

    assert_eq!(dropped, 0);
    assert!(pending.response.is_none());
    assert!(pending.outputs.is_empty());
}

#[test]
fn pending_command_reports_retained_output_truncation() {
    let job_id = uuid::Uuid::new_v4();
    let (response, _receiver) = oneshot::channel();
    let mut pending = PendingCommand {
        client_id: "client-a".to_string(),
        job_id,
        command_version: 1,
        payload_hash: "payload-a".to_string(),
        ack: None,
        outputs: Vec::new(),
        response: Some(response),
    };

    let mut dropped = 0_u64;
    for _ in 0..(MAX_RETAINED_COMMAND_OUTPUTS + 2) {
        dropped += pending.retain_output_if_response_waiting(CommandOutput {
            job_id,
            stream: OutputStream::Stdout,
            data: b"line\n".to_vec(),
            exit_code: None,
            done: false,
        });
    }

    assert_eq!(dropped, 2);
    assert_eq!(pending.outputs.len(), MAX_RETAINED_COMMAND_OUTPUTS);
}
