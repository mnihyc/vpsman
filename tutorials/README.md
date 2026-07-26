# vpsman Tutorials

These tutorials are operator-facing guides for using the system. They assume
the architecture and security model documented in `../README.md` and
`../docs/operator-access-scopes.md`: agents connect to the gateway over plain
TCP with Noise protection, browsers and CLI keep the super password local, and
privileged operations send request-bound assertions that the private gateway
verifies.

Use these in order for a new deployment:

1. `00-operator-quickstart.md`: shortest path from local control plane to
   registered VPS, fleet view, privileged command, backup, and update check.
2. `01-local-control-plane.md`: run the API, gateway, worker, and panel locally.
3. `02-install-agents.md`: create direct gateway identity material, install root or
   unprivileged agents, and reinstall rebuilt VPSs.
4. `03-fleet-organization.md`: organize 20+ VPSs with tags, bulk targeting,
   and alerts.
5. `04-daily-operations.md`: run commands, inspect job/audit history, manage
   retention/export, use terminal sessions, file transfers, process
   supervision, and schedules.
6. `05-source-templates.md`: choose default, shared, and VPS-local data
   source presets without hardcoding provider assumptions.
7. `06-tunnels-routing-adapters.md`: declare native, observed, or
   adapter-managed tunnels; inspect topology and tests; and optionally control
   OSPF costs through operator-owned adapters.
8. `07-backup-restore-migration.md`: create backups, restore, roll back
   restores, and link rebuilt-VPS migration records.
9. `08-agent-updates.md`: publish, stage, activate, and roll back agent
   updates.
10. `09-headless-cli-vty.md`: operate the system without the browser panel.

Command examples use the development form:

```sh
cargo run -p vpsctl -- <command>
```

For a versioned deployment bundle, run from the bundle root and replace that
prefix with `./runtime/cli/current/vpsctl`.

Common environment when running the control plane manually from source:

```sh
export VPSMAN_API_URL=http://127.0.0.1:8080
export VPSMAN_API_TOKEN=<operator_token>
export VPSMAN_SUPER_PASSWORD=<local_super_password>
export VPSMAN_SUPER_SALT_HEX=<64_hex_salt>
```

The Compose deployment intentionally keeps port 8080 private. For its bundled
CLI, use the Nginx console/API origin instead:

```sh
export VPSMAN_API_URL=http://127.0.0.1:5173
```

`VPSMAN_API_TOKEN` authenticates the operator to the API. The super password
and salt stay local to the browser/CLI/VTY and are used to build request-bound
privilege assertions for the private gateway. Keep them out of shell history
where possible.
