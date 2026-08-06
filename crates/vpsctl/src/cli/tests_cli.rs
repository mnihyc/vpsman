use clap::Parser;

use super::{Args, Command};

#[test]
fn backup_policy_upsert_accepts_an_explicit_update_target() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let schedule_id =
                uuid::Uuid::parse_str("52ff9113-03bd-4fa5-a166-3243681826fe").unwrap();
            let parsed = Args::try_parse_from([
                "vpsctl",
                "backup-policy-upsert",
                "--schedule-id",
                "52ff9113-03bd-4fa5-a166-3243681826fe",
                "--name",
                "nightly-edge",
                "--include-config",
                "--clients",
                "edge-a",
                "--confirmed",
            ])
            .unwrap();
            let Command::BackupPolicyUpsert {
                schedule_id: parsed_schedule_id,
                ..
            } = parsed.command
            else {
                panic!("expected backup-policy-upsert command");
            };
            assert_eq!(parsed_schedule_id, Some(schedule_id));
        })
        .expect("spawn CLI parser test")
        .join()
        .expect("CLI parser test panicked");
}

#[test]
fn backup_policy_listing_accepts_explicit_page_bounds() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let parsed = Args::try_parse_from([
                "vpsctl",
                "backup-policies",
                "--limit",
                "500",
                "--offset",
                "1000",
            ])
            .unwrap();
            let Command::BackupPolicies { limit, offset } = parsed.command else {
                panic!("expected backup-policies command");
            };
            assert_eq!(limit, 500);
            assert_eq!(offset, 1000);
        })
        .expect("spawn CLI parser test")
        .join()
        .expect("CLI parser test panicked");
}

#[test]
fn agent_update_check_activation_is_explicit() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let parsed = Args::try_parse_from([
                "vpsctl",
                "agent-update-check",
                "--clients",
                "edge-a",
                "--confirmed",
            ])
            .unwrap();
            let Command::AgentUpdateCheck {
                activate,
                restart_agent,
                ..
            } = parsed.command
            else {
                panic!("expected agent-update-check command");
            };
            assert!(!activate);
            assert!(!restart_agent);

            let parsed = Args::try_parse_from([
                "vpsctl",
                "agent-update-check",
                "--activate",
                "--restart-agent",
                "--clients",
                "edge-a",
                "--confirmed",
            ])
            .unwrap();
            let Command::AgentUpdateCheck {
                activate,
                restart_agent,
                ..
            } = parsed.command
            else {
                panic!("expected agent-update-check command");
            };
            assert!(activate);
            assert!(restart_agent);
        })
        .expect("spawn CLI parser test")
        .join()
        .expect("CLI parser test panicked");
}

#[test]
fn tunnel_plan_defaults_do_not_enable_or_require_ospf() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let parsed = Args::try_parse_from([
                "vpsctl",
                "tunnel-plan",
                "--name",
                "edge",
                "--interface-name",
                "tun0",
                "--kind",
                "gre",
                "--left-client-id",
                "left",
                "--right-client-id",
                "right",
                "--left-remote-underlay",
                "198.51.100.10",
                "--right-remote-underlay",
                "203.0.113.20",
                "--left-tunnel-ipv4-cidr",
                "10.255.0.0/31",
                "--right-tunnel-ipv4-cidr",
                "10.255.0.1/31",
                "--bandwidth-mbps",
                "100",
            ]);
            assert!(parsed.is_ok(), "{parsed:?}");
        })
        .expect("spawn CLI parser test")
        .join()
        .expect("CLI parser test panicked");
}

#[test]
fn tunnel_plan_credential_rotation_reuses_the_reviewed_mutation_shape() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let parsed = Args::try_parse_from([
                "vpsctl",
                "tunnel-plan-rotate-credentials",
                "--plan-id",
                "00000000-0000-4000-8000-000000000001",
                "--expected-revision",
                "7",
                "--confirmed",
            ])
            .unwrap();
            let Command::TunnelPlanRotateCredentials(request) = parsed.command else {
                panic!("expected tunnel-plan-rotate-credentials command");
            };
            assert_eq!(request.plan_id, "00000000-0000-4000-8000-000000000001");
            assert_eq!(request.expected_revision, Some(7));
            assert!(request.confirmed);
        })
        .expect("spawn CLI parser test")
        .join()
        .expect("CLI parser test panicked");
}
