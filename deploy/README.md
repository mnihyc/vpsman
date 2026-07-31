# Deploy Directory Layout

This directory is the Docker Compose deployment root. It can be renamed or
copied outside a source checkout; paths below are relative to this directory.
Production installations should use the exact-tag
`vpsman-deploy-vX.Y.Z.tar.gz` asset from the selected GitHub release. See the
[production deployment and recovery runbook](../docs/production-deployment.md)
for pinned installation, network exposure, backup, restore, upgrade, and
rollback procedures. The same runbook is stored at
`docs/production-deployment.md` inside a release deployment bundle.

```text
.
|-- .env                         # local compose environment; not committed
|-- .env.example                 # template for .env
|-- compose.yml                  # compose service graph and volume mounts
|-- nginx.conf                   # frontend reverse-proxy config
|-- update.sh                    # manifest-driven release start/update/rollback
|-- AGENT_GATEWAY_INSTALL.md     # agent install notes
|-- vpsctl -> runtime/cli/current/vpsctl  # updater-created host CLI link
|-- config/
|   |-- vpsman.toml              # authoritative non-secret suite config
|   `-- secrets/                 # generated local secret files; not committed
`-- runtime/                     # persistent/runtime state; not committed
    |-- postgres/
    |   `-- data/                # PostgreSQL data directory
    |-- data/
    |   |-- gateway-control.sock # gateway control socket when running
    |   |-- gateway-spool/       # gateway delivery overflow/replay spool
    |   `-- objects/
    |       `-- backups/         # filesystem object store root
    |           |-- backups/                 # retained backup artifacts
    |           |-- job-outputs/             # large retained job outputs
    |           |-- file-transfers/          # file-transfer handoff artifacts
    |           `-- file-transfer-sources/   # uploaded source artifacts
    |-- downloads/               # downloaded release metadata
    |-- update-backups/          # automatic pre-activation database dumps
    |-- transactions/            # interrupted update recovery state
    |-- update.lock              # updater concurrency lock
    |-- server/
    |   |-- current/             # active server release payload
    |   `-- previous/            # rollback server payload
    |-- frontend/
    |   |-- current/             # active frontend release payload
    |   `-- previous/            # rollback frontend payload
    `-- cli/
        |-- current/             # active host CLI release payload; contains vpsctl
        `-- previous/            # rollback host CLI payload
```

The versioned release bundle also carries `LICENSE-APACHE`, `LICENSE-MIT`,
`SECURITY.md`, and the production/migration runbooks under `docs/`.
`RELEASE_TAG` starts with the bundle tag and is then maintained atomically by
the updater as the authoritative active payload tag.

## Persistence Model

Compose mounts `./runtime/data` into API, gateway, and worker containers as
`/var/lib/vpsman`. The default suite config keeps the filesystem object store
under `/var/lib/vpsman/objects/backups`, so object data persists on the host at
`runtime/data/objects/backups`.

Compose mounts `./runtime/postgres/data` into PostgreSQL as
`/var/lib/postgresql/data`. PostgreSQL stores artifact metadata, job history,
retention state, operators, schedules, and other control-plane records.

Deleting containers or recreating services does not delete these bind-mounted
runtime directories. Deleting `runtime/data` removes filesystem object data.
Deleting `runtime/postgres/data` removes the database metadata and should be
treated as destructive.

Compose caps each container at five rotated 10 MiB JSON log files by default;
`.env` can override the size and file count. vpsman informational events and
Nginx access records use stdout, while warnings/errors use stderr. Dependency
logs default to warning level, with a separate filter for each vpsman service.

## Config Versus Runtime

`config/vpsman.toml` is the single compose suite config for non-secret product
settings. Runtime data stays under `runtime/`; do not move the config into
`runtime/`.

Job execution budgets are bounded by `[timeout].max_job_timeout_secs`.
The default is 3600 seconds; raise it explicitly for multi-hour maintenance
jobs so API validation, worker schedules, frontend signing, CLI submission, and
agent capability checks share the same maximum.

Tunnel endpoint allocation pools are under `[network]`. They are empty by
default; set IPv4 and/or IPv6 pool CIDRs before using Generate endpoints or
`tunnel-allocate` without explicit pool arguments.
Admins can edit the same global pools in **System > Suite Config > Network**;
Advanced TOML is not required.

The gateway `[gateway].api_url` is intentionally plain `http://` for the
private compose network or another trusted private network. Expose TLS at the
operator-facing reverse proxy; do not publish the internal API or gateway
control endpoints directly.

The shipped Nginx proxy allows request bodies up to `25m` on `/api/`. This
covers the largest supported Base64-expanded JSON request used by the current
file, backup-chunk, and source-artifact workflows. It is a per-request proxy
ceiling, not the artifact-retention limit: larger supported artifacts must use
the product's chunked upload or binary streaming paths rather than one JSON
request.

`config/secrets/` is generated by `vpsctl compose-secrets` and mounted into
containers through `/run/secrets/...` or read-only secret mounts. Keep these
files private.

## Release Payloads

`update.sh first-start latest` installs release assets into
`runtime/server/current`, `runtime/frontend/current`, and
`runtime/cli/current`, creates missing compose secrets when
`VPSMAN_SUPER_PASSWORD` is set, snapshots PostgreSQL, then starts compose. The
snapshot makes API migrations reversible if activation fails, including for a
restored database. A successful first-start prints the generated
`VPSMAN_SUPER_SALT_HEX`; save it for browser and CLI privilege unlock. The
persistent copy remains in `./config/secrets/operator-privilege.env`.

After first start, open the browser console. An empty control plane shows
**Create first operator** and signs in the initial admin after creation. Once an
operator exists, the same unauthenticated page shows **Sign in**.

The current canonical database is intentionally fresh-only and does not support
an in-place update from an earlier schema model; review
[migration compatibility](../docs/migration-compatibility.md) before updating
an older deployment.

`update.sh latest` updates the same three release payloads for an existing
deployment. It validates authoritative `version.json` metadata, asset layouts,
and migration compatibility, stops application writers, stores a PostgreSQL
dump under `runtime/update-backups/`, activates the release transactionally,
and verifies readiness and release identity. Repeating the active tag is a
no-op only after the version manifest, current payload contents, and live build
readiness all match. Same-tag manifest drift, corruption, or unready services
fail closed without replacing the older rollback payload.
Rollback applies the same safeguards before swapping `current` and `previous`
for server, frontend, and CLI together. Use `update.sh recover` after an
interrupted transaction. Successful activation updates `RELEASE_TAG`; a rollback
to a legacy payload without embedded release metadata removes the marker rather
than leaving a false recovery identity.

The host CLI is `runtime/cli/current/vpsctl`. Because the API container is not
published directly, point the CLI at the Nginx origin:

```sh
./runtime/cli/current/vpsctl --api-url http://127.0.0.1:5173 health
```

Payload update and rollback do not replace `compose.yml`, `nginx.conf`, `.env`,
suite configuration, or secrets, and they do not reverse external effects.
Review the target versioned deployment bundle and take an encrypted off-host
control-plane backup before upgrading; the updater's local database dump is an
additional recovery layer, and product-managed VPS backups do not cover the
control plane itself. Automatic first-start, update, and rollback dumps are
mode-restricted but are not encrypted or pruned; monitor
`runtime/update-backups/` and apply the deployment's reviewed retention policy.
The application payload is selected by the exact release manifest, but the
upstream Compose image tags are mutable; the production runbook documents that
residual and the operator-owned digest-pinning boundary.
