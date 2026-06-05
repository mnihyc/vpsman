# vpsman Tutorials

These tutorials are operator-facing guides for using the system. They assume
the architecture and security model from `../DESIGN.md`: agents connect to the
gateway over plain TCP with Noise protection, privileged operations are
authorized by local proof generation, and the super password is never sent to
the API.

Use these in order for a new deployment:

1. `00-operator-quickstart.md`: shortest path from local control plane to
   enrolled VPS, fleet view, proof-gated command, backup, and update check.
2. `01-local-control-plane.md`: run the API, gateway, worker, and panel locally.
3. `02-enroll-agents.md`: create enrollment material, install root or
   unprivileged agents, and re-enroll rebuilt VPSs.
4. `03-fleet-organization.md`: organize 20+ VPSs with tags, bulk targeting,
   and alerts.
5. `04-daily-operations.md`: run commands, inspect job/audit history, manage
   retention/export, use terminal sessions, file transfers, process
   supervision, and schedules.
6. `05-data-source-presets.md`: choose default, shared, and VPS-local data
   source presets without hardcoding provider assumptions.
7. `06-tunnels-topology-bird2.md`: manage runtime-owned tunnels, imported
   tunnels, topology, probes, speed tests, and Bird2 OSPF costs.
8. `07-backup-restore-migration.md`: create backups, restore, roll back
   restores, and link rebuilt-VPS migration records.
9. `08-agent-updates.md`: publish, stage, activate, and roll back agent
   updates.
10. `09-headless-cli-vty.md`: operate the system without the browser panel.

Command examples use the development form:

```sh
cargo run -p vpsctl -- <command>
```

For installed deployments, replace that prefix with `vpsctl`.

Common environment:

```sh
export VPSMAN_API_URL=http://127.0.0.1:8080
export VPSMAN_API_TOKEN=<operator_token>
export VPSMAN_SUPER_PASSWORD=<local_super_password>
export VPSMAN_SUPER_SALT_HEX=<64_hex_salt>
```

`VPSMAN_API_TOKEN` authenticates the operator to the API. The super password
and salt are used locally by browser/CLI/VTY proof generation for privileged
commands. Keep them out of shell history where possible.
