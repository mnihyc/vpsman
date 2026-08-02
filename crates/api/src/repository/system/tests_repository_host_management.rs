use super::*;
use uuid::Uuid;

#[test]
fn successful_evidence_is_not_lost_behind_recent_failed_attempts() {
    let successful_job_id = Uuid::new_v4();
    let mut attempts = (0..201)
        .map(|_| HostJobAttemptView {
            job_id: Uuid::new_v4(),
            status: "failed".to_string(),
            message: None,
            completed_at: None,
        })
        .collect::<Vec<_>>();
    attempts.push(HostJobAttemptView {
        job_id: successful_job_id,
        status: "completed".to_string(),
        message: None,
        completed_at: None,
    });

    let evidence = host_job_evidence_from_newest_attempts(&attempts);

    assert_eq!(
        evidence
            .latest_attempt
            .as_ref()
            .map(|attempt| attempt.status.as_str()),
        Some("failed")
    );
    assert_eq!(evidence.latest_success_job_id, Some(successful_job_id));
}
