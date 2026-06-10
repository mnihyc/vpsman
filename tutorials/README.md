# vpsman Tutorials

These tutorials are operator-facing guides for using the system. They assume
the architecture and security model from `../DESIGN.md`: agents connect to the
gateway over plain TCP with Noise protection, browsers and CLI keep the super
password local, and privileged operations send request-bound assertions that
the private gateway verifies.

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
# First-time setup prints both the gateway verifier and operator salt:
cargo run -p vpsctl -- privilege-verifier --generate-salt
export VPSMAN_SUPER_SALT_HEX=<super_salt_hex_from_output>
```

`VPSMAN_API_TOKEN` authenticates the operator to the API. The super password
and salt stay local to the browser/CLI/VTY and are used to build request-bound
privilege assertions for the private gateway. Put only the generated
`privilege_verifier_key_hex` into gateway env as
`VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX`; never put the password, salt, or verifier
into API or agent envs. Keep operator-side material out of shell history where
possible.
