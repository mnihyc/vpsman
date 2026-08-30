use super::{
    append_process_supervisor_inventory, build_process_supervisor_inventory,
    ensure_process_supervisor_inventory_complete, SupervisorInventoryOutput,
    PROCESS_SUPERVISOR_INVENTORY_PAGE_SIZE, PROCESS_SUPERVISOR_INVENTORY_SCAN_LIMIT_ERROR,
};
use std::collections::BTreeSet;
use uuid::Uuid;

#[test]
fn builds_deduplicated_supervisor_inventory_from_latest_outputs() {
    let start_job = Uuid::new_v4();
    let status_job = Uuid::new_v4();
    let outputs = vec![
        SupervisorInventoryOutput {
            job_id: status_job,
            client_id: "edge-a".to_string(),
            stream: "stdout".to_string(),
            data: serde_json::to_vec(&serde_json::json!({
                "type": "process_status",
                "processes": [{
                    "name": "ospf-worker",
                    "status": "running",
                    "pid": 4242,
                    "started_unix": 1700000000_u64,
                    "stdout_log": "/tmp/ospf.stdout.log",
                    "stderr_log": "/tmp/ospf.stderr.log",
                    "restart_attempts": 2,
                    "last_exit_code": 7,
                    "last_exit_unix": 1700000010_u64,
                    "last_restart_unix": 1700000011_u64,
                    "limit_effectiveness": {
                        "overall": { "status": "degraded_desired_only" }
                    },
                    "cgroup_status": {
                        "status": "available",
                        "process_count": 2,
                        "cpu_weight": 39,
                        "memory_current_bytes": 1048576,
                        "pids_current": 2
                    }
                }]
            }))
            .unwrap(),
            created_at: "200".to_string(),
            command_type: "process_status".to_string(),
        },
        SupervisorInventoryOutput {
            job_id: start_job,
            client_id: "edge-a".to_string(),
            stream: "status".to_string(),
            data: serde_json::to_vec(&serde_json::json!({
                "type": "process_start",
                "name": "ospf-worker",
                "status": "running",
                "pid": 4000
            }))
            .unwrap(),
            created_at: "100".to_string(),
            command_type: "process_start".to_string(),
        },
    ];

    let inventory = build_process_supervisor_inventory(outputs, 50);

    assert_eq!(inventory.len(), 1);
    assert_eq!(inventory[0].client_id, "edge-a");
    assert_eq!(inventory[0].name, "ospf-worker");
    assert_eq!(inventory[0].pid, Some(4242));
    assert_eq!(inventory[0].source_job_id, status_job);
    assert_eq!(inventory[0].source_command_type, "process_status");
    assert_eq!(inventory[0].restart_attempts, Some(2));
    assert_eq!(inventory[0].last_exit_code, Some(7));
    assert_eq!(inventory[0].last_exit_unix, Some(1700000010));
    assert_eq!(inventory[0].last_restart_unix, Some(1700000011));
    assert_eq!(
        inventory[0].limit_effectiveness_status.as_deref(),
        Some("degraded_desired_only")
    );
    assert_eq!(inventory[0].cgroup_status.as_deref(), Some("available"));
    assert_eq!(inventory[0].cgroup_process_count, Some(2));
    assert_eq!(inventory[0].cgroup_cpu_weight, Some(39));
    assert_eq!(inventory[0].cgroup_memory_current_bytes, Some(1048576));
    assert_eq!(inventory[0].cgroup_pids_current, Some(2));
}

#[test]
fn ignores_non_inventory_output_shapes() {
    let inventory = build_process_supervisor_inventory(
        vec![SupervisorInventoryOutput {
            job_id: Uuid::new_v4(),
            client_id: "edge-a".to_string(),
            stream: "stdout".to_string(),
            data: b"not json".to_vec(),
            created_at: "100".to_string(),
            command_type: "process_status".to_string(),
        }],
        50,
    );

    assert!(inventory.is_empty());
}

#[test]
fn supervisor_inventory_continues_past_repeated_output_pages() {
    let process_output = |name: &str, created_at: usize| SupervisorInventoryOutput {
        job_id: Uuid::new_v4(),
        client_id: "edge-a".to_string(),
        stream: "stdout".to_string(),
        data: serde_json::to_vec(&serde_json::json!({
            "type": "process_status",
            "processes": [{ "name": name, "status": "running" }]
        }))
        .unwrap(),
        created_at: format!("{created_at:05}"),
        command_type: "process_status".to_string(),
    };
    let mut outputs = (1..=5_001)
        .rev()
        .map(|created_at| process_output("frequent", created_at))
        .collect::<Vec<_>>();
    outputs.push(process_output("quiet", 0));
    let mut outputs = outputs.into_iter();
    let mut seen = BTreeSet::new();
    let mut inventory = Vec::new();
    loop {
        let page = outputs
            .by_ref()
            .take(PROCESS_SUPERVISOR_INVENTORY_PAGE_SIZE as usize)
            .collect::<Vec<_>>();
        if page.is_empty() {
            break;
        }
        if append_process_supervisor_inventory(page, &mut seen, &mut inventory, 2) {
            break;
        }
    }

    assert_eq!(inventory.len(), 2);
    assert_eq!(inventory[0].name, "frequent");
    assert_eq!(inventory[1].name, "quiet");
}

#[test]
fn supervisor_inventory_never_returns_an_incomplete_bounded_scan() {
    ensure_process_supervisor_inventory_complete(2, 2, false).unwrap();
    ensure_process_supervisor_inventory_complete(1, 2, true).unwrap();
    assert_eq!(
        ensure_process_supervisor_inventory_complete(1, 2, false)
            .unwrap_err()
            .to_string(),
        PROCESS_SUPERVISOR_INVENTORY_SCAN_LIMIT_ERROR
    );
}
