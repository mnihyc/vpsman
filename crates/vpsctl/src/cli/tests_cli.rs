use clap::Parser;

use super::{Args, Command};
use crate::commands_schedules::{ScheduleRunOnArg, ScheduleTriggerKindArg};

const TYPED_ALERT_RULE_JSON: &str = r#"{"name":"offline","enabled":true,"rule_kind":"state","evidence_source":"agent.status","correlation_mode":"natural_key","trigger_condition_expression":"evidence.status = offline","resolve_condition_expression":"evidence.status = online","resolve_meta_condition":{"kind":"sustained","seconds":60},"severity":"critical","category":"agent_status","title_template":"Agent offline","detail_template":"{subject.display_name} is offline"}"#;

#[test]
fn alert_policy_cli_accepts_typed_rule_json_and_full_policy_files() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let parsed = Args::try_parse_from([
                "vpsctl",
                "alert-policy",
                "preview",
                "--name",
                "offline-agents",
                "--selector",
                "tag:edge",
                "--rule-json",
                TYPED_ALERT_RULE_JSON,
            ])
            .unwrap();
            let Command::AlertPolicy(command) = parsed.command else {
                panic!("expected alert-policy command");
            };
            let crate::cli_access::AlertPolicySubcommand::Preview(request) = command.command else {
                panic!("expected alert-policy preview command");
            };
            assert_eq!(request.selector, "tag:edge");
            assert_eq!(request.rule_json, vec![TYPED_ALERT_RULE_JSON.to_string()]);

            let parsed = Args::try_parse_from([
                "vpsctl",
                "alert-policy",
                "upsert",
                "--name",
                "offline-agents",
                "--file",
                "./offline-policy.json",
                "--confirmed",
            ])
            .unwrap();
            let Command::AlertPolicy(command) = parsed.command else {
                panic!("expected alert-policy command");
            };
            let crate::cli_access::AlertPolicySubcommand::Upsert(request) = command.command else {
                panic!("expected alert-policy upsert command");
            };
            assert_eq!(
                request.file.as_deref(),
                Some(std::path::Path::new("./offline-policy.json"))
            );
            assert!(request.rule_json.is_empty());
            assert!(request.confirmed);
        })
        .expect("spawn CLI parser test")
        .join()
        .expect("CLI parser test panicked");
}

#[test]
fn alert_policy_cli_rejects_removed_shorthand_flags() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            for (flag, value) in [
                ("--rule", "evidence.status = offline"),
                ("--window-secs", "60"),
                ("--severity", "critical"),
                ("--traffic-selector", "eth0"),
            ] {
                let mut args = vec![
                    "vpsctl",
                    "alert-policy",
                    "preview",
                    "--name",
                    "offline-agents",
                    "--selector",
                    "tag:edge",
                    "--rule-json",
                    TYPED_ALERT_RULE_JSON,
                ];
                args.extend([flag, value]);
                assert!(Args::try_parse_from(args).is_err(), "accepted {flag}");
            }
        })
        .expect("spawn CLI parser test")
        .join()
        .expect("CLI parser test panicked");
}

#[test]
fn schedule_create_preserves_cron_defaults_without_compatibility_flags() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let parsed = Args::try_parse_from([
                "vpsctl",
                "schedule-create",
                "--name",
                "hourly",
                "--command",
                "/bin/true",
                "--clients",
                "edge-a",
                "--confirmed",
            ])
            .unwrap();
            let Command::ScheduleCreate(request) = parsed.command else {
                panic!("expected schedule-create command");
            };
            assert_eq!(request.trigger_kind, ScheduleTriggerKindArg::Cron);
            assert_eq!(request.run_on, None);
            assert_eq!(request.max_failures, -1);
            assert_eq!(request.command.as_deref(), Some("/bin/true"));
            assert!(request.cron_expr.is_none());
            assert!(request.catch_up_policy.is_none());
        })
        .expect("spawn CLI parser test")
        .join()
        .expect("CLI parser test panicked");
}

#[test]
fn schedule_failure_tolerance_cli_accepts_signed_values_and_defaults() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            for command in ["schedule-create", "schedule-update", "backup-policy-upsert"] {
                for value in [None, Some("-1"), Some("0"), Some("100")] {
                    let mut args = vec!["vpsctl", command, "--name", "failure-tolerance"];
                    if command == "schedule-update" {
                        args.extend(["--schedule-id", "11111111-1111-4111-8111-111111111111"]);
                    }
                    if let Some(value) = value {
                        args.extend(["--max-failures", value]);
                    }
                    let parsed = Args::try_parse_from(args).unwrap();
                    let actual = match parsed.command {
                        Command::ScheduleCreate(request) => request.max_failures,
                        Command::ScheduleUpdate(request) => request.max_failures,
                        Command::BackupPolicyUpsert { max_failures, .. } => max_failures,
                        _ => panic!("unexpected command"),
                    };
                    assert_eq!(actual, value.unwrap_or("-1").parse::<i32>().unwrap());
                }
            }
        })
        .expect("spawn CLI parser test")
        .join()
        .expect("CLI parser test panicked");
}

#[test]
fn schedule_create_accepts_an_explicit_alert_event_shape() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let parsed = Args::try_parse_from([
                "vpsctl",
                "schedule-create",
                "--name",
                "traffic-limit",
                "--trigger-kind",
                "event",
                "--run-on",
                "all-at-once",
                "--event-expression",
                "alert.triggered && alert.category:traffic",
                "--event-argv-template",
                "/usr/local/bin/limit-traffic",
                "--event-argv-template",
                "{event.kind}",
                "--event-argv-template",
                "{alert.target_id}",
                "--tags",
                "edge",
                "--confirmed",
            ])
            .unwrap();
            let Command::ScheduleCreate(request) = parsed.command else {
                panic!("expected schedule-create command");
            };
            assert_eq!(request.trigger_kind, ScheduleTriggerKindArg::Event);
            assert_eq!(request.run_on, Some(ScheduleRunOnArg::AllAtOnce));
            assert!(request.command.is_none());
            assert_eq!(
                request.event_argv_template,
                vec![
                    "/usr/local/bin/limit-traffic",
                    "{event.kind}",
                    "{alert.target_id}"
                ]
            );
        })
        .expect("spawn CLI parser test")
        .join()
        .expect("CLI parser test panicked");
}

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
fn fou_cli_accepts_only_typed_encapsulation_flags() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let base = [
                "vpsctl",
                "tunnel-plan",
                "--name=fou-cli",
                "--interface-name=foucli",
                "--kind=fou",
                "--left-client-id=left",
                "--right-client-id=right",
                "--left-remote-underlay=192.0.2.2",
                "--right-remote-underlay=192.0.2.1",
                "--left-tunnel-ipv4-cidr=10.0.0.0/31",
                "--right-tunnel-ipv4-cidr=10.0.0.1/31",
                "--bandwidth-mbps=100",
            ];
            for kind in vpsman_common::RuntimeTunnelFouKind::ALL {
                let flag = format!("--fou-tunnel-kind={}", kind.linux_tunnel_mode());
                let parsed = Args::try_parse_from(base.into_iter().chain([flag.as_str()])).unwrap();
                let Command::TunnelPlan(request) = parsed.command else {
                    panic!("expected tunnel-plan");
                };
                assert_eq!(
                    request
                        .fou_tunnel_kind
                        .map(vpsman_common::RuntimeTunnelFouKind::from),
                    Some(kind)
                );
            }
            // Family validation belongs to the shared planner, not Clap parsing.
            for invalid in [
                "--fou-ipproto=47",
                "--fou-tunnel-kind=47",
                "--fou-tunnel-kind=gretap",
            ] {
                assert!(
                    Args::try_parse_from(base.into_iter().chain([invalid])).is_err(),
                    "{invalid}"
                );
            }
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
            let base = [
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
            ];
            let parsed = Args::try_parse_from(base);
            let Command::TunnelPlan(request) = parsed.unwrap().command else {
                panic!("expected tunnel-plan command");
            };
            assert!(request.manage_link_local);
            assert!(request.additional_left_ipv4.is_empty());
            assert!(request.additional_left_ipv6.is_empty());
            assert!(request.additional_right_ipv4.is_empty());
            assert!(request.additional_right_ipv6.is_empty());
            let parsed = Args::try_parse_from(base.into_iter().chain([
                "--additional-left-ipv4=192.0.2.1/32,192.0.2.2/32",
                "--additional-left-ipv4",
                "192.0.2.3/32",
                "--additional-right-ipv4=192.0.2.4/32",
                "--additional-left-ipv6=fe80::1/64",
                "--additional-right-ipv6",
                "fd00::2/128,fe80::2/64",
                "--manage-link-local=false",
            ]))
            .unwrap();
            let Command::TunnelPlan(request) = parsed.command else {
                panic!("expected tunnel-plan command");
            };
            assert_eq!(
                request.additional_left_ipv4,
                ["192.0.2.1/32", "192.0.2.2/32", "192.0.2.3/32"]
            );
            assert_eq!(request.additional_right_ipv4, ["192.0.2.4/32"]);
            assert_eq!(request.additional_left_ipv6, ["fe80::1/64"]);
            assert_eq!(request.additional_right_ipv6, ["fd00::2/128", "fe80::2/64"]);
            assert!(!request.manage_link_local);
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

#[test]
fn network_traffic_import_vnstat_accepts_explicit_hosts_interfaces_and_start() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let parsed = Args::try_parse_from([
                "vpsctl",
                "network-traffic-import-vnstat",
                "--clients",
                "edge-a,edge-b",
                "--interface",
                "eth0,ens3",
                "--start",
                "2024-08-01",
                "--confirmed",
            ])
            .unwrap();
            let Command::NetworkTrafficImportVnstat(request) = parsed.command else {
                panic!("expected network-traffic-import-vnstat command");
            };
            assert_eq!(request.clients, ["edge-a", "edge-b"]);
            assert_eq!(request.interfaces, ["eth0", "ens3"]);
            assert_eq!(request.start, "2024-08-01");
            assert!(request.confirmed);
        })
        .expect("spawn CLI parser test")
        .join()
        .expect("CLI parser test panicked");
}

#[test]
fn network_traffic_import_vnstat_allows_interface_discovery() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let parsed = Args::try_parse_from([
                "vpsctl",
                "network-traffic-import-vnstat",
                "--clients",
                "edge-a",
                "--start",
                "2024-08-01",
                "--confirmed",
            ])
            .unwrap();
            let Command::NetworkTrafficImportVnstat(request) = parsed.command else {
                panic!("expected network-traffic-import-vnstat command");
            };
            assert!(request.interfaces.is_empty());
        })
        .expect("spawn CLI parser test")
        .join()
        .expect("CLI parser test panicked");
}

#[test]
fn ospf_cli_preserves_directional_plan_options_and_requires_two_apply_targets() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let base = [
                "vpsctl",
                "tunnel-plan",
                "--name=edge",
                "--interface-name=tun0",
                "--kind=gre",
                "--left-client-id=left",
                "--right-client-id=right",
                "--left-remote-underlay=198.51.100.10",
                "--right-remote-underlay=203.0.113.20",
                "--left-tunnel-ipv4-cidr=10.255.0.0/31",
                "--right-tunnel-ipv4-cidr=10.255.0.1/31",
                "--bandwidth-mbps=100",
            ];
            let Command::TunnelPlan(defaults) = Args::try_parse_from(base).unwrap().command else {
                panic!("expected tunnel-plan");
            };
            assert!(!defaults.ospf);
            assert_eq!(vpsman_common::OspfControlMode::from(defaults.ospf_mode), vpsman_common::OspfControlMode::Automatic);
            assert_eq!(defaults.ospf_cost_floor, 5);
            assert_eq!(defaults.ospf_left_cost_offset, 0.0);
            assert_eq!(defaults.ospf_right_cost_offset, 0.0);
            assert_eq!(defaults.ospf_left_cost_multiplier, 1.0);
            assert_eq!(defaults.ospf_right_cost_multiplier, 1.0);
            let parsed = Args::try_parse_from(base.into_iter().chain([
                "--ospf",
                "--ospf-latency-ms=32.5",
                "--ospf-cost-floor=10",
                "--ospf-left-cost-offset",
                "-18",
                "--ospf-right-cost-offset=28",
                "--ospf-left-cost-multiplier=0.5",
                "--ospf-right-cost-multiplier=1.49",
            ])).unwrap();
            let Command::TunnelPlan(request) = parsed.command else {
                panic!("expected tunnel-plan");
            };
            assert_eq!(vpsman_common::OspfControlMode::from(request.ospf_mode), vpsman_common::OspfControlMode::Automatic);
            let reviewed = Args::try_parse_from(base.into_iter().chain([
                "--ospf", "--ospf-latency-ms=32.5", "--ospf-mode=reviewed"
            ])).unwrap();
            let Command::TunnelPlan(reviewed) = reviewed.command else {
                panic!("expected tunnel-plan");
            };
            assert_eq!(vpsman_common::OspfControlMode::from(reviewed.ospf_mode), vpsman_common::OspfControlMode::Reviewed);
            assert_eq!(request.ospf_cost_floor, 10);
            assert_eq!(request.ospf_left_cost_offset, -18.0);
            assert_eq!(request.ospf_right_cost_offset, 28.0);
            assert_eq!(request.ospf_left_cost_multiplier, 0.5);
            assert_eq!(request.ospf_right_cost_multiplier, 1.49);
            for flag in [
                "--ospf-cost-floor=5",
                "--ospf-left-cost-offset=0",
                "--ospf-right-cost-offset=0",
                "--ospf-left-cost-multiplier=1",
                "--ospf-right-cost-multiplier=1",
            ] {
                assert!(Args::try_parse_from(base.into_iter().chain([flag])).is_err(), "{flag}");
            }
            for flag in ["--ospf-cost-floor=0", "--ospf-cost-floor=65536", "--ospf-cost-floor=1.5"] {
                assert!(Args::try_parse_from(base.into_iter().chain(["--ospf", "--ospf-latency-ms=32.5", flag])).is_err(), "{flag}");
            }

            let apply = [
                "vpsctl",
                "tunnel-ospf-cost-update",
                "--plan-id=00000000-0000-0000-0000-000000000001",
                "--plan-revision=7",
                "--recommendation-id=ospf-1234abcd5678ef90",
                "--left-current-ospf-cost=50",
                "--right-current-ospf-cost=75",
                "--left-adapter-definition-hash=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "--right-adapter-definition-hash=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "--confirmed",
            ];
            let parsed = Args::try_parse_from(apply.into_iter().chain([
                "--left-desired-ospf-cost=55", "--right-desired-ospf-cost=75"
            ])).unwrap();
            let Command::TunnelOspfCostUpdate(request) = parsed.command else {
                panic!("expected tunnel-ospf-cost-update");
            };
            assert_eq!(request.left_desired_ospf_cost, 55);
            assert_eq!(request.right_desired_ospf_cost, 75);
            for targets in [
                vec!["--desired-ospf-cost=55"],
                vec!["--left-desired-ospf-cost=55"],
                vec!["--right-desired-ospf-cost=75"],
                vec!["--left-desired-ospf-cost=0", "--right-desired-ospf-cost=75"],
            ] {
                assert!(Args::try_parse_from(apply.into_iter().chain(targets)).is_err());
            }
        })
        .expect("spawn CLI parser test")
        .join()
        .expect("CLI parser test panicked");
}
