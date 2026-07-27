# Production Deployment and Recovery

This runbook covers the vpsman control plane. Managed-VPS backup and restore
jobs are separate product workflows; they do not back up the control-plane
database, deployment secrets, or release state.

## Install a Pinned Deployment Bundle

Production installations should start from a versioned GitHub release bundle,
not a mutable source branch or `latest`. Set the repository and reviewed stable
tag, then verify the bundle against the release checksum manifest:

```sh
export VPSMAN_RELEASE_REPO=mnihyc/vpsman
export VPSMAN_RELEASE_TAG=vX.Y.Z
release_url="https://github.com/${VPSMAN_RELEASE_REPO}/releases/download/${VPSMAN_RELEASE_TAG}"

curl -fLO "${release_url}/vpsman-deploy-${VPSMAN_RELEASE_TAG}.tar.gz"
curl -fLO "${release_url}/SHA256SUMS"
grep "  vpsman-deploy-${VPSMAN_RELEASE_TAG}.tar.gz$" SHA256SUMS \
  > SHA256SUMS.deploy
test "$(wc -l < SHA256SUMS.deploy)" -eq 1
sha256sum -c SHA256SUMS.deploy

tar -xzf "vpsman-deploy-${VPSMAN_RELEASE_TAG}.tar.gz"
cd "vpsman-deploy-${VPSMAN_RELEASE_TAG}"
```

The control-plane host currently needs x86-64 Linux, Docker Engine with
Compose, `cmp`, `curl`, `diff`, `flock`, `python3`, `sha256sum`, `sha384sum`,
`tar`, and `unzip`. ARM64 remains supported for agents and the standalone
`vpsctl` artifact, but the published server bundle is currently x86-64 only.

The bundle and `SHA256SUMS` pin the vpsman application payloads, but the shipped
Compose file currently uses upstream major/minor image tags rather than
repository-owned digest pins. Those PostgreSQL, Debian, and Nginx tags are
mutable, so two otherwise pinned installations can resolve different container
image bytes. Production operators that require fully reproducible images must
mirror and digest-pin reviewed images in their maintained Compose file, record
those digests with the control-plane backup, and carry the pins forward while
reviewing deployment-file changes. The project does not currently publish
authoritative image digests.

Before first start:

1. Copy `.env.example` to `.env`, set a unique URL-safe PostgreSQL password,
   and restrict the file to its operator account.
2. Review `config/vpsman.toml`; keep the internal API and gateway-control
   interfaces private.
3. Keep `config/secrets/`, `.env`, and `runtime/` out of source control, logs,
   and unencrypted backup destinations.
4. Generate the first secret set through the updater with a strong local super
   password:

```sh
cp .env.example .env
chmod 600 .env
# Edit .env and config/vpsman.toml now.

umask 077
export VPSMAN_SUPER_PASSWORD='<local_super_password>'
./update.sh first-start "$VPSMAN_RELEASE_TAG"
unset VPSMAN_SUPER_PASSWORD

curl -fsS http://127.0.0.1:5173/health
./runtime/cli/current/vpsctl --version
```

Create the first operator before allowing other users to reach the console.
Treat the one-time bootstrap boundary as administrative access.
First-start snapshots PostgreSQL before launching the API, including when the
database was restored from a backup. If activation fails, `update.sh` restores
that snapshot before removing the failed payload.

## Network Exposure

The shipped defaults bind the browser console and raw TCP agent gateway to
loopback. Keep them that way for local evaluation.

For remote operators, put the console behind an operator-owned HTTPS reverse
proxy or private network and set `VPSMAN_FRONTEND_BIND` only to the address that
proxy needs. Do not expose the API container, `/internal/`, the PostgreSQL
service, or the gateway-control interface.

Remote agents need a reachable raw TCP endpoint; this is not an HTTP/WebSocket
route. Set a deliberate publish mapping such as
`VPSMAN_GATEWAY_PUBLISH=0.0.0.0:9443:9443`, restrict the host firewall to the
expected source networks where practical, configure NAT forwarding when
needed, and advertise the externally reachable endpoint to agents. Agent
traffic is authenticated and encrypted with the configured Noise identities,
but that does not replace host firewalling, rate limiting, monitoring, or key
rotation.

## Control-Plane Backup

Define an RPO, retention period, encryption method, and off-host destination
before production use. The following creates a consistent maintenance-window
backup for the default filesystem object store:

```bash
set -Eeuo pipefail
cd /path/to/the/versioned-deployment
umask 077

[[ -d runtime ]] || {
  echo "runtime directory is missing; reconcile the deployment before backup" >&2
  exit 1
}
exec 9>runtime/update.lock
if ! flock -n 9; then
  echo "another update, rollback, recovery, or backup is already running" >&2
  exit 1
fi

[[ -f RELEASE_TAG ]] || {
  echo "RELEASE_TAG is missing or invalid; reconcile the active release before backup" >&2
  exit 1
}
active_release="$(sed -n '1p' RELEASE_TAG)"
if [[ ! "$active_release" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$ ]]; then
  echo "RELEASE_TAG is missing or invalid; reconcile the active release before backup" >&2
  exit 1
fi

backup_dir="../vpsman-control-plane-$(date -u +%Y%m%dT%H%M%SZ)"
if ! mkdir -m 0700 -- "$backup_dir"; then
  echo "backup destination already exists or cannot be created: $backup_dir" >&2
  exit 1
fi
dump_partial="$backup_dir/.postgres.dump.partial"
dump_final="$backup_dir/postgres.dump"
archive_partial="$backup_dir/.deployment-files.tar.gz.partial"
archive_final="$backup_dir/deployment-files.tar.gz"
application_services_stopped=0

restart_application_services() {
  if [[ "$application_services_stopped" -eq 1 ]]; then
    docker compose start api gateway worker frontend
  fi
}

publish_no_clobber() {
  local partial="$1" final="$2"

  mv -T --no-clobber -- "$partial" "$final"
  if [[ -e "$partial" || -L "$partial" ]]; then
    echo "refusing to overwrite unexpected backup destination: $final" >&2
    return 1
  fi
  [[ -f "$final" && ! -L "$final" ]] || {
    echo "backup publication did not create a regular file: $final" >&2
    return 1
  }
}
trap restart_application_services EXIT

application_services_stopped=1
docker compose stop api gateway worker frontend
docker compose exec -T postgres sh -ec \
  'pg_dump --format=custom --username="$POSTGRES_USER" --dbname="$POSTGRES_DB"' \
  >"$dump_partial"
[[ -s "$dump_partial" ]] || {
  echo "PostgreSQL backup is empty" >&2
  exit 1
}
docker compose exec -T postgres sh -ec \
  'exec pg_restore --exit-on-error --file=/dev/null' \
  <"$dump_partial"
publish_no_clobber "$dump_partial" "$dump_final"

tar -czf "$archive_partial" \
  .env compose.yml nginx.conf config runtime/data runtime/downloads RELEASE_TAG
[[ -s "$archive_partial" ]] || {
  echo "deployment-files archive is empty" >&2
  exit 1
}
tar -tzf "$archive_partial" >/dev/null
publish_no_clobber "$archive_partial" "$archive_final"

docker compose start api gateway worker frontend
application_services_stopped=0
curl -fsS http://127.0.0.1:5173/health
trap - EXIT
```

The shared `runtime/update.lock` prevents backup, update, rollback, and recovery
from overlapping. Stopping the four application services freezes control-plane
mutations while PostgreSQL remains available for `pg_dump`. The example
exclusively creates a mode-0700 destination, fully reads the custom-format dump
with `pg_restore`, validates the deployment archive, and publishes each
artifact with a same-filesystem, no-clobber rename. A same-second rerun or an
unexpected final file therefore aborts instead of overwriting evidence. If any
backup command fails, restart the stopped services after preserving the partial
artifact for diagnosis. Do not archive `runtime/postgres/data` while PostgreSQL
is running and do not treat a raw data directory as a portable substitute for
`pg_dump`.

`RELEASE_TAG` is created by the versioned bundle and updated atomically only
after the updater commits a healthy first start, update, or metadata-bearing
rollback. The validation above prevents a legacy rollback without embedded
release metadata from producing an ambiguous recovery archive.
The deployment archive also retains the maintained Compose and Nginx files,
including any reviewed local image-digest pins or ingress changes.

`runtime/data` includes the default object store and gateway spool. If
`config/vpsman.toml` points to an external object store, snapshot that store
with matching retention and consistency guarantees as a separate step.

Encrypt the database dump and deployment archive before sending them off-host.
The archive contains database credentials, gateway keys, privilege-verifier
material, internal tokens, backup objects, job output, and transferred files.
Regularly test that a retained backup can be decrypted and restored.

## Restore and Disaster Recovery

Test recovery on an isolated network first. Block the raw gateway port so a
rehearsal cannot compete with the production control plane for live agents.

1. Read and export the tag with
   `export VPSMAN_RELEASE_TAG="$(tar -xOf deployment-files.tar.gz RELEASE_TAG)"`,
   then obtain and
   checksum-verify that exact deployment bundle. Review
   [Migration Compatibility](migration-compatibility.md) before choosing a
   different release.
2. Extract the deployment bundle, then extract `deployment-files.tar.gz` into
   its root. Confirm `.env`, `config/secrets/`, and `runtime/data` permissions.
3. Start only PostgreSQL and wait until it accepts connections.
4. Restore the custom-format dump.
5. Download the pinned release payload, start the stack, and verify health,
   operator authentication, agent identity, object access, and audit history.

```sh
set -Eeuo pipefail
docker compose up -d postgres
postgres_ready=0
for attempt in $(seq 1 60); do
  if docker compose exec -T postgres sh -ec \
    'pg_isready --username="$POSTGRES_USER" --dbname="$POSTGRES_DB"'
  then
    postgres_ready=1
    break
  fi
  sleep 2
done
if [[ "$postgres_ready" -ne 1 ]]; then
  echo "PostgreSQL did not become ready within 120 seconds" >&2
  exit 1
fi

docker compose exec -T postgres sh -ec \
  'pg_restore --clean --if-exists --no-owner --no-privileges \
    --username="$POSTGRES_USER" --dbname="$POSTGRES_DB"' \
  < /path/to/postgres.dump

./update.sh first-start "$VPSMAN_RELEASE_TAG"
curl -fsS http://127.0.0.1:5173/health
docker compose ps
```

Do not bypass SQLx migration checksum failures or edit `_sqlx_migrations`.
Releases `v0.1.0` through `v0.1.3` cannot currently be upgraded in place to the
current migration baseline; restore them with their matching release or wait
for a tested compatibility bridge.

## Upgrade and Rollback

Before every production upgrade:

1. Read the target release notes, this runbook, `SECURITY.md`, and
   [Migration Compatibility](migration-compatibility.md).
2. Take and test a control-plane backup.
3. Download and checksum-verify the target deployment bundle.
4. Compare its `.env.example`, `compose.yml`, `nginx.conf`, `update.sh`,
   `install-agent.sh`, and `config/vpsman.toml` with the installed deployment.
   Merge reviewed deployment changes without overwriting `.env`,
   `config/secrets/`, or `runtime/`.
5. Run `./update.sh vX.Y.Z`, then check `/health`, `docker compose ps`, recent
   service logs, operator sign-in, and a canary agent before wider operations.

Use `latest` only for disposable environments; it makes change review and
recovery less reproducible. Prereleases are not production-supported.

For an existing deployment, the updater verifies the target migration history,
stops application writers, stores a PostgreSQL dump under
`runtime/update-backups/`, activates all three payloads as one transaction, and
checks readiness and release identity. If an interrupted transaction remains,
run `./update.sh recover` and preserve its evidence if automated recovery
refuses to proceed. This local safeguard does not replace an encrypted,
off-host control-plane backup. These sensitive local dumps are mode-restricted
but are not automatically encrypted or pruned; monitor their disk use and apply
the reviewed retention policy after recovery points are safely stored off-host.
First-start uses the same pre-activation database safeguard, including for the
restore workflow above. Successful activation also updates `RELEASE_TAG`; do
not hand-edit it to a tag that is not actually active.

`./update.sh rollback` transactionally swaps the server, frontend, and CLI
payloads with their previous copies after checking migration compatibility and
taking another database dump. It does not reverse database migrations,
deployment file changes, configuration changes, or external side effects. If
the previous release cannot safely read the upgraded database, rollback refuses
the transition; restore the full pre-upgrade database and deployment backup
instead. Keep the gateway blocked until exactly one recovered control plane is
authoritative.
