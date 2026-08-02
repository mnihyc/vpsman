use super::*;

#[test]
fn recognizes_job_follow_as_job_output_command() {
    assert!(is_vty_job_output_command(
        "job-follow 11111111-2222-4333-8444-555555555555"
    ));
    assert!(is_vty_job_output_command(
        "job-follow 11111111-2222-4333-8444-555555555555 --json"
    ));
}
