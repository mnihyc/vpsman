# Operator Access Scopes

The API container and gateway-control interface are private operator
control-plane services and must not be published directly. The operator-facing
origin may deliberately serve an expiring read-only monitoring share through
the bundled frontend/Nginx path. Only the allowlisted public-share routes are
unauthenticated by an operator bearer session; all operator API routes keep their
normal scope checks.

The unauthenticated visitor namespace is limited to
`/api/v1/public/monitoring-shares/{share_id}/bootstrap` and its `/data` history
endpoint. Creation, listing, extension, revocation, and all other management
remain authenticated under `/api/v1/monitoring-shares`.

Operator tokens carry explicit scopes. `admin` has `*`. The default `operator`
role receives the scopes needed for normal daily operation. The default
`viewer` role receives only `fleet:read`.

## Read Scopes

- `fleet:read`: fleet metadata, agent inventory, gateway sessions, monitoring
  cards and per-VPS resource/network/Ping history, job status, target status,
  fleet and policy alert state, traffic accounting summaries, topology
  summaries, the narrow canonical product-name display projection, and other
  non-payload operational status. It does not expose generic VPS rule records.
- `jobs:read`: durable job output payloads, output archives, file-download
  payloads, output chunks, output comparison previews, process-supervisor
  inventory, file-transfer session records, file-transfer source artifacts, and
  file-transfer handoff downloads.
- `backups:read`: backup requests, backup policies, restore plans, backup
  artifact metadata/downloads, migration-link listings, fleet alert
  evidence views/exports that include backup evidence, and backup-artifact
  history exports.
- `terminal:read`: terminal session records and retained PTY replay bytes.
- `integrations:read`: webhook rules, webhook dry runs, webhook deliveries,
  alert notification channels, and alert notification deliveries.
- `templates:read`: built-in and user-defined command templates and their operation payloads.
- `schedules:read`: saved schedule definitions, target snapshots, timing, and
  recurring operation payloads.
- `config:read`: configuration preset definitions, the typed per-VPS desired
  configuration workspace and provenance, saved sparse overrides, override and
  bulk previews, readiness and runtime-sync state, optional redacted live-agent
  runtime evidence, runtime config patch generators, rendered incremental
  config patches, per-VPS rule values, rule-aware VPS selector
  evaluation and suggestions, Config > Rules dry-runs, and private agent-update
  release metadata. Any live selector using `vps.rules` requires this scope in
  addition to the operation's normal scope; unavailable rule evidence is an
  explicit error rather than an empty match.
- `network:read`: full tunnel plans, exportable runtime `plan.json` details,
  OSPF update plans, port-forward desired/runtime state, general Ping-target
  definitions/assignments/history, hostname-resolution candidates, raw network
  observations, and topology history exports.
- `audit:read`: audit logs and audit history exports.
- `sharing:read`: monitoring-share management metadata, saved selector and
  exact frozen target IDs/count, visible groups, lifecycle state, creator,
  visitor count, and first/last access evidence. It does not expose the bearer
  URL; unauthenticated public projections use separate allowlisted records and
  never expose those IDs.

## Write Scopes

Existing write scopes remain separate from read scopes. Examples include
`jobs:write`, `inventory:write`, `schedules:write`, `backups:write`,
`network:write`, `config:write`, `integrations:write`, `templates:write`,
`history:write`, and `sharing:write`. `sharing:write` permits an operator to
create, update frozen targets, extend, or revoke monitoring shares and recover
an active bearer URL with **Copy URL**. The same scope already authorizes share
creation and its returned bearer URL; `sharing:read` remains the separate scope
for listing management records and target evidence.

Config > Rules writes require `config:write`. Alert Policy mutations require
`integrations:write`, `fleet:read`, and `backups:read`, because one policy can
own agent, job, backup, and capability evidence. Notification-channel writes
require `integrations:write`.

Cron schedule mutations use the existing schedule/job authority. Alert-event
schedule create, edit, enable, target refresh, and server-authoritative argv
preview additionally require `fleet:read` and `backups:read` alongside
`schedules:write` and `jobs:write`. The worker rechecks the captured actor's
same scopes before dispatch, so a saved definition cannot outlive revoked alert
visibility. Event expressions cannot read mutable VPS fields; their fixed
reviewed targets remain separate from the policy evidence subject.

Per-VPS override replacement/reset and reviewed bulk incremental runtime-config
apply require `config:write`. Reading live agent evidence additionally dispatches
an explicit ConfigRead job under the existing job-dispatch authority; the page
does not poll it or use it as the saved override base.

Tunnel-plan mutations and the reviewed, plan-scoped **Clear evidence** action
require `network:write`. Clearing evidence does not grant access to job or audit
payloads and does not change tunnel runtime state.

Port-forward create, update, enable, disable, reapply, bulk, and delete actions
require `network:write`. Forgetting an unconfirmed removal tombstone additionally
requires the `admin` role because it discards host-cleanup evidence.

General Ping-target create/edit, enable/disable, frozen-target update, primary
assignment, and delete actions also require `network:write`. Their definitions
and assignment evidence remain readable through `network:read`.

History retention writes require `history:write` plus authority for the selected
domain. Audit retention requires `audit:read`; job-output retention requires
`jobs:write`; backup-artifact retention requires `backups:write`; network
observation and topology retention require `network:write`; telemetry and
system rollups, accepted high-resolution telemetry, long-term resource,
network, and Ping history, authoritative traffic counters, client lifecycle,
and gateway session retention require `inventory:write`.

Server artifact cleanup requires explicit cleanup domains. `job_output` and
`file_transfer` cleanup require `jobs:write`; `backup_artifact` cleanup requires
`backups:write`. The preview hash binds the reviewed domains and matched
artifacts.

## Public Monitoring Share Boundary

Creating a shared view resolves and freezes an exact VPS list, visible metric
groups, detail-history permission, and expiry. The default expiry is 24 hours;
the accepted range is one minute through 365 days. An active share's frozen
targets change only through a reviewed **Update targets** action against its
saved selector; visibility cannot change after creation. Active links can be
extended, capped at 365 days from extension time, or revoked immediately and
irreversibly. Expired and revoked links cannot be reactivated.

The returned URL carries a high-entropy secret in its browser fragment. The
control plane stores the recoverable high-entropy token so authenticated
operators can use **Copy URL** later; treat that URL and database field as bearer
credentials. Each authenticated recovery is recorded in audit evidence and its
response is never cacheable. Public data requests require the share secret plus a
visitor bootstrap identity, but never an operator token or privilege assertion.
Each distinct visitor creates one `monitoring_share.visitor_opened` audit event
with its share ID, visitor ID, source IP, bounded User-Agent, frozen target count,
and visibility. Subsequent polling updates last-accessed evidence without
creating another audit event for that visitor.

Public projections always include display name and health. Depending on
the immutable visibility selection, they may also include allowlisted
provider/region/country tags whose values are not IP literals, the optional
configured product name, resources, network rate, authoritative traffic,
billing display, normalized system information, general Ping, and detail history.
Billing and system information
are independent opt-in groups and omit missing facts. The projection never
includes raw `os-release`, hostname, IP addresses, capability payloads, build or
process identities, or per-interface evidence. They use
a persisted random 256-bit VPS key generated independently for each target in
each share. The key is stable for that share's lifetime and is never derived
from the URL-secret digest or predictable internal VPS ID. Public projections
never expose real VPS IDs, network-address fields, internal configuration,
actions, jobs, terminals, files, backups, audit data, or operator identity.
Operator-controlled display names, share names, optional product names (when
identity context is enabled), and Ping target names are included verbatim;
operators must keep sensitive addresses out of labels intended for public
sharing.

## Practical Defaults

When creating an operator with an empty scope list, the API applies defaults for
the selected role. For custom scope lists, include every read and write scope
the workflow needs. For example, a read-only operations user who can inspect job
outputs but not dispatch jobs needs at least:

```text
fleet:read,jobs:read
```

A normal non-admin operator typically uses the default role scopes rather than
a custom list. That default includes `sharing:read` and `sharing:write`; the
default viewer remains limited to `fleet:read` and cannot inspect or mutate
share-management records.

## User And Session Management

The dashboard manages accounts, roles, scopes, MFA, and active bearer sessions
under Access > Operators. Audit > Sessions provides searchable operator and
terminal evidence plus reviewed bulk revocation of active non-current bearer
sessions. Operators enroll TOTP under Access > Privilege vault by scanning the
QR code (the manual setup key remains available) and confirming the current
six-digit code. Setup first stores a disabled pending secret; TOTP becomes
enabled only after the password and a current code from that exact QR/setup key
both validate. A wrong-secret or invalid code leaves TOTP disabled and reports
failure; an accepted time step cannot be reused for a later TOTP mutation.
Reopening the same pending enrollment returns the same QR/setup key instead of
silently replacing it.

Access > Gateway sessions shows live and ended agent streams and edits the
current admin's reusable agent-installer endpoints, server public key, and
install mode. These defaults do not change gateway runtime binds or its private
key. Access > VPS identities owns agent key lifecycle, and Access > Privilege
vault owns local privilege unlock state.

Unlock verifies a non-mutating, request-bound assertion with the gateway before
the console reports success. The entered super password and privilege salt stay
in the browser. The console keeps only the derived signing capability in local
storage, bound to the signed-in operator, so refreshes and browser restarts
remain unlocked after gateway re-verification. Lock, sign-out, or an operator
change clears it. The optional encrypted local vault is separate and still
requires its local passphrase when used.

Identity registration, key rotation, key revocation, and VPS deletion separate
the durable identity change from follow-up work. The API returns explicit
gateway-disconnect and terminal-reconciliation outcomes after the committed
record. The console keeps the completion panel open and shows a warning when an
old gateway session may remain active, including the recovery path under
Access > Gateway sessions. VPS deletion also names every surviving tunnel peer
whose cleanup apply could not be queued. Operators must not repeat the primary
mutation merely because a follow-up outcome failed.

Operator usernames are immutable. Disabling or deleting an operator blocks
login, revokes that operator's sessions, preserves audit history, and prevents
the username from being reused.

Each operator has an explicit refresh/session TTL. The default is 365 days; the
access token lifetime is one day. Admin-targeted changes and changes that grant
the admin role require an explicit admin-risk acknowledgement in the dashboard,
CLI, VTY, and API payload.

Login throttling and auth history use the operator client IP. The bundled API
has no published host port and is reached through the frontend reverse proxy,
so its deployment config trusts the complete forwarded client chain. Nginx
preserves the `X-Forwarded-For` chain supplied by an external TLS provider and
appends its connection peer. Docker DNS routes the proxy to `api`; no container
address is pinned. A deployment that exposes the API directly must instead
restrict `[api].trusted_proxy_cidrs` or
`VPSMAN_TRUSTED_PROXY_CIDRS` to its actual proxy peers.

Authentication failures feed two bounded lockout buckets: one for the
username/client-IP pair and one for the client IP across usernames. A hostile
client therefore cannot lock an operator out from every network, while a single
source still cannot rotate usernames without being throttled. The historical
`operator_auth_username_failed_attempt_limit` setting controls the
username/client-IP bucket. Both buckets default to 8 failures within 15
minutes, followed by a 15-minute lockout.

Fleet WebSocket streams use the same bearer-session authority as HTTP routes.
The server periodically revalidates token expiry, session revocation, operator
status, and `fleet:read` scope, then closes streams that no longer have access.
