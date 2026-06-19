# AUDIT.md

Production-impact audit issue ledger.

Use this file to persist verified production-impact issues only. Keep each issue
as its own record. Do not store design negotiations, implementation plans,
transcripts, or broad investigation notes here.

When adding an issue, add one index row and one matching issue section before
the template. Prefer deduping against existing entries over recording variants
of the same root cause.

## Index

| ID | Severity | Status | Area | Title |
| --- | --- | --- | --- | --- |
| AUD-001 | Medium/High | Fixed | Agent/File Archive And Download | Former archive/download symlink dereference handling |
| AUD-002 | Medium/High | Fixed | API/Auth | Operator login and TOTP failures have no server-side throttling |
| AUD-003 | Medium | Fixed | API/WebSocket/Auth | Fleet WebSocket stream bypasses fleet-read scope checks |
| AUD-004 | Medium/High | Fixed | API/Job Outputs/Auth | Job-output payload bytes are readable with fleet-read scope |
| AUD-005 | High | Fixed | API/Object Storage | Backup/update object-store separation |
| AUD-006 | Medium/High | Fixed | API/Worker/Artifact Cleanup | Artifact cleanup metadata/object ordering |
| AUD-007 | Medium/High | Fixed | Worker/Schedules/Client Lifecycle | One deleted fixed target can fail an entire multi-target schedule |
| AUD-008 | Medium/High | Fixed | Worker/Schedules/Agent Update | Scheduled update-family jobs bypass the busy-client skip policy |
| AUD-009 | Medium/High | Fixed | Agent/File Operations | Former unsafe file-copy overwrite and destination symlink handling |
| AUD-010 | Medium | Fixed | Agent/File Operations | Former chmod top-level symlink handling |
| AUD-011 | High | Fixed | Agent/Process Supervisor | Supervised process log growth and tail-read safety |
| AUD-012 | Medium/High | Fixed | Agent/Process Supervisor | Supervisor process-record durability |
| AUD-013 | Medium/High | Fixed | API/Agent/Network OSPF | OSPF cost updates mutate runtime config without updating the canonical tunnel plan |
| AUD-014 | Medium/High | Fixed | Agent/File Read And Download | Direct file read/download byte-cap enforcement |
| AUD-015 | Medium/High | Fixed | Agent/Backup | Backup plaintext byte-limit enforcement during file growth |
| AUD-016 | Medium/High | Fixed | API/Terminal/Auth | Terminal replay returns PTY bytes with fleet-read scope |
| AUD-017 | High | Fixed | API/Worker/Webhooks/Auth | Webhook rule and delivery reads expose outbound targets and rendered payloads |
| AUD-018 | Medium/High | Fixed | API/Command Templates/Auth | Saved command templates expose full operation payloads to fleet readers |
| AUD-019 | Medium/High | Fixed | API/Schedules/Auth | Schedule listings expose full recurring job operations to fleet readers |
| AUD-020 | Medium/High | Fixed | API/Alerts/Auth | Alert notification channels and deliveries expose outbound targets and payloads |
| AUD-021 | Medium/High | Fixed | API/Data Sources/Auth | Data-source presets and rendered hot-config expose executable config to fleet readers |
| AUD-022 | Medium/High | Fixed | API/Hot Config/Auth | Hot-config rule templates expose raw generators and rendered patches |
| AUD-023 | Medium/High | Fixed | API/Network/Auth | Tunnel plan reads expose runtime commands and generated network config |
| AUD-024 | Medium/High | Fixed | API/Agent Updates/Auth | Hosted update artifacts were exposed as public API downloads |
| AUD-025 | Medium/High | Fixed | API/File Transfers/Auth | File-transfer artifact reads used fleet metadata or write scopes instead of jobs-read |
| AUD-026 | Medium/High | Fixed | API/History/Auth | History export default can include payload domains with fleet-read only |
| AUD-027 | Medium/High | Fixed | API/Backups/Auth | Backup and restore read surfaces use fleet metadata or write scopes |
| AUD-028 | Critical | Fixed | Frontend/Job Dispatch | Job dispatch confirmation used mutable operation state after review |
| AUD-029 | Critical | Fixed | Frontend/Backups/Restore | Backup and restore confirmations used mutable form state after review |
| AUD-030 | High | Fixed | Frontend/Data Sources | Data-source apply and lifecycle confirmations used mutable state after review |
| AUD-031 | Critical | Fixed | Frontend/Topology | Network apply confirmations use mutable plan, side, backend, and option state after review |
| AUD-032 | Critical | Fixed | Frontend/Topology/OSPF | OSPF cost update confirmation uses mutable plan, side, target, and cost state after review |
| AUD-033 | High | Fixed | Frontend/Topology/Adapters | Tunnel adapter promotion confirmation uses mutable adapter contract after review |
| AUD-034 | Critical | Fixed | Frontend/Access/Keys | Gateway identity import and key revoke confirmations use mutable key lifecycle fields |
| AUD-035 | Critical | Fixed | Frontend/Config | Single-VPS config apply confirmation uses mutable TOML payload after review |
| AUD-036 | Medium/High | Confirmed | Frontend/Webhooks | Webhook queue dispatch confirmation can send a different event than reviewed |
| AUD-037 | High | Confirmed | Frontend/Audit Retention | Audit history prune confirmation uses mutable prune domain and mode after review |
| AUD-038 | Medium/High | Confirmed | Frontend/Webhook Retention | Webhook delivery cleanup deletes using live filters instead of the reviewed preview |
| AUD-039 | High | Confirmed | Frontend/Topology/Automation | Monitoring automation bulk action submits privileged config patches without review |
| AUD-040 | High | Confirmed | Frontend/Agent Updates | Update release registry records artifact hashes without a review confirmation |
| AUD-041 | Medium/High | Confirmed | Frontend/Fleet Tags | Inline Fleet tag mutations bypass preview confirmation and schedule impact review |
| AUD-042 | High | Confirmed | Frontend/Alerts/Webhooks | Alert and webhook configuration saves bypass required operator review |
| AUD-043 | High | Confirmed | Frontend/Alerts | Fleet alert triage actions bypass required operator review |
| AUD-044 | Medium/High | Confirmed | Frontend/File Transfers | Source and handoff artifact persistence bypasses operator review |
| AUD-045 | Medium/High | Confirmed | Frontend/Command Templates | Command template saves persist reusable operation payloads without review |
| AUD-046 | High | Fixed | API/CLI/Auth | Operator access-management mutations bypass or auto-confirm the confirmation contract |
| AUD-047 | Medium/High | Confirmed | API/Backups/Auth | Migration-link listings expose restore metadata with fleet-read scope |
| AUD-048 | High | Confirmed | API/History/Auth | History retention prune can delete job and backup payload history with inventory-write only |
| AUD-049 | High | Confirmed | API/Worker/Artifact Cleanup/Auth | Server artifact cleanup can delete backup artifacts with jobs-write only |
| AUD-050 | Critical | Fixed | API/Worker/Artifact Cleanup | Artifact cleanup jobs re-evaluate expressions instead of deleting the reviewed artifact set |
| AUD-051 | Medium/High | Confirmed | API/History/Artifact Cleanup | History retention object prune drops metadata before object deletion succeeds |
| AUD-052 | High | Confirmed | API/Data Sources/Auth | Data-source preset and assignment mutations use inventory-write instead of config-write |
| AUD-053 | High | Confirmed | API/Frontend/Data Sources | Data-source preset create path silently updates existing presets without review |
| AUD-054 | Medium/High | Confirmed | API/Config/Hot Config | Hot-config rule-template mutations lack confirmation and audit records |
| AUD-055 | Medium/High | Confirmed | Frontend/File Browser | File save confirmation can mark unsent editor changes as saved |
| AUD-056 | Medium/High | Confirmed | API/Worker/Backups/Retention | Backup policy retention prune drops backup metadata before object deletion succeeds |
| AUD-057 | High | Fixed | API/Auth/User Management | User management can remove the last active admin |
| AUD-058 | Medium/High | Confirmed | API/Integrations/Auth | Integration mutations use inventory-write instead of an integrations write boundary |
| AUD-059 | Medium/High | Confirmed | API/Command Templates/Auth | Command-template mutations use jobs-write instead of a templates write boundary |
| AUD-060 | Medium/High | Confirmed | API/Agent Updates/Auth | Update-release registry mutations use jobs-write instead of config-write |
| AUD-061 | High | Fixed | Frontend/System Users | User-management confirmations remain armed after editor or selection changes |
| AUD-062 | High | Confirmed | API/Object Storage/Artifacts | Artifact creation can commit metadata or bytes without cleanup-registry consistency |
| AUD-063 | High | Confirmed | Frontend/Schedules | Schedule confirmations remain armed after form, defer, or table context changes |
| AUD-064 | Medium/High | Confirmed | Frontend/Agent Updates | Release-registry manual update shortcut cannot provide the artifact URL it requires |
| AUD-065 | High | Confirmed | Frontend/Integrations | Delivery queue confirmations are not bound to previewed rows |
| AUD-066 | High | Fixed | API/Deploy/Security | API binary and suite config default to all-interface binding |
| AUD-067 | High | Fixed | Deploy/Nginx/API Boundary | Public frontend proxy exposes private API and WebSocket routes |
| AUD-068 | High | Fixed | API/CLI/Schedules | Schedule mutations lack an explicit backend confirmation contract |
| AUD-069 | Medium/High | Fixed | API/Backups | Chunked backup artifact commit ignores the confirmation flag |
| AUD-070 | High | Fixed | API/Frontend/CLI/Network | Tunnel-plan save and lifecycle mutations lack a backend confirmation contract |
| AUD-071 | Medium/High | Fixed | API/Frontend/CLI/Jobs | Job and server-job cancellation bypass the confirmation contract |
| AUD-072 | Medium/High | Confirmed | API/Frontend/CLI/Inventory/Selectors | Non-unique VPS display names make name selectors ambiguous for production jobs |
| AUD-073 | High | Confirmed | API/Agent/Terminal/Storage | Live terminal output can grow API job-output storage without a retention ceiling |
| AUD-074 | Medium/High | Confirmed | API/Object Storage/Job Outputs | Job-output object artifacts can be committed without cleanup-registry repair |
| AUD-075 | Medium/High | Confirmed | API/History/Auth | Audit logs are readable and exportable with fleet-read scope |
| AUD-076 | Medium/High | Confirmed | API/Gateway/Terminal/Reliability | Terminal stream output retries are not idempotent |
| AUD-077 | Medium/High | Confirmed | Gateway/API/Terminal/Lifecycle | Terminal final stream status can expire as noncritical output |
| AUD-078 | Medium/High | Confirmed | API/Network/Auth | OSPF update-plan reads expose generated Bird2 snippets with fleet-read scope |
| AUD-079 | High | Confirmed | API/Network/Auth | Network observations expose runtime command reports with fleet-read scope |
| AUD-080 | High | Confirmed | Gateway/Spool/Security | Gateway spool files persist the internal API bearer token |
| AUD-081 | High | Confirmed | API/Object Storage/Security | Filesystem object-store artifacts rely on default filesystem permissions |
| AUD-082 | Medium/High | Confirmed | API/Downloads/Security | Transient payload spool files in temp directories rely on default permissions |
| AUD-083 | High | Fixed | Agent/File Transfer/Security | Agent file-upload staging exposes payloads before final modes are applied |
| AUD-084 | High | Fixed | Agent/Updates/Reliability | Agent updater cannot follow the official GitHub release redirects |
| AUD-085 | Medium/High | Confirmed | CLI/Downloads/Security | vpsctl local download staging uses default-readable named temp files |
| AUD-086 | High | Fixed | Agent/Restore/Security | Agent restore staging exposes restored payloads before archive modes are applied |
| AUD-087 | High | Fixed | Agent/Restore/Safety | Restore destination roots can be escaped through symlinked parent components |
| AUD-088 | High | Confirmed | Agent/Backup/Safety | Backup jobs follow selected-path symlinks without an explicit operator choice |
| AUD-089 | Medium/High | Fixed | Agent/File Browser/Security | Text-file edit staging exposes payloads before final modes are applied |
| AUD-090 | Medium/High | Fixed | Agent/File Browser/Ownership | Chown on a symlink reports success while changing nothing |
| AUD-091 | High | Confirmed | Agent/Restore/Safety | Agent-local restore archives are not required to be hash-bound |
| AUD-092 | High | Confirmed | Agent/Restore/Reliability | Agent-local restore reads the entire archive into memory without a cap |
| AUD-093 | Medium/High | Confirmed | Agent/Config/Security | Hot config rewrites can lose restrictive config-file permissions |
| AUD-094 | High | Confirmed | API/Suite Config/Security | Suite config saves can widen secret-bearing config-file permissions |
| AUD-095 | High | Confirmed | API/Suite Config/Audit | Suite config audit redaction leaves database URLs visible |
| AUD-096 | Medium/High | Confirmed | API/Suite Config/Audit | Suite config can be applied without a durable audit record |
| AUD-097 | Medium/High | Confirmed | API/Suite Config/Audit | Suite config changed-key detection runs after redaction |
| AUD-098 | High | Confirmed | Frontend/Suite Config | Suite config save review can use a stale validation result for a newer draft |
| AUD-099 | Medium/High | Confirmed | API/Suite Config/Durability | Suite config file replacement is rename-only without fsync durability |
| AUD-100 | Medium/High | Confirmed | API/Auth/Audit | Locked login attempts can still flood durable audit logs |
| AUD-101 | Medium/High | Confirmed | Deploy/API/Suite Config | Official compose mounts the dashboard-editable suite config read-only |
| AUD-102 | High | Fixed | API/Frontend/Suite Config/Privilege | Suite config privilege assertion is not bound to the TOML payload |
| AUD-103 | Medium/High | Confirmed | API/Auth/Deploy | Login throttling and auth history use proxy IP instead of the operator IP |
| AUD-104 | Medium/High | Confirmed | API/Auth/TOTP | Authenticated TOTP management is an unthrottled password and code oracle |
| AUD-105 | Medium/High | Confirmed | API/File Transfers/Terminal/Retention | Derived session records can outlive the job-output evidence they require |
| AUD-106 | High | Confirmed | API/Backups/Object Storage | Backup artifact metadata can be recorded without object-store verification |
| AUD-107 | High | Confirmed | API/Schedules/Client Lifecycle | Stale fixed targets can block schedule management and apply-now |
| AUD-108 | High | Fixed | API/Jobs/State Machine | Terminal targets can leave the parent job active after a crash or side-effect error |
| AUD-109 | Medium/High | Fixed | Gateway/API/Job Outputs | Gateway spool replay treats sequence existence as full output acknowledgement |
| AUD-110 | High | Confirmed | Frontend/CLI/Backups/Migrations | Bundled migration-run can persist a migration link before restore dispatch succeeds |
| AUD-111 | Medium/High | Confirmed | API/CLI/Backups/Restore Plans | Restore plans can record config-restore intent that later restore-run rejects |
| AUD-112 | High | Fixed | API/Jobs/Client Lifecycle | Deleting or revoking a client can leave already-created queued targets unclaimable forever |
| AUD-113 | High | Fixed | API/Gateway/Key Lifecycle | Replacing a client public key does not invalidate the old live gateway session |
| AUD-114 | High | Fixed | API/Gateway/Client Lifecycle | Delete and key-revoke mark sessions ended without disconnecting the live gateway session |
| AUD-115 | Medium/High | Confirmed | API/WebSocket/Auth | Fleet WebSocket streams continue after token expiry, session revocation, or scope removal |
| AUD-116 | High | Confirmed | API/Integrations/Confirmation | Alert and webhook configuration delete routes lack backend confirmation |
| AUD-117 | Medium/High | Confirmed | Worker/Alerts/Reliability | Alert notification webhooks are not retried automatically after transient failures |
| AUD-118 | High | Confirmed | API/Integrations/Delivery State | Manual delivery processors can send in-progress webhooks before failing the state update |
| AUD-119 | High | Fixed | Agent/Updates/Lifecycle | Agent update activation can replace the binary before durable heartbeat evidence exists |
| AUD-120 | High | Fixed | API/Agent Updates/Lifecycle | Activation heartbeat completion trusts job ID without verifying the artifact hash |
| AUD-121 | High | Fixed | API/Access/Privilege | Agent trust-root and client deletion mutations bypass request-bound privilege verification |
| AUD-122 | Medium/High | Fixed | API/Gateway/Job Outputs | Late command output is durably accepted after the target is already terminal |
| AUD-123 | Medium/High | Confirmed | API/Process Supervisor/Auth | Process-supervisor inventory exposes job-output-derived process details with fleet-read scope |
| AUD-124 | Medium/High | Confirmed | API/Fleet Alerts/Auth | Fleet alert evidence exposes backup paths and artifact IDs with fleet-read scope |
| AUD-125 | High | Confirmed | API/Fleet Alerts/Webhooks | Fleet alert read routes can enqueue webhook integration events |
| AUD-126 | Medium/High | Confirmed | API/Data Sources/State | Data-source read paths persist default assignments for all clients, including hidden clients |
| AUD-127 | High | Confirmed | Gateway/Forwarder/Shutdown | Controlled gateway shutdown can lose queued RAM forwarder events |
| AUD-128 | High | Fixed | Agent/File Browser/Safety | Recursive file delete can escape through symlink-swap races |
| AUD-129 | Medium/High | Confirmed | Gateway/Terminal/Resource Bounds | Terminal output forwarding bypasses the gateway RAM spool budget |
| AUD-130 | High | Fixed | Agent/File Browser/Safety | Copy, chmod, and chown can follow symlinks after validation races |
| AUD-131 | High | Fixed | Agent/File Read And Download/Safety | Read and download paths can dereference symlinks after validation |
| AUD-132 | High | Fixed | API/Jobs/State Machine | Precompleted skipped targets are not atomic with job creation |
| AUD-133 | High | Fixed | Agent/File Upload/Safety | Upload staging pathnames can be swapped into symlinks before chmod, chown, chunk writes, or commit |
| AUD-134 | High | Fixed | Agent/Restore/Safety | Restore staging pathnames can be precreated or swapped into symlinks |
| AUD-135 | High | Fixed | Agent/File Browser/Safety | Text-write and copy staging pathnames can be swapped before chmod or commit |
| AUD-136 | Medium/High | Fixed | Agent/File Browser/Safety | Directory creation can chmod a swapped symlink target after mkdir |
| AUD-137 | Medium/High | Confirmed | API/Command Templates/Confirmation | Command-template delete route lacks backend confirmation |
| AUD-138 | Medium/High | Confirmed | API/CLI/Data Sources/Confirmation | Data-source preset updates can bypass confirmation for one assigned VPS |
| AUD-139 | Medium/High | Confirmed | CLI/VTY/Fleet Tags | CLI tag create and single-VPS assignment auto-confirm tag mutations |
| AUD-140 | Medium/High | Fixed | Frontend/File Browser | Single-file browser confirmations remain armed after operation edits |
| AUD-141 | High | Confirmed | Agent/Process Supervisor/Safety | Supervisor PID records can target reused host processes after agent restart |
| AUD-142 | High | Confirmed | Agent/Process Supervisor/Security | Supervisor records and logs are written with default-readable permissions |
| AUD-143 | Medium/High | Confirmed | Docs/Deployment/API Boundary | Headless CLI tutorial presents the public panel URL as the operator API endpoint |
| AUD-144 | High | Confirmed | API/Worker/Agent Updates | Strict registered-update policy only gates direct staging jobs |
| AUD-145 | High | Confirmed | API/Gateway/Key Lifecycle | Key rotation, revoke, and delete disconnect before DB invalidation, leaving a reconnect race |
| AUD-146 | High | Confirmed | Deploy/Nginx/API Boundary | Publishing the dashboard frontend still publishes API and WebSocket routes |
| AUD-147 | Medium/High | Confirmed | Deploy/Agent Install/Supply Chain | Custom agent binary URL installs without a required SHA-256 pin |
| AUD-148 | High | Confirmed | API/Frontend/CLI/Backups/Retention | Backup policy prune confirms scope and mode but reselects live artifacts instead of the reviewed candidate set |
| AUD-149 | High | Confirmed | Deploy/Update/Rollback | Compose update and rollback swap release directories without forcing container recreation |
| AUD-150 | High | Confirmed | Gateway/API/Telemetry/Lifecycle | Displaced gateway sessions can keep forwarding telemetry after replacement |
| AUD-151 | High | Fixed | API/Frontend/CLI/Auth/Privilege | Operator management mutations lack request-bound privilege verification |
| AUD-152 | High | Confirmed | Frontend/Backups/Migrations | Migration restore runs can use stale hidden restore options |
| AUD-153 | Medium/High | Confirmed | API/Telemetry/Retention | Per-interface network-rate telemetry has no retention path |
| AUD-154 | High | Confirmed | API/Frontend/CLI/History Retention | History retention prune reselects live rows instead of deleting the reviewed dry-run set |
| AUD-155 | High | Confirmed | Worker/Artifact Cleanup/Observability | Failed artifact cleanup jobs can hide already-deleted artifacts |
| AUD-156 | High | Confirmed | Agent/Process Supervisor/Command Semantics | Process status and log reads can restart supervised processes |
| AUD-157 | Medium/High | Confirmed | API/Gateway/Client Lifecycle/Retention | Client and gateway lifecycle histories have no retention path |
| AUD-158 | Medium/High | Confirmed | API/Worker/Webhooks/Retention | Webhook events in the default partition bypass event retention |
| AUD-159 | Medium/High | Confirmed | Worker/Webhooks/Retention/Alerts | Webhook permanent-failure deliveries bypass delivery retention and create unbounded alerts |
| AUD-160 | Medium/High | Confirmed | Worker/Webhooks/Retention/Config | Webhook-rule retention silently clamps the shipped 90-day setting to 7 days |
| AUD-161 | High | Confirmed | Worker/Server Jobs/Artifact Cleanup | Artifact cleanup server jobs can remain running forever after worker loss |
| AUD-162 | High | Confirmed | Agent/Updates/Safety | Update-check activation can downgrade agents from an older release manifest |
| AUD-163 | High | Confirmed | Agent/Custom Runtime Commands/Reliability | Custom JSON command timeouts can be bypassed after stdout closes |
| AUD-164 | High | Confirmed | Agent/Process Supervisor/Timeouts | Process supervisor stop and restart can mutate host state after command timeout |
| AUD-165 | High | Confirmed | Agent/Network Apply/Rollback | Managed network rollback rewrites files non-atomically and drops original modes |
| AUD-166 | Medium/High | Confirmed | API/File Transfers/Reliability | Duplicate resumable download chunks can poison server-side handoff |
| AUD-167 | Medium/High | Confirmed | API/Backups/Migrations/Privilege | Migration-link creation bypasses request-bound privilege verification |
| AUD-168 | Medium/High | Confirmed | API/Backups/Resource Bounds | Chunked backup artifact commit rehydrates the whole artifact in API memory |
| AUD-169 | Medium/High | Fixed | API/Backups/Restore Workflow | Agent backups can be valid above the API restore-preparation inline limit |
| AUD-170 | High | Fixed | Frontend/API/Backups/Key Custody | Dashboard restore preparation sends the backup private key to the API |
| AUD-171 | Critical | Fixed | API/Backups/Restore Payloads/Webhooks | Inline restore archives persist decrypted backup content in jobs and webhooks |
| AUD-172 | High | Fixed | API/Auth/User Management | Password reset preserves old-password-encrypted TOTP secrets |
| AUD-173 | High | Fixed | Frontend/Fleet Tags | Bulk tag preview races can apply a stale target set to a newer tag form |
| AUD-174 | High | Fixed | Frontend/Server Jobs/Artifact Cleanup | Artifact cleanup preview races can queue a stale cleanup set after expression edits |
| AUD-175 | High | Fixed | Frontend/Job Dispatch | Dispatch review can open a stale confirmation after operation or selector edits |
| AUD-176 | High | Fixed | Frontend/Config/Data Sources | Config and data-source review requests can open stale confirmations after edits |
| AUD-177 | Critical | Fixed | Frontend/Topology/Network | Network mutation review requests can open stale confirmations after topology edits |
| AUD-178 | Critical | Fixed | Frontend/Backups/Restore | Backup and restore review requests can open stale confirmations after edits |
| AUD-179 | Medium/High | Confirmed | API/Backups/Object Storage | Multiple backup artifacts can reference the same object key |
| AUD-180 | Medium/High | Confirmed | API/File Transfers/Artifact Cleanup | Reuploaded file-transfer source artifacts can inherit stale cleanup age |
| AUD-181 | High | Fixed | Frontend/Access/Keys | Key lifecycle review can open stale confirmations after key-field edits |
| AUD-182 | Medium/High | Confirmed | API/Gateway/Terminal/Lifecycle | Terminal stream output can append after the terminal-open target is terminal |
| AUD-183 | High | Fixed | Frontend/Fleet/Delete | VPS deletion confirmation can remain armed after fleet selection changes |
| AUD-184 | Critical | Fixed | Frontend/Jobs/Multi-File | Bulk file review can open stale confirmations after selector or operation edits |
| AUD-185 | High | Confirmed | Agent/API/Terminal | Terminal input sequencing can drop out-of-order or conflicting input |
| AUD-186 | Medium/High | Confirmed | Agent/Gateway/Terminal/Lifecycle | Terminal PTYs can survive disconnect or access revocation without reconciliation |
| AUD-187 | Medium/High | Confirmed | API/Frontend/History Retention | History retention policy saves ignore the confirmation contract |
| AUD-188 | High | Fixed | Agent/File Browser/Safety | File rename and move can follow path races outside the reviewed source or destination |
| AUD-189 | Medium/High | Confirmed | Deploy/Agent Install/Docs | Official agent install examples do not start the service they claim to start |
| AUD-190 | Medium/High | Confirmed | Deploy/Compose/Database | Secure compose password edits leave API and worker using the wrong Postgres credentials |
| AUD-191 | High | Confirmed | API/Gateway/Dispatch | Backup gateway endpoints cannot receive API dispatch, cancel, or lifecycle disconnect control |
| AUD-192 | Medium/High | Confirmed | Gateway/Deploy/Security | Gateway agent TCP listener still defaults to all-interface binding |
| AUD-193 | High | Confirmed | Gateway/API/Lifecycle | Gateway lifecycle events can expire before API accepts a new process incarnation |
| AUD-194 | High | Confirmed | Release/Updates/Supply Chain | Manual release workflow can publish tag-named update assets from the wrong commit |
| AUD-195 | Medium/High | Confirmed | API/Gateway/Security/Docs | Documented dev internal token bypasses placeholder startup validation |
| AUD-196 | Medium | Confirmed | Docs/Local Control Plane | Manual quickstart no longer starts a usable Postgres-backed API |
| AUD-197 | High | Confirmed | Deploy/API/Gateway/Secrets | API and worker containers can read gateway-only secret material |
| AUD-198 | Medium/High | Confirmed | API/Worker/Object Storage/Security | S3-compatible object store accepts plaintext HTTP endpoints for signed requests |
| AUD-199 | High | Confirmed | API/Frontend/Job Outputs/Resource Bounds | Job-output and file-download archive exports can exhaust API temp disk across targets |
| AUD-200 | High | Confirmed | API/Frontend/CLI/Job Outputs/Resource Bounds | Job-output listing and chunk downloads load entire output history without pagination |
| AUD-201 | High | Confirmed | API/Frontend/CLI/File Transfers/Resource Bounds | Server-side file-transfer handoff scans all client chunks and leaks temp files on failed assembly |
| AUD-202 | Medium/High | Confirmed | API/Backups/Resource Cleanup | Retained backup handoff leaks staging files when assembly fails |
| AUD-203 | Medium/High | Confirmed | API/Backups/Resource Bounds | Retained backup handoff rehydrates the whole artifact in API memory after streaming |
| AUD-204 | Medium/High | Confirmed | API/Frontend/CLI/Backups/Resource Cleanup | Abandoned chunked backup upload sessions can leave staging files indefinitely |
| AUD-205 | High | Confirmed | Agent/Backups/Restore | Restore post-hooks can fail without making the restore target fail safely |
| AUD-206 | Medium/High | Confirmed | API/Worker/Frontend/Alerts | Alert notification delivery kinds can be saved but cannot be delivered by the shipped worker |
| AUD-207 | High | Fixed | API/Worker/Schedules/Auth | Schedules keep dispatching privileged jobs after owner disable/delete or scope loss |
| AUD-208 | High | Fixed | Worker/Backups/Retention/Auth | Backup-policy retention prune can delete backups after policy owner loses authority |
| AUD-209 | High | Fixed | API/Worker/Server Jobs/Auth | Queued artifact cleanup can delete artifacts after creator disable/delete or scope loss |
| AUD-210 | Medium/High | Confirmed | CLI/Output/Security | vpsctl structured-output capture writes sensitive stdout to default-permission temp files |
| AUD-211 | High | Confirmed | API/CLI/Agent/Backups/Restore | Restore jobs do not bind the declared source backup to the submitted archive bytes |
| AUD-212 | Medium/High | Confirmed | Agent/API/User Sessions/Job Status | User-session inventory timeouts are reported as generic failures |
| AUD-213 | Medium/High | Confirmed | API/Frontend/Backups/Job Lifecycle | Failed backup jobs leave auto-created backup requests permanently in progress |
| AUD-214 | High | Fixed | API/Dispatcher/Auth/Job Lifecycle | Queued jobs keep dispatching after actor disable/delete or scope loss |
| AUD-215 | High | Confirmed | API/Frontend/CLI/Terminal/Resource Bounds | Terminal replay loads full session output history before applying replay bounds |
| AUD-216 | High | Confirmed | Gateway/Spool/Replay | Gateway spool replay can strand valid events after per-target queue saturation |
| AUD-217 | High | Fixed | API/Frontend/CLI/Backups | Chunked backup artifact upload defaults exceed the route body limit |
| AUD-218 | High | Fixed | API/Frontend/CLI/File Operations | Chunked file-push jobs exceed the job-create route body limit |
| AUD-219 | Medium/High | Confirmed | API/Worker/Integrations/Delivery State | Disabled integrations can still deliver already queued outbound work |
| AUD-220 | High | Fixed | API/Worker/Integrations/Auth | Queued integration deliveries are not bound to the originating actor authority |
| AUD-221 | Medium/High | Confirmed | API/Frontend/System Dashboard | System dashboard omits agent-lost lifecycle failures |
| AUD-222 | Medium | Confirmed | Frontend/System Config/Security | Suite config editor still presents the private API bind as a public API setting |
| AUD-223 | High | Confirmed | API/Gateway/Client Lifecycle | Lifecycle disconnect can report success while older queued commands still deliver |
| AUD-224 | Medium/High | Fixed | Agent/CLI/Frontend/File Pull | File pull byte caps can be bypassed when a file grows after stat |
| AUD-225 | Medium/High | Fixed | Agent/File Browser/Resource Bounds | Text save hash checks read the whole destination file into memory |
| AUD-226 | High | Fixed | API/Job Outputs/State Machine | Final output insertion is not atomic with target terminalization |
| AUD-227 | Medium/High | Fixed | Agent/Frontend/File Browser/Resource Bounds | Directory listing reads and sorts every entry before applying the page limit |
| AUD-228 | Medium/High | Fixed | Agent/API/Network Speed Tests | Network speed-test server accepts the first TCP peer without verifying the expected tunnel peer |
| AUD-229 | High | Confirmed | API/Frontend/Network Topology | Topology evidence and OSPF recommendations are keyed by mutable tunnel-plan names |
| AUD-230 | Medium/High | Confirmed | Agent/Telemetry/Network Probes | Autonomous latency monitoring captures custom probe output without a byte limit |
| AUD-231 | Medium/High | Fixed | API/CLI/Agent/Network Speed Tests | Network speed tests are treated as confirmation-free read-only jobs despite opening listeners and sending traffic |
| AUD-232 | Medium/High | Fixed | API/Dispatcher/Agent/Network Speed Tests | Network speed tests bypass exclusive dispatch serialization and can overlap on the same tunnel endpoints |
| AUD-233 | Medium/High | Fixed | API/Worker/Agent/Network Speed Tests | Network speed tests can dispatch one endpoint after the peer target is skipped |
| AUD-234 | High | Skipped | API/Worker/Webhooks/Security | Job-created webhooks deliver full job operation payloads to external targets |
| AUD-235 | High | Fixed | API/Frontend/Jobs/Idempotency | Job-create retries can dispatch the same reviewed action under a new job ID |

## Issues

### AUD-001: Former Archive/Download Symlink Dereference Handling

- Severity: Medium/High
- Status: Fixed
- Area: Agent/File Archive And Download
- Context: Operator archive and directory-download jobs walk a selected VPS
  directory and return a tar archive.
- Root Cause: The previous walk used no-follow metadata to classify symlinks,
  but tar creation used `tar::Builder::append_path_with_name`, whose default
  behavior follows symlinks.
- Impact: A symlink inside the selected directory can cause returned archives
  to include the symlink target's file contents, leaking readable files outside
  the selected tree and making preflight manifest/size checks misleading.
- Evidence: `crates/agent/src/file_browser.rs::build_tar_archive`,
  `crates/agent/src/file_browser.rs::append_tar_path_checked`,
  `crates/agent/src/file_download.rs::build_directory_download_artifact`,
  `crates/agent/src/file_download.rs::append_tar_path_checked`,
  `tar 0.4.46 Builder::follow_symlinks`.
- Notes: Practical when an operator archives application directories that
  commonly contain symlinks to shared config, logs, or mounted data. Fixed by
  making archive/download builders default to preserving symlinks and adding an
  explicit `follow_symlinks` opt-in.

### AUD-002: Operator Login And TOTP Failures Have No Server-Side Throttling

- Severity: Medium/High
- Status: Fixed
- Area: API/Auth
- Context: Operator login protects the control plane for fleet operations,
  backups, restores, networking, updates, and admin workflows.
- Root Cause: Login validation delegates directly to repository auth; missing
  user, bad password, missing TOTP, decrypt failure, and invalid TOTP return
  without durable failed-attempt state, cooldown, lockout, or audit evidence.
- Impact: A remote client can repeatedly guess passwords or TOTP codes and
  force repeated password/TOTP verification work without API-side throttling.
- Evidence: `crates/api/src/routes_auth.rs::login_operator`,
  `crates/api/src/repository_auth.rs::login_operator_with_throttle`,
  `migrations/0001_identity_access.sql`.
- Notes: This matters for any deployment where the login endpoint is reachable
  over a network, even if TLS and strong passwords are configured. Fixed by
  adding durable username/IP throttle buckets with strict 8-failures/24-hour
  defaults and a 24-hour auto-unlock window.

### AUD-003: Fleet WebSocket Stream Bypasses Fleet-Read Scope Checks

- Severity: Medium
- Status: Fixed
- Area: API/WebSocket/Auth
- Context: HTTP fleet routes enforce `fleet:read`; the WebSocket stream sends
  the live fleet snapshot and subsequent fleet events.
- Root Cause: The WebSocket handler accepts any valid operator access token and
  calls `fleet_snapshot`/event subscription without checking scopes.
- Impact: Narrow-scoped operators can subscribe to inventory/session state and
  live fleet events across a boundary that HTTP routes enforce.
- Evidence: `crates/api/src/routes_ws.rs`, `crates/api/src/routes.rs`,
  `crates/api/src/state.rs::fleet_snapshot`.
- Notes: Fixed by requiring `fleet:read` during WebSocket token
  authentication before streaming fleet snapshots or events.

### AUD-004: Job-Output Payload Bytes Are Readable With Fleet-Read Scope

- Severity: Medium/High
- Status: Fixed
- Area: API/Job Outputs/Auth
- Context: Durable job outputs contain stdout, stderr, status payloads,
  file-download bytes, and object-backed chunks from privileged operations.
- Root Cause: Payload-bearing job-output list/download/archive routes require
  only `fleet:read`, and the response model exposes inline bytes plus
  object-backed output references.
- Impact: Read-only fleet operators can retrieve command output, scripts,
  logs, file contents, restore details, and other data that is more sensitive
  than fleet inventory metadata.
- Evidence: `crates/api/src/routes_job_history.rs`,
  `crates/api/src/model.rs::JobOutputView`,
  `crates/api/src/repository_job_outputs.rs`.
- Notes: Fixed by moving payload-bearing job-output reads, downloads,
  archives, and comparisons behind `jobs:read`; job and target metadata remain
  available through `fleet:read`.

### AUD-005: Backup/Update Object-Store Separation

- Severity: High
- Status: Fixed
- Area: API/Object Storage
- Context: The API exposes separate configuration for backup artifacts and
  hosted agent-update artifacts.
- Root Cause: Previous startup logic built backup and update stores separately,
  then collapsed them into one shared store before assigning API state.
- Impact: Backup and update storage boundaries were not honored. This could break
  retention, permissions, separation of duties, update artifact availability,
  and operator expectations in long-running deployments.
- Evidence: `crates/api/src/main.rs` object-store initialization,
  `crates/api/src/state.rs::AppState`.
- Notes: Fixed by keeping backup and update stores separate, defaulting both to
  local filesystem storage, and removing API/dashboard-generated externally
  reachable update artifact URLs.

### AUD-006: Artifact Cleanup Metadata/Object Ordering

- Severity: Medium/High
- Status: Fixed
- Area: API/Worker/Artifact Cleanup
- Context: Backup artifacts, file-transfer artifacts, and object-backed job
  outputs are durable operator evidence.
- Root Cause: Previous cleanup paths deleted object-store payloads before
  deleting or tombstoning the corresponding Postgres metadata.
- Impact: A crash or database error after object deletion could leave visible
  metadata that points at missing objects, producing failed downloads and
  misleading retention/cleanup state.
- Evidence: `crates/api/src/routes_history.rs`,
  `crates/api/src/routes_backups.rs`,
  `crates/worker/src/backup_policy_retention.rs`,
  `crates/worker/src/main.rs` artifact cleanup paths.
- Notes: This is a normal operational failure window, not a theoretical storage
  corruption scenario. Fixed by marking object metadata/registry rows out of
  active state before deleting bytes and by adding a retryable `deleting`
  registry state.

### AUD-007: One Deleted Fixed Target Can Fail An Entire Multi-Target Schedule

- Severity: Medium/High
- Status: Fixed
- Area: Worker/Schedules/Client Lifecycle
- Context: Schedules store target snapshots and are expected to keep running as
  VPSs are added, removed, hidden, or offboarded.
- Root Cause: Previous schedule materialization loaded capabilities for all fixed target
  IDs and aborts with `fixed_target_not_found` if any requested client is no
  longer visible.
- Impact: One deleted target could prevent jobs from being created for all
  remaining valid targets. Repeated failures can disable recurring backups,
  updates, or maintenance across healthy VPSs.
- Evidence: `crates/worker/src/main.rs` schedule materialization,
  `crates/api/src/repository_inventory.rs` client deletion/hide behavior.
- Notes: This is practical in long-running 20+ VPS fleets where offboarding is
  normal. Fixed by materializing available fixed targets, recording unavailable
  saved targets as skipped with `fixed_target_unavailable`, and preserving
  missing target IDs in schedule audit metadata without failing the whole run.

### AUD-008: Scheduled Update-Family Jobs Bypass The Busy-Client Skip Policy

- Severity: Medium/High
- Status: Fixed
- Area: Worker/Schedules/Agent Update
- Context: Update-family commands are exclusive lifecycle operations; manual
  API job creation pre-skips clients with other active targets.
- Root Cause: Previous worker schedule materialization applied capability skips and then
  inserts queued targets directly, without running the API busy-update check.
- Impact: Scheduled `agent_update`, `agent_update_activate`,
  `agent_update_rollback`, or `agent_update_check` could collide with active
  backups, restores, file operations, scripts, or another update lifecycle.
- Evidence: `crates/api/src/routes_jobs.rs::busy_update_skip_targets`,
  `crates/worker/src/main.rs` schedule target insertion.
- Notes: The browser-visible `busy_update_skipped` behavior was therefore not
  consistent between manual and scheduled update workflows. Fixed by applying
  the same active-target busy check during schedule materialization and emitting
  the shared `busy_agent_active_jobs` skipped status for busy scheduled update
  targets.

### AUD-009: Former Unsafe File-Copy Overwrite And Destination Symlink Handling

- Severity: Medium/High
- Status: Fixed
- Area: Agent/File Operations
- Context: Operators can copy files/directories on an agent, with overwrite
  allowed for existing destinations.
- Root Cause: The previous copy implementation wrote to the final destination
  path and used normal filesystem copy/permission calls that followed
  destination symlinks when overwrite was allowed.
- Impact: A destination symlink can cause the copy to overwrite a different
  file than the operator selected. Interrupted or failed copies can also leave
  partial final-destination contents.
- Evidence: `crates/agent/src/file_browser.rs::execute_file_copy`,
  `crates/agent/src/file_browser.rs::copy_path`,
  `crates/agent/src/file_browser.rs::copy_file_contents_checked`.
- Notes: Practical on real servers where application trees commonly contain
  symlinks and operators use file-management workflows repeatedly. Fixed by
  rejecting destination symlinks unless explicitly opted into, and by copying
  regular-file overwrites through a same-directory temporary file before atomic
  rename into place.

### AUD-010: Former Chmod Top-Level Symlink Handling

- Severity: Medium
- Status: Fixed
- Area: Agent/File Operations
- Context: Operators can change permissions for selected filesystem paths.
- Root Cause: The previous implementation checked the selected path with
  no-follow metadata, then called `set_permissions` on the path, which follows
  the symlink target on Unix.
- Impact: A chmod request on a symlink could mutate the target file's permissions
  instead of rejecting or modifying only the link metadata.
- Evidence: `crates/agent/src/file_browser.rs::execute_file_chmod`,
  `crates/agent/src/file_browser.rs` `set_permissions` calls.
- Notes: The blast radius is narrower than copy/archive, but it can still break
  service permissions or weaken local file access. Fixed by rejecting symlink
  targets unless the command explicitly opts into following symlinks.

### AUD-011: Supervised Process Log Growth And Tail-Read Safety

- Severity: High
- Status: Fixed
- Area: Agent/Process Supervisor
- Context: The agent process supervisor writes stdout/stderr logs for managed
  daemons and exposes `process_logs` to operators.
- Root Cause: Previous supervised stdout/stderr files appended without rotation
  or size caps, and previous log tailing used an unbounded read before
  returning a bounded tail.
- Impact: Long-running or noisy daemons can fill disk over time. Reading a tail
  of a large log can also consume excessive memory/I/O on small VPSs.
- Evidence: `crates/agent/src/supervisor.rs::execute_process_supervisor_command`,
  `crates/agent/src/supervisor.rs::push_tail_output`,
  `crates/agent/src/supervisor.rs::tail_file`.
- Notes: This directly affects long-running fleet operation. Fixed by adding
  bounded log rotation and keeping log tail reads bounded by the rotated active
  log size.

### AUD-012: Supervisor Process-Record Durability

- Severity: Medium/High
- Status: Fixed
- Area: Agent/Process Supervisor
- Context: The agent persists supervisor records so managed daemons can be
  reconciled after restart.
- Root Cause: Previous supervisor records were written non-atomically, and
  process start could spawn the child before the record was durably persisted.
- Impact: A crash or write failure could leave a daemon running without a valid
  supervisor record, making later status/restart/stop/reconcile behavior wrong.
- Evidence: `crates/agent/src/supervisor.rs` process start and record
  persistence paths, `crates/agent/src/runtime.rs` startup reconcile call.
- Notes: This is practical whenever agents manage long-lived processes on
  constrained VPS disks. Fixed by atomic temp-file/fsync/rename record writes
  and by terminating newly spawned children if their first durable record write
  fails.

### AUD-013: OSPF Cost Updates Mutate Runtime Config Without Updating The Canonical Tunnel Plan

- Severity: Medium/High
- Status: Fixed
- Area: API/Agent/Network OSPF
- Context: Operators can apply recommended OSPF cost changes to tunnel routing
  config from API-generated network recommendations.
- Root Cause: The agent-side `network_ospf_cost_update` mutates managed
  Bird/runtime config, while previous API tunnel-plan execution recording did
  not update the stored tunnel plan's canonical `recommended_ospf_cost`.
- Impact: The VPS routing state could diverge from the API's source-of-truth
  tunnel plan. Later recommendations, UI state, applies, or rollbacks can be
  based on stale cost data.
- Evidence: `crates/agent/src/network_apply.rs`,
  `crates/api/src/job_request.rs::validate_network_ospf_cost_update_operation`,
  `crates/api/src/repository_network.rs`,
  `crates/api/src/repository_network_recommendations.rs`.
- Notes: This affects real topology automation rather than a cosmetic display.
  Fixed by recording completed `network_ospf_cost_update` jobs as canonical
  tunnel-plan cost updates, using stale/idempotent protection, without marking
  either tunnel side as fully applied.

### AUD-014: Direct File Read/Download Byte-Cap Enforcement

- Severity: Medium/High
- Status: Fixed
- Area: Agent/File Read And Download
- Context: Operators can read or download files with configured maximum sizes.
- Root Cause: Previous direct file read/download checks relied on metadata
  captured before reading; bytes actually read or streamed were not consistently
  capped against the same limit if the file grew during the operation.
- Impact: A growing log or application file could exceed intended read/download
  bounds, increasing memory, network, and output-storage pressure.
- Evidence: `crates/agent/src/file_browser.rs::execute_file_read_text`,
  `crates/agent/src/file_download.rs::execute_regular_file_download`,
  `crates/agent/src/file_download.rs::stream_regular_file_download`.
- Notes: Growing logs are common during normal 20+ VPS operation. Fixed by
  enforcing byte caps while reading and streaming, not only before the read.

### AUD-015: Backup Plaintext Byte-Limit Enforcement During File Growth

- Severity: Medium/High
- Status: Fixed
- Area: Agent/Backup
- Context: Agent backup jobs enforce a maximum plaintext archive size before
  encrypting and returning backup artifacts.
- Root Cause: Previous backup scope accounting used file metadata before
  reading, then read and built the plaintext archive before the final
  archive-size check.
- Impact: Files that grew during backup could force the agent to read/build
  oversized plaintext data before rejection, causing avoidable memory and I/O
  pressure on small VPSs.
- Evidence: `crates/agent/src/backup.rs::collect_backup_files`,
  `crates/agent/src/backup.rs::encode_backup_tar_archive`,
  `crates/agent/src/backup.rs::execute_backup`.
- Notes: The final limit prevents accepting the artifact, but too late to
  protect runtime resource usage. Fixed by bounding file reads and tar encoding
  against the configured plaintext limit.

### AUD-016: Terminal Replay Returns PTY Bytes With Fleet-Read Scope

- Severity: Medium/High
- Status: Fixed
- Area: API/Terminal/Auth
- Context: Terminal sessions capture interactive PTY output that may include
  commands, prompts, file contents, secrets, or operational diagnostics.
- Root Cause: Terminal session and replay routes require only `fleet:read`,
  while replay models return the PTY byte stream.
- Impact: A fleet-read operator can retrieve interactive terminal contents,
  which are typically more sensitive than inventory metadata.
- Evidence: `crates/api/src/routes_terminal_sessions.rs`,
  `crates/api/src/model_terminal.rs`.
- Notes: Fixed by requiring `terminal:read` for terminal session records and
  replay hydration, including metadata-only replay requests.

### AUD-017: Webhook Rule And Delivery Reads Expose Outbound Targets And Rendered Payloads

- Severity: High
- Status: Fixed
- Area: API/Worker/Webhooks/Auth
- Context: Webhook rules and deliveries define external endpoints and rendered
  payloads for automation/notification workflows.
- Root Cause: Webhook rule and delivery list/detail routes require only
  `fleet:read`, while mutation/dispatch routes require stronger write scopes.
  Response models expose target URLs, headers/metadata, and rendered payloads.
- Impact: Read-only fleet operators can see outbound integration destinations
  and payload contents that may include secrets, topology details, or operational
  event data.
- Evidence: `crates/api/src/routes_webhook_rules.rs`,
  `crates/api/src/model_webhook_rules.rs`.
- Notes: Fixed by requiring `integrations:read` for webhook rule reads, dry
  runs, and delivery reads.

### AUD-018: Saved Command Templates Expose Full Operation Payloads To Fleet Readers

- Severity: Medium/High
- Status: Fixed
- Area: API/Command Templates/Auth
- Context: Saved command templates can store reusable payloads for privileged
  command execution and fleet maintenance.
- Root Cause: Template read/list routes require only `fleet:read`, and response
  models expose the full command payload.
- Impact: A read-only operator can inspect scripts, file paths, restore/update
  payloads, and operational parameters that should follow command/inventory
  permissions.
- Evidence: `crates/api/src/routes_command_templates.rs`,
  `crates/api/src/model_command_templates.rs`.
- Notes: Fixed by requiring `templates:read` for saved command template reads.

### AUD-019: Schedule Listings Expose Full Recurring Job Operations To Fleet Readers

- Severity: Medium/High
- Status: Fixed
- Area: API/Schedules/Auth
- Context: Schedules store recurring job definitions, target snapshots, and
  operation payloads.
- Root Cause: Schedule list/detail routes require only `fleet:read`, and the
  schedule view returns command payload, target specification, and timing data.
- Impact: A read-only operator can learn recurring backup, restore, script,
  update, and network operation details beyond inventory metadata.
- Evidence: `crates/api/src/routes_schedules.rs`,
  `crates/api/src/model.rs::ScheduleView`.
- Notes: Fixed by requiring `schedules:read` for schedule listings.

### AUD-020: Alert Notification Channels And Deliveries Expose Outbound Targets And Payloads

- Severity: Medium/High
- Status: Fixed
- Area: API/Alerts/Auth
- Context: Alert notification channels and delivery records contain outbound
  target configuration, dedupe keys, and rendered alert payloads.
- Root Cause: Channel and delivery list routes require only `fleet:read`, while
  create/delete/dispatch/process routes require stronger inventory-write
  authority. Response models expose target and payload fields.
- Impact: Read-only fleet users can inspect notification destinations and
  delivered operational payloads.
- Evidence: `crates/api/src/routes_alerts.rs`,
  `crates/api/src/model_alert_notifications.rs`.
- Notes: Fixed by requiring `integrations:read` for alert notification channel
  and delivery reads.

### AUD-021: Data-Source Presets And Rendered Hot-Config Expose Executable Config To Fleet Readers

- Severity: Medium/High
- Status: Fixed
- Area: API/Data Sources/Auth
- Context: Data-source presets and rendered hot-config can include command
  argv, module settings, runtime tool paths, and generated TOML for agents.
- Root Cause: Preset listing, diff/test helpers, assignment listing, and
  rendered hot-config routes require only `fleet:read`; response models expose
  raw definitions, rendered sections, and full TOML.
- Impact: Read-only fleet operators can read executable agent configuration and
  local operational policy that should be controlled by stronger config
  permissions.
- Evidence: `crates/api/src/routes_inventory.rs`,
  `crates/api/src/model_data_sources.rs`.
- Notes: Fixed by requiring `config:read` for preset definitions, assignments,
  diff/test helpers, and rendered hot config. Data-source readiness status
  remains `fleet:read`.

### AUD-022: Hot-Config Rule Templates Expose Raw Generators And Rendered Patches

- Severity: Medium/High
- Status: Fixed
- Area: API/Hot Config/Auth
- Context: Hot-config rule templates generate TOML patches that can change
  agent behavior across data sources, telemetry, and runtime policy.
- Root Cause: Template listing/rendering requires only `fleet:read`, and
  response models return raw generator bodies, rendered TOML, and parsed patch
  values.
- Impact: Read-only operators can inspect config generator logic and render
  arbitrary template values into executable patches.
- Evidence: `crates/api/src/routes_inventory.rs`,
  `crates/api/src/model_data_sources.rs`,
  `crates/api/src/repository_hot_config_rule_templates.rs`.
- Notes: Fixed by requiring `config:read` for hot-config rule template reads
  and renders.

### AUD-023: Tunnel Plan Reads Expose Runtime Commands And Generated Network Config

- Severity: Medium/High
- Status: Fixed
- Area: API/Network/Auth
- Context: Tunnel plans include endpoint inputs, rendered runtime plans,
  generated network snippets, topology addresses, and adapter command hooks.
- Root Cause: Tunnel plan listing requires only `fleet:read`, while
  `TunnelPlanView` returns both original input and full rendered plan objects.
- Impact: Read-only fleet users can inspect privileged network mutation details
  such as generated ifupdown/Bird snippets, touched files, runtime control
  settings, underlay/tunnel addresses, and custom runtime commands.
- Evidence: `crates/api/src/routes_network.rs`,
  `crates/api/src/model.rs::TunnelPlanView`,
  `crates/api/src/repository_network.rs`,
  `crates/common/src/config/models.rs`.
- Notes: Fixed by requiring `network:read` for full tunnel-plan reads.
  Topology and recommendation summaries remain `fleet:read`.

### AUD-024: Hosted Update Artifacts Were Exposed As Public API Downloads

- Severity: Medium/High
- Status: Fixed
- Area: API/Agent Updates/Auth
- Context: Operators can upload signed agent-update artifacts into the private
  API update object store.
- Root Cause: The hosted artifact download route did not require an operator
  token, release reads used `fleet:read`, and docs/scripts described fronting
  `/api/v1/agent-update-artifacts/{sha256}` as a public update URL.
- Impact: A private control-plane API route could be treated as a public
  artifact origin. That violates the operator-only API boundary and risks
  exposing hosted update binaries through the dashboard/API path.
- Evidence: `crates/api/src/routes_update_releases.rs`,
  `tutorials/08-agent-updates.md`, `scripts/smoke-minio-update-artifact.sh`,
  `frontend/src/panels/jobs/AgentUpdateReleasesPanel.tsx`.
- Notes: Fixed by requiring `config:read` for release reads and hosted artifact
  downloads, updating docs/scripts/UI to describe hosted paths as private API
  downloads, and documenting external HTTPS hosts as the only agent-facing
  artifact URLs.

### AUD-025: File-Transfer Artifact Reads Used Fleet Metadata Or Write Scopes Instead Of Jobs-Read

- Severity: Medium/High
- Status: Fixed
- Area: API/File Transfers/Auth
- Context: File-transfer sessions, source artifacts, and handoff artifacts
  contain paths, object references, hashes, sizes, and downloadable bytes.
- Root Cause: Session and handoff reads used `fleet:read`, while source
  artifact list/download used `jobs:write`, mixing sensitive payload reads with
  either metadata access or mutation authority.
- Impact: Fleet readers could inspect transfer paths and handoff payload
  metadata, while read-only job-output operators could not read file-transfer
  source artifacts without write permission.
- Evidence: `crates/api/src/routes_file_transfers.rs`,
  `crates/api/src/model_file_transfer.rs`.
- Notes: Fixed by requiring `jobs:read` for file-transfer session listing,
  source artifact listing/download, and handoff artifact download. Upload and
  handoff creation remain `jobs:write`.

### AUD-026: History Export Default Can Include Payload Domains With Fleet-Read Only

- Severity: Medium/High
- Status: Fixed
- Area: API/History/Auth
- Context: History export is an operator bulk export surface used for audit,
  retention review, troubleshooting, and offline analysis.
- Root Cause: The export route required only `fleet:read`, while the default
  domain set includes `job_outputs` and `backup_artifacts`. Those domains can
  contain inline job-output bytes, output object references, backup artifact
  IDs, hashes, sizes, and backup download metadata.
- Impact: A fleet metadata reader could request the default export, or
  explicitly request a payload-bearing domain, and receive sensitive job or
  backup evidence that belongs behind narrower payload read scopes.
- Evidence: `crates/api/src/routes_history.rs::export_history`,
  `crates/api/src/model_history.rs::HistoryDomain`, and
  `crates/api/src/repository_history.rs` export domain assembly.
- Notes: Fixed by authorizing each requested export domain separately:
  `job_outputs` requires `jobs:read`, `backup_artifacts` requires
  `backups:read`, and non-payload operational domains remain `fleet:read`.

### AUD-027: Backup And Restore Read Surfaces Use Fleet Metadata Or Write Scopes

- Severity: Medium/High
- Status: Fixed
- Area: API/Backups/Auth
- Context: Backup requests, backup policies, restore plans, and encrypted
  backup artifact downloads expose filesystem scope, retention policy,
  restore destination, object identifiers, hashes, sizes, and artifact bytes.
- Root Cause: Backup/restore listing routes were readable with `fleet:read`,
  while backup artifact download required the write-oriented
  `backups:write`/operator role instead of a precise backup read scope.
- Impact: Fleet metadata readers could inspect backup and restore plans beyond
  normal inventory/status data, and read-only backup auditors had to be granted
  mutation authority to download encrypted artifacts.
- Evidence: `crates/api/src/routes_backups.rs`,
  `crates/api/src/routes_restores.rs`,
  `crates/api/src/model_backups.rs`, and `crates/api/src/model_restores.rs`.
- Notes: Fixed by adding `backups:read` and requiring it for backup request
  reads, backup artifact metadata/downloads, backup policy reads, restore plan
  reads, and backup-artifact history exports. Backup/restore mutations remain
  on existing write scopes.

### AUD-028: Job Dispatch Confirmation Used Mutable Operation State After Review

- Severity: Critical
- Status: Fixed
- Area: Frontend/Job Dispatch
- Context: Operators review a job dispatch prompt before sending commands,
  updates, file transfers, restores, and other privileged job operations to
  fixed VPS targets.
- Root Cause: The confirmation prompt froze resolved targets and timeout, but
  the final submit path rebuilt the job operation from current component state.
  Editing command/operation fields while the prompt remained open could change
  what was submitted without a fresh review.
- Impact: A reviewed dispatch could execute a different operation than the one
  shown in the confirmation prompt. For destructive file/update/script actions,
  this can affect the wrong command intent even when target resolution is fixed.
- Evidence: `frontend/src/panels/JobDispatchPanel.tsx`,
  `frontend/tests/dispatch-target-consistency.spec.ts`.
- Notes: Fixed by making the prompt store a full dispatch snapshot, including
  operation, argv, command type, destructive flag, payload hash, privilege
  assertion, and file-transfer parameters. Relevant edits close the prompt and
  force a fresh review.

### AUD-029: Backup And Restore Confirmations Used Mutable Form State After Review

- Severity: Critical
- Status: Fixed
- Area: Frontend/Backups/Restore
- Context: Backup requests, artifact uploads/promotions, restore plans, restore
  runs, rollback, and migration runs are privileged workflows with explicit
  confirmation prompts.
- Root Cause: Most backup/restore confirmation prompts stored only an action
  flag. Confirm handlers then read current form state, current files, current
  restore output, or current migration selection at submit time.
- Impact: Operators could review one backup/restore/migration action, edit a
  field while the prompt stayed open, and submit a different request without a
  second review. This is production-critical for restore destinations, backup
  scope, artifact object keys, rollback targets, and migration execution.
- Evidence: `frontend/src/panels/BackupsPanel.tsx`,
  `frontend/tests/dispatch-target-consistency.spec.ts`.
- Notes: Fixed by creating per-action frozen snapshots at review time. Restore
  archive preparation and rollback output loading now happen before confirmation
  is opened; confirm submits only the stored snapshot. Edits close the prompt.

### AUD-030: Data-Source Apply And Lifecycle Confirmations Used Mutable State After Review

- Severity: High
- Status: Fixed
- Area: Frontend/Data Sources
- Context: Data-source preset assignment, rendered hot-config apply, and preset
  lifecycle updates can affect agent runtime configuration.
- Root Cause: Assignment/apply/update confirmations were partially action
  flags, with final submit paths depending on mutable selector, rendered
  config, timeout, preset definition, and privilege state.
- Impact: A confirmation prompt could become stale relative to the form and
  submit a different config change than the operator reviewed. This undermines
  the purpose of confirmation on runtime config workflows.
- Evidence: `frontend/src/panels/DataSourcePresetPanel.tsx`,
  `frontend/tests/dispatch-target-consistency.spec.ts`.
- Notes: Fixed by freezing assignment, apply, and lifecycle-update snapshots.
  Data-source apply stores the rendered TOML operation, payload hash, privilege
  assertion, client, and timeout; edits close the prompt and require review
  again.

### AUD-031: Network Apply Confirmations Use Mutable Plan, Side, Backend, And Option State After Review

- Severity: Critical
- Status: Fixed
- Area: Frontend/Topology
- Context: Network apply controls can apply, rollback, inspect, probe, and run
  speed tests for tunnel plans, including privileged network mutations.
- Root Cause: Opening the prompt stores only `pendingAction`. The prompt items
  and confirm handler derive plan, endpoint side, backend, timeout, privilege
  mode, probe options, and speed-test options from live component state.
  `submitNetworkChange` rebuilds the operation from that live state.
- Impact: An operator can review one tunnel plan/side/backend/timeout and
  submit a different network operation without another confirmation. This can
  mutate tunnel config on the wrong endpoint or with different backend/options
  than reviewed.
- Evidence: `frontend/src/panels/topology/TopologyApplyControls.tsx:99-116`
  renders confirmation items from live state,
  `frontend/src/panels/topology/TopologyApplyControls.tsx:140-155` stores
  only the action, and
  `frontend/src/panels/topology/TopologyApplyControls.tsx:163-229` rebuilds
  and submits the job from current state.
- Notes: Fixed by creating a frozen network-action snapshot at review time.
  The snapshot stores action, command, target IDs, operation, timeout,
  force-unprivileged mode, payload hash, and privilege assertion. Confirm
  submits only that snapshot. Edits to reviewed inputs close the prompt, and
  privilege assertion expiry closes the prompt automatically.

### AUD-032: OSPF Cost Update Confirmation Uses Mutable Plan, Side, Target, And Cost State After Review

- Severity: Critical
- Status: Fixed
- Area: Frontend/Topology/OSPF
- Context: The OSPF cost update panel submits privileged
  `network_ospf_cost_update` jobs and updates tunnel routing cost state.
- Root Cause: The confirmation is a boolean prompt. It displays selector,
  plan, endpoint, cost delta, timeout, and privilege mode from live state, then
  `submitOspfCostUpdate` rebuilds the operation from the current selected plan,
  side, current cost, recommended cost, timeout, and privilege toggle.
- Impact: A reviewed OSPF cost update can be submitted for a different plan,
  endpoint, target, or cost than shown. This can alter routing behavior on a
  production tunnel without an accurate final review.
- Evidence: `frontend/src/panels/topology/TopologyOspfUpdateControls.tsx:94-112`
  builds confirmation items from live state,
  `frontend/src/panels/topology/TopologyOspfUpdateControls.tsx:128-168`
  rebuilds the submitted job from live state, and
  `frontend/src/panels/topology/TopologyOspfUpdateControls.tsx:279-285`
  confirms without a frozen snapshot.
- Notes: Fixed by creating a frozen OSPF update snapshot at review time. The
  snapshot stores the exact target, operation, cost delta, timeout,
  force-unprivileged mode, payload hash, and privilege assertion. Confirm
  submits only that snapshot. Edits to reviewed inputs close the prompt, and
  privilege assertion expiry closes the prompt automatically.

### AUD-033: Tunnel Adapter Promotion Confirmation Uses Mutable Adapter Contract After Review

- Severity: High
- Status: Fixed
- Area: Frontend/Topology/Adapters
- Context: Adapter promotion converts an observed tunnel plan into an
  externally managed runtime adapter with saved lifecycle command strings and
  optional topology evidence.
- Root Cause: The prompt is controlled by `adapterConfirmationOpen` only. It
  displays the current plan and status command, but confirm calls
  `executeAdapterPromotion`, which reads the current observed plan, adapter
  command fields, traffic settings, name, and topology evidence.
- Impact: Operators can review promotion of one adapter contract and submit a
  different command set or observed plan. This can save incorrect runtime
  control commands for future tunnel operations and drift checks.
- Evidence: `frontend/src/panels/topology/TopologyPromotionPanel.tsx:150-189`
  submits live adapter state, and
  `frontend/src/panels/topology/TopologyPromotionPanel.tsx:225-238` confirms
  with only an open boolean. The editable adapter fields remain live at
  `frontend/src/panels/topology/TopologyPromotionPanel.tsx:439-585`.
- Notes: Fixed by freezing the complete adapter promotion request at review
  time. Confirm submits only that request. Edits to adapter contract fields or
  the selected observed plan close the prompt and require review again.

### AUD-034: Gateway Identity Import And Key Revoke Confirmations Use Mutable Key Lifecycle Fields

- Severity: Critical
- Status: Fixed
- Area: Frontend/Access/Keys
- Context: The Access panel can import or rotate gateway agent identities and
  revoke current VPS keys, hiding revoked VPS records and ending sessions.
- Root Cause: The confirmation state stores only an action kind. The prompt
  displays live client ID, public-key hash, mode, VPS ID, and revoke reason.
  Confirm handlers then read the current form fields.
- Impact: An operator can review import/rotation or key revoke for one client
  and submit another without a fresh review. This can register an unintended
  gateway identity, rotate the wrong key, or revoke/hide the wrong VPS.
- Evidence: `frontend/src/panels/AccessPanel.tsx:295-322` imports from live
  identity fields, `frontend/src/panels/AccessPanel.tsx:341-361` revokes from
  live revoke fields, and `frontend/src/panels/AccessPanel.tsx:1011-1040`
  displays confirmation items from those same mutable fields.
- Notes: Store frozen key lifecycle requests in the pending confirmation,
  including client ID, public key, mode, display name/tags, revoke target, and
  reason. Editing fields while a prompt is open should close it.
- Fix: Access-panel identity import/rotation and key revoke now build frozen
  confirmation snapshots containing the exact request fields and local privilege
  assertion. Editing any reviewed key lifecycle field closes the prompt; confirm
  submits only the frozen snapshot.

### AUD-035: Single-VPS Config Apply Confirmation Uses Mutable TOML Payload After Review

- Severity: Critical
- Status: Fixed
- Area: Frontend/Config
- Context: The single-VPS config editor reads one agent config, lets the
  operator edit redacted TOML, then submits a privileged hot-config job.
- Root Cause: The prompt stores no apply snapshot. It displays live target and
  base hash, then `applyConfig` builds the operation from current
  `redactedToml`, `baseHash`, timeout, privilege material, and target state at
  confirm time. Editing the TOML does not close the prompt.
- Impact: An operator can review one TOML payload and apply a different
  payload without another review. This can change runtime agent configuration
  on a production VPS outside the confirmed intent.
- Evidence: `frontend/src/panels/ConfigPanel.tsx:815-850` builds and submits
  the config job from live state, while
  `frontend/src/panels/ConfigPanel.tsx:910-925` allows TOML edits and confirms
  without a frozen apply snapshot.
- Notes: Fixed by reusing the bulk-config snapshot model. Single-VPS apply now
  freezes client ID, selector, TOML operation, base hash, timeout, payload hash,
  and privilege assertion at review time. Confirm submits only that snapshot.
  Target, TOML, timeout, privilege, or reread changes close the prompt, and
  privilege assertion expiry closes the prompt automatically.

### AUD-036: Webhook Queue Dispatch Confirmation Can Send A Different Event Than Reviewed

- Severity: Medium/High
- Status: Fixed
- Area: Frontend/Webhooks
- Context: Webhook queue dispatch matches webhook rules for an event kind/id
  and creates delivery records that can later call external endpoints.
- Root Cause: The confirmation prompt stores only `queueConfirmation`. The
  reviewed event kind/id are displayed from live inputs, and confirm calls
  `dispatch(false)`, which reads the current event kind/id.
- Impact: Operators can review dispatch for one event and queue webhook
  deliveries for another event. This can notify external systems with the
  wrong operational context and create misleading delivery history.
- Evidence: `frontend/src/panels/FleetWorkspace.tsx:4611-4623` queues from
  live event inputs, `frontend/src/panels/FleetWorkspace.tsx:4957-4971`
  keeps event inputs editable, and
  `frontend/src/panels/FleetWorkspace.tsx:5008-5031` confirms without a
  frozen event snapshot.
- Notes: The process-queued action can remain queue-current-state if labeled
  that way, but event dispatch should freeze event kind/id and limit at review
  time or close on event edits.
  Reconfirmed after commit `07ecbe7`: this is only partially addressed. A
  prompt exists, but it is not a frozen dispatch snapshot; confirm still calls
  `dispatch(false)` and reads mutable `eventKind`/`eventId`.

### AUD-037: Audit History Prune Confirmation Uses Mutable Prune Domain And Mode After Review

- Severity: High
- Status: Fixed
- Area: Frontend/Audit Retention
- Context: History retention prune can delete retained audit/history rows and,
  when not metadata-only, retained object files for the selected domain.
- Root Cause: The confirmation prompt displays the current domain, retention
  days, prune limit, and metadata-only mode, but confirm calls `prune(false)`,
  which reads live domain and metadata-only state. Input changes do not close
  the prompt.
- Impact: Operators can review pruning one domain or metadata-only mode and
  execute pruning for a different domain or object-deleting mode. This risks
  unintended deletion of operational history or retained objects.
- Evidence: `frontend/src/panels/AuditLogPanel.tsx:140-154` prunes from live
  state, `frontend/src/panels/AuditLogPanel.tsx:218-241` keeps prune controls
  editable, and `frontend/src/panels/AuditLogPanel.tsx:258-273` confirms
  without a frozen prune request.
- Notes: Store the prune request shown in the prompt, including domain and
  metadata-only mode. If retention-day/limit are server-policy context rather
  than request fields, the UI should make that explicit and close the prompt
  when policy inputs change.
  Reconfirmed after commit `07ecbe7`: this is only partially addressed. A
  prompt exists, but confirm still calls `prune(false)` and submits live
  retention state rather than a reviewed immutable request.

### AUD-038: Webhook Delivery Cleanup Deletes Using Live Filters Instead Of The Reviewed Preview

- Severity: Medium/High
- Status: Confirmed
- Area: Frontend/Webhook Retention
- Context: Webhook delivery maintenance previews retained delivery rows by
  age/status/rule and then confirms deletion of the matched history rows.
- Root Cause: The preview result is displayed, but confirmation calls
  `rotate(true)`, which builds the deletion request from live rotation days,
  status, and rule controls instead of the previewed filter snapshot.
- Impact: Operators can preview one cleanup set and delete a different set of
  webhook delivery history after changing filters while the confirmation stays
  open. This can remove forensic delivery records outside the reviewed scope.
- Evidence: `frontend/src/panels/FleetWorkspace.tsx:5250-5265` uses live
  rotation filters for both preview and delete,
  `frontend/src/panels/FleetWorkspace.tsx:5314-5348` keeps filters editable,
  and `frontend/src/panels/FleetWorkspace.tsx:5360-5379` confirms deletion
  without binding it to the preview request.
- Notes: Store the preview request and preview hash/equivalent server token if
  available, then submit that exact cleanup request. Filter edits should clear
  the preview and close confirmation.

### AUD-039: Monitoring Automation Bulk Action Submits Privileged Config Patches Without Review

- Severity: High
- Status: Confirmed
- Area: Frontend/Topology/Automation
- Context: The topology automation table can enable or disable latency
  monitoring and automatic OSPF update settings across selected VPSs by writing
  incremental agent config patches.
- Root Cause: The table actions call `applyAutomationBulk` directly. That
  helper builds `data_source_config_patch` operations, privilege assertions,
  and job requests with `confirmed: true` for each selected VPS without a
  preview or confirmation prompt.
- Impact: A bulk table action can immediately alter monitoring and auto-OSPF
  runtime configuration on multiple production VPSs. This bypasses the normal
  reviewed config-apply workflow and gives operators no frozen target/payload
  snapshot before dispatch.
- Evidence: `frontend/src/panels/TopologyPanel.tsx:368-380` wires
  `Enable monitoring`/`Disable monitoring` directly to `applyAutomationBulk`,
  and `frontend/src/panels/TopologyPanel.tsx:986-1028` submits privileged
  `data_source_config_patch` jobs with `destructive: true` and
  `confirmed: true`.
- Notes: Treat this like other bulk config jobs: preview/freeze selected
  targets, generated TOML per target, payload hashes, timeout, and privilege
  assertions before dispatching.

### AUD-040: Update Release Registry Records Artifact Hashes Without A Review Confirmation

- Severity: High
- Status: Confirmed
- Area: Frontend/Agent Updates
- Context: The agent update registry records external artifact URLs and
  SHA-256 hashes. When registered-update policy is enforced, those hashes
  become the API/worker admission source for manual update jobs.
- Root Cause: The frontend `Record release` button validates the mutable form
  and posts `confirmed: true` directly. There is no confirmation prompt showing
  name, version, channel, artifact URL, artifact hash, rollback URL/hash, and
  size before the record is committed. The CLI explicitly requires
  `--confirmed` for the same operation.
- Impact: Operators can commit a wrong or unintended update artifact hash/URL
  without a final review. In deployments that enforce registered updates, that
  can admit the wrong binary for later fleet update jobs or block the intended
  update path.
- Evidence: `frontend/src/panels/jobs/AgentUpdateReleasesPanel.tsx:61-94`
  submits the release record with `confirmed: true`,
  `frontend/src/panels/jobs/AgentUpdateReleasesPanel.tsx:197-280` provides a
  direct `Record release` form, `crates/api/src/routes_update_releases.rs:120-122`
  requires confirmation, and `crates/vpsctl/src/commands_config.rs:289-291`
  requires CLI `--confirmed`.
- Notes: Add a frozen review prompt or an equivalent explicit review step for
  the release metadata that will be stored.

### AUD-041: Inline Fleet Tag Mutations Bypass Preview Confirmation And Schedule Impact Review

- Severity: Medium/High
- Status: Confirmed
- Area: Frontend/Fleet Tags
- Context: Tags drive operator selectors, alert scopes, grouping, schedule
  target updates, and future bulk job targeting. The dedicated bulk tag panel
  previews affected VPSs and schedule impacts before confirmation.
- Root Cause: Fleet instance detail and selection panels call
  `mutateTagsForAgents` directly. That helper builds privilege assertions and
  submits `/api/v1/tags/bulk` with `confirmed: true`, bypassing the preview
  response that includes `schedule_impacts`.
- Impact: Operators can add or remove tags from one or many selected VPSs
  without seeing the affected fixed targets or schedule impact notices. In
  larger fleets this can silently change future targeting and scoped alert
  behavior.
- Evidence: `frontend/src/panels/FleetWorkspace.tsx:463-495` submits
  confirmed bulk tag mutations directly,
  `frontend/src/panels/FleetWorkspace.tsx:1078-1085` uses it from single-VPS
  detail, `frontend/src/panels/FleetWorkspace.tsx:1684-1692` uses it from
  selected rows, and `crates/api/src/repository_inventory.rs:399-423` shows
  the preview path returning schedule impacts when `confirmed` is false.
- Notes: Route inline tag changes through the same review/preview model as
  the dedicated bulk tag panel, or explicitly remove direct inline mutation
  controls in favor of opening the reviewed Tags workflow.

### AUD-042: Alert And Webhook Configuration Saves Bypass Required Operator Review

- Severity: High
- Status: Confirmed
- Area: Frontend/Alerts/Webhooks
- Context: Fleet alert policies, alert notification channels, and webhook
  rules control which events become alerts and which external endpoints receive
  operational notifications.
- Root Cause: The frontend editors and enable/disable table actions submit
  upsert requests with `confirmed: true` directly. The API intentionally
  requires confirmation for these records, but the UI has no frozen review step
  showing the full outbound target, expression, body template, scope, severity,
  cooldown, and enabled state before saving.
- Impact: Operators can create, update, enable, or disable production alerting
  and webhook delivery behavior without a final review. This can suppress
  critical notifications, route alerts to the wrong endpoint, or send rendered
  webhook payloads to unintended external systems.
- Evidence: `frontend/src/panels/FleetWorkspace.tsx:2817-2835` saves alert
  policies with `confirmed: true`,
  `frontend/src/panels/FleetWorkspace.tsx:3637-3654` saves notification
  channels with `confirmed: true`, and
  `frontend/src/panels/FleetWorkspace.tsx:4501-4514` saves webhook rules with
  `confirmed: true`. Enable/disable table actions reuse confirmed upsert
  requests at `frontend/src/panels/FleetWorkspace.tsx:3699-3708` and
  `frontend/src/panels/FleetWorkspace.tsx:4722-4743`. The API requires
  confirmation at `crates/api/src/routes_alerts.rs:547-550`,
  `crates/api/src/routes_alerts.rs:358-364`, and
  `crates/api/src/routes_webhook_rules.rs:156-158`.
- Notes: This is separate from delivery queue dispatch. It affects saved
  integration configuration that future automatic workers will use.

### AUD-043: Fleet Alert Triage Actions Bypass Required Operator Review

- Severity: High
- Status: Confirmed
- Area: Frontend/Alerts
- Context: The fleet alert table lets operators acknowledge, mute, escalate,
  and clear current production alerts, including critical alerts across
  selected rows.
- Root Cause: Table and row actions call `updateAlerts` directly. That helper
  sends each alert state mutation with `confirmed: true`, fixed canned reasons,
  and a hard-coded four-hour mute duration without a confirmation prompt or
  frozen selected-alert snapshot.
- Impact: A bulk selection mistake can immediately mute or clear active
  alerts. In a 20+ VPS fleet this can hide ongoing production incidents or make
  alert history show deliberate operator triage that was never reviewed.
- Evidence: `frontend/src/panels/FleetWorkspace.tsx:5583-5606` submits alert
  state updates directly with `confirmed: true`, and
  `frontend/src/panels/FleetWorkspace.tsx:5633-5665` wires bulk table actions
  for acknowledge, mute, escalate, and clear directly to that helper. The API
  requires explicit confirmation at `crates/api/src/routes_alerts.rs:307-310`.
- Notes: The reviewed snapshot should include alert IDs/titles, action,
  affected count, mute duration if any, and reason. Clearing or muting should
  not be a one-click confirmed mutation.

### AUD-044: Source And Handoff Artifact Persistence Bypasses Operator Review

- Severity: Medium/High
- Status: Confirmed
- Area: Frontend/File Transfers
- Context: File-transfer source artifacts store operator-supplied bytes in the
  API artifact store for later remote uploads. File-transfer handoffs promote
  completed remote downloads into retained server-side artifacts and then
  download them to the operator.
- Root Cause: The frontend source-upload path reads the selected browser file,
  computes its hash, and posts it with `confirmed: true` immediately. The
  handoff path also creates server-side retained artifacts with
  `{ confirmed: true }` from direct row and bulk download actions. The API
  requires confirmation for both operations, but the UI does not show a frozen
  review of file name/path, source or remote VPS, size, SHA-256, session,
  retention implications, and download/save method before persisting bytes.
- Impact: Operators can accidentally retain the wrong local file, sensitive
  local content, or the wrong completed remote download as reusable server-side
  artifacts. These artifacts are visible to jobs-read operators; source
  artifacts can later be selected as upload payloads, and handoff artifacts
  become durable retained payload copies.
- Evidence: `frontend/src/panels/jobs/FileTransferSessionsPanel.tsx:129-149`
  uploads source artifacts directly with `confirmed: true`.
  `frontend/src/panels/jobs/FileTransferSessionsPanel.tsx:62-79` and
  `frontend/src/panels/jobs/FileTransferSessionsPanel.tsx:82-108` create
  handoffs directly from single-row and bulk actions, while
  `frontend/src/hooks/useJobsData.ts:353-360` sends `{ confirmed: true }`.
  The API rejects unconfirmed handoffs at
  `crates/api/src/routes_file_transfers.rs:182-194` and unconfirmed source
  artifact uploads at `crates/api/src/routes_file_transfers.rs:582-588`.
- Notes: This is durable payload persistence rather than immediate remote host
  mutation, so it should still follow the system's reviewed-payload model.

### AUD-045: Command Template Saves Persist Reusable Operation Payloads Without Review

- Severity: Medium/High
- Status: Confirmed
- Area: Frontend/Command Templates
- Context: Command templates are reusable saved job definitions. They can carry
  command bodies, file operations, process supervisor operations, update
  parameters, hot-config payloads, timeout defaults, privilege defaults, and
  destructive/confirmation defaults for later dispatch.
- Root Cause: The dispatch panel saves user-defined templates by building the
  current operation from live composer state and sending `confirmed: true`.
  Only built-in template copy and template deletion paths use a prompt; normal
  template create/update has no review step showing the exact reusable payload
  and defaults that will be stored.
- Impact: Operators can persist a wrong or unintended command template that
  later dispatches production jobs with misleading names/defaults. This is
  especially risky for destructive file, update, process, backup, restore, or
  config operations because future operators may trust the saved template.
- Evidence: `frontend/src/panels/JobDispatchPanel.tsx:870-939` builds and
  saves the template from current composer state with `confirmed: true`.
  `crates/api/src/routes_command_templates.rs:53-54` rejects unconfirmed
  template upserts, proving the API treats the operation as requiring explicit
  confirmation. Template deletion is separately prompted at
  `frontend/src/panels/JobDispatchPanel.tsx:948-956` and
  `frontend/src/panels/JobDispatchPanel.tsx:1218-1221`.
- Notes: This issue is about durable reusable command metadata, not immediate
  job execution. The dispatch path itself already has a target/payload
  confirmation model.

### AUD-046: Operator Access-Management Mutations Bypass Or Auto-Confirm The Confirmation Contract

- Severity: High
- Status: Fixed
- Area: API/CLI/Auth
- Context: Operator management can create accounts, grant roles and scopes,
  disable/delete users, reset passwords, clear TOTP secrets, and revoke bearer
  sessions. These actions directly control who can operate the private control
  plane.
- Root Cause: The API requires `confirmed` for operator update, lifecycle,
  password reset, and TOTP-clear routes, but the CLI and VTY wrappers hard-code
  `confirmed: true` with no `--confirmed` flag. Operator creation has no
  `confirmed` field or API enforcement at all, and operator session revocation
  is a raw `DELETE` route with no confirmation payload. The frontend has
  review prompts for these workflows, so browser, CLI, and API behavior are not
  aligned.
- Impact: A CLI or direct API operation can change access-control state without
  the same explicit review required elsewhere in the system. Practical
  consequences include accidentally creating an admin or broad-scope operator,
  disabling/deleting the wrong operator, resetting the wrong password, clearing
  TOTP for the wrong account, or revoking another operator's active session.
- Evidence: `crates/api/src/auth_model.rs:286-294` defines
  `CreateOperatorRequest` without `confirmed`,
  `crates/api/src/routes_auth.rs:312-332` creates operators without
  `require_confirmed`, `crates/api/src/routes_auth.rs:341-449` requires
  confirmation for later operator mutations, and
  `crates/api/src/routes_auth.rs:492-503` revokes sessions without a confirmed
  request body. `crates/vpsctl/src/commands_auth.rs:96-118` creates operators
  without confirmation, while `crates/vpsctl/src/commands_auth.rs:124-180`
  hard-codes `confirmed: true` for update/status/password mutations and
  `crates/vpsctl/src/commands_auth.rs:232-244` revokes sessions directly.
  `crates/vpsctl/src/cli_access.rs:25-110` exposes these commands without
  `confirmed` fields, and `crates/vpsctl/src/vty_direct.rs:182-280` mirrors
  the same behavior.
- Notes: This should be fixed as a clean contract update: all sensitive
  operator mutations should have one consistent confirmation requirement across
  API, CLI, VTY, frontend, docs, smoke scripts, and tests. Bootstrap remains a
  separate first-operator setup path.
- Resolution: Fixed by requiring explicit confirmation on create/update/status,
  password reset, TOTP clear, and session revoke API requests; by replacing raw
  session DELETE with a confirmed POST revoke request; and by requiring
  `--confirmed` from CLI/VTY operator mutation paths instead of hardcoding it.

### AUD-047: Migration-Link Listings Expose Restore Metadata With Fleet-Read Scope

- Severity: Medium/High
- Status: Confirmed
- Area: API/Backups/Auth
- Context: Migration links connect metadata-only restore plans to later
  migration workflows. They expose backup request IDs, source and target VPSs,
  restore paths, config inclusion, destination roots, notes, and migration
  status.
- Root Cause: `GET /api/v1/migration-links` requires only `fleet:read`, while
  adjacent backup artifact, restore plan, and backup request reads require
  `backups:read`. The migration-link view carries restore/backup payload
  metadata rather than plain fleet inventory metadata.
- Impact: A fleet-only reader can inspect backup/restore migration intent,
  including filesystem paths and destination roots, without backup read
  authority. This leaks operational restore plans and can disclose sensitive
  path structure from production VPSs.
- Evidence: `crates/api/src/routes_migrations.rs:18-24` authorizes
  `list_migration_links` with `fleet:read`. `crates/api/src/model_backups.rs:282-294`
  shows `MigrationLinkView` includes restore plan ID, source backup request ID,
  source/target clients, paths, `include_config`, destination root, and note.
  `crates/api/src/routes_restores.rs:24-32` and
  `crates/api/src/routes_backups.rs:55-76` use `backups:read` for adjacent
  restore and backup read surfaces.
- Notes: This is the same class of payload-metadata boundary as prior backup
  read-scope fixes. Listing migration links should require `backups:read`, and
  CLI/frontend docs/tests should follow that scope.

### AUD-048: History Retention Prune Can Delete Job And Backup Payload History With Inventory-Write Only

- Severity: High
- Status: Confirmed
- Area: API/History/Auth
- Context: History retention policies and prune runs cover multiple domains,
  including `job_outputs` and `backup_artifacts`. For object-backed domains,
  a non-metadata-only prune can delete both database rows and object-store
  payloads.
- Root Cause: `upsert_history_retention_policy` and
  `prune_history_retention` require only `inventory:write` regardless of the
  requested domain. The export path already maps `job_outputs` to `jobs:read`
  and `backup_artifacts` to `backups:read`, but the write/prune path does not
  apply equivalent domain-specific authority.
- Impact: An operator with inventory write authority but without `jobs:read`,
  `jobs:write`, `backups:read`, or `backups:write` can change retention policy
  for job-output or backup-artifact history and can prune those records and
  retained objects. This can remove forensic command output, file-transfer
  payload history, or backup artifact metadata/objects outside the operator's
  intended authority.
- Evidence: `crates/api/src/routes_history.rs:31-43` authorizes retention
  policy upsert with `inventory:write`, and
  `crates/api/src/routes_history.rs:47-132` authorizes prune with
  `inventory:write` before deleting rows and, when not metadata-only, deleting
  object-store keys. `crates/api/src/model_history.rs:102-130` shows prune can
  target any domain. `crates/api/src/routes_history.rs:334-342` already treats
  `job_outputs` as `jobs:read` and `backup_artifacts` as `backups:read` for
  export, proving those domains are not plain fleet inventory data.
- Notes: The clean fix should use domain-aware write authority for retention
  updates and prune execution, with tests proving inventory-only operators
  cannot prune payload-bearing domains.

### AUD-049: Server Artifact Cleanup Can Delete Backup Artifacts With Jobs-Write Only

- Severity: High
- Status: Confirmed
- Area: API/Worker/Artifact Cleanup/Auth
- Context: Server artifact cleanup is an operator-triggered server job that
  filters the shared `server_artifacts` registry by expression and deletes
  matched retained objects. The registry includes job output artifacts, file
  transfer artifacts, and backup artifacts.
- Root Cause: Cleanup preview and creation require only `jobs:write`.
  The cleanup expression can match any active `server_artifacts.domain`,
  including `backup_artifact`. The worker then applies domain-specific deletion
  behavior, including deleting unreferenced `backup_artifacts` rows and object
  keys.
- Impact: An operator with job-write authority but without backup authority can
  queue deletion of backup artifact metadata and retained encrypted backup
  objects by using an expression such as `artifact.domain = "backup_artifact"`.
  This crosses the backup permission boundary and can remove restore material
  outside the operator's intended scope.
- Evidence: `crates/api/src/routes_server_jobs.rs:25-57` authorizes artifact
  cleanup preview/create with `jobs:write` only.
  `crates/api/src/repository_server_jobs.rs:277-314` loads all active
  `server_artifacts` rows without domain restriction, and
  `crates/api/src/repository_server_jobs.rs:334-360` exposes
  `artifact.domain` to the expression evaluator.
  `crates/api/src/repository_backup_artifacts.rs:376-391` registers backup
  artifacts with domain `backup_artifact`.
  `crates/worker/src/main.rs:1232-1254` dispatches cleanup by domain, and
  `crates/worker/src/main.rs:1356-1393` deletes backup artifact rows before
  deleting the object key when the backup artifact is unreferenced.
- Notes: Cleanup authorization should be domain-aware. Job-output and
  file-transfer cleanup can stay under jobs authority, while backup-artifact
  cleanup needs backup authority or a separate admin-only cleanup capability.

### AUD-050: Artifact Cleanup Jobs Re-Evaluate Expressions Instead Of Deleting The Reviewed Artifact Set

- Severity: Critical
- Status: Fixed
- Area: API/Worker/Artifact Cleanup
- Context: Artifact cleanup is a destructive server-side maintenance job. The
  UI and API use a preview hash so operators can review the matched artifact
  count/bytes before queuing deletion.
- Root Cause: The API validates the submitted `preview_hash` only at job
  creation time, then stores the cleanup expression and aggregate counts. It
  does not store the exact artifact IDs/object keys from the reviewed preview.
  The worker later claims the job, reads only `id` and `expression`, loads the
  current `server_artifacts` table, re-evaluates the expression, and deletes up
  to 1000 current matches.
- Impact: Cleanup can delete artifacts that were not shown in the operator's
  reviewed preview. Any matching artifact created between preview/job creation
  and worker execution becomes eligible for deletion. In production this can
  remove fresh job-output, file-transfer, or backup artifacts that happened to
  match a broad expression such as a domain or client filter.
- Evidence: `crates/api/src/repository_server_jobs.rs:93-104` validates the
  preview hash against the current preview during job creation, but
  `crates/api/src/repository_server_jobs.rs:132-171` stores only expression,
  preview hash, matched count, and matched bytes. `crates/worker/src/main.rs:1159-1185`
  claims only `job.id` and `job.expression`. `crates/worker/src/main.rs:1203-1216`
  calls `artifact_cleanup_candidates(pool)` at execution time and filters by
  the expression again, then `take(1000)` deletes that current set rather than
  the previewed artifact identity list.
- Notes: A destructive cleanup confirmation should freeze artifact identities,
  or use a server-side preview token/table that the worker consumes exactly.
  Aggregate count/hash alone is not enough if the worker later ignores the
  matched identity set.
- Fix: Cleanup job creation now persists the exact reviewed artifact IDs and
  identity fields in `server_job_artifact_cleanup_targets`. The worker consumes
  those rows, skips rows whose current artifact identity no longer matches the
  review snapshot or whose artifact row is already gone, and processes the full
  reviewed set instead of re-evaluating the expression at execution time.

### AUD-051: History Retention Object Prune Drops Metadata Before Object Deletion Succeeds

- Severity: Medium/High
- Status: Confirmed
- Area: API/History/Artifact Cleanup
- Context: History retention can prune object-backed domains such as
  `job_outputs` and `backup_artifacts`. When `metadata_only` is false, the
  operator expects both the metadata row and retained object-store payload to
  be removed or a recoverable failure to be visible.
- Root Cause: For each object-backed candidate, the API first deletes or
  mutates the database metadata, then deletes the object key from the object
  store. If object deletion fails, the response reports `partial_error`, but
  the metadata row is already gone and no longer points to the orphaned object.
- Impact: A transient filesystem/S3 delete failure can leave retained payload
  bytes in the object store with no normal database record for future review or
  cleanup. At fleet scale this creates untracked storage growth and makes
  retention results misleading: the row is counted as pruned even though the
  object still exists.
- Evidence: `crates/api/src/routes_history.rs:115-132` calls
  `prune_history_retention_object_candidate` before `store.delete_confirmed`.
  `crates/api/src/repository_history.rs:432-489` deletes memory/Postgres
  metadata for each object candidate, and
  `crates/api/src/repository_history.rs:760-838` deletes job-output or backup
  artifact rows before marking matching server artifacts as deleting.
  `crates/api/src/tests_history.rs:201-223` explicitly covers a delete failure
  case where `status` becomes `partial_error` while `pruned_rows` includes the
  failed object.
- Notes: This is separate from server artifact cleanup ordering. The history
  retention path should either delete objects before committing metadata
  removal, mark rows as deleting and retry them, or preserve enough durable
  metadata to retry failed object deletion.

### AUD-052: Data-Source Preset And Assignment Mutations Use Inventory-Write Instead Of Config-Write

- Severity: High
- Status: Confirmed
- Area: API/Data Sources/Auth
- Context: Data-source presets define executable and config-generation inputs
  for telemetry, process inventory, command execution policy, tunnel adapters,
  backup object-store behavior, restore path mapping, and update artifact
  sources. Assignments select which VPSs inherit those definitions.
- Root Cause: Read routes for presets, assignments, diff/test, and rendered
  hot config correctly require `config:read`, and hot-config rule-template
  mutations require `config:write`. However data-source preset create/clone,
  preset update, and preset assignment mutation routes still require
  `inventory:write`.
- Impact: An operator intended to manage inventory/tags can mutate reusable
  config and executable-source definitions, then assign them to production VPSs,
  without `config:write`. This can alter generated hot-config patches, update
  sources, backup/restore behavior, process inventory commands, or network
  adapter behavior outside the operator's intended authority.
- Evidence: `crates/api/src/routes_inventory.rs:221-237` protects
  data-source reads with `config:read`, and
  `crates/api/src/routes_inventory.rs:248-267` protects hot-config template
  mutations with `config:write`. In contrast,
  `crates/api/src/routes_inventory.rs:284-305` protects data-source preset
  create/clone with `inventory:write`,
  `crates/api/src/routes_inventory.rs:337-358` protects preset update with
  `inventory:write`, and `crates/api/src/routes_inventory.rs:436-457`
  protects preset assignment with `inventory:write`.
  `docs/operator-access-scopes.md:21-25` classifies these definitions and
  rendered config as `config:read`, not plain fleet metadata.
- Notes: This should be a clean scope correction to `config:write`, with
  CLI/frontend/docs/default role expectations updated intentionally.

### AUD-053: Data-Source Preset Create Path Silently Updates Existing Presets Without Review

- Severity: High
- Status: Confirmed
- Area: API/Frontend/Data Sources
- Context: Data-source presets can be assigned to VPSs and used to generate
  agent hot-config patches. Updating an assigned preset can change future
  runtime configuration for production VPSs.
- Root Cause: The API route named create delegates to
  `create_data_source_preset`, but the repository implementation behaves as an
  upsert: if a non-built-in preset with the same domain/name/scope/owner
  already exists, it updates that preset's description and definition. This
  path has no `confirmed` field, no diff/test preview, and no affected-client
  review. The frontend create form submits directly and also has no
  confirmation prompt.
- Impact: An operator can unintentionally overwrite an existing reusable
  preset by reusing a name in the create form or CLI/API call. If that preset
  is assigned, subsequent rendered hot-config patches or update/backup/network
  runtime behavior can change without the reviewed lifecycle update path.
- Evidence: `crates/api/src/model_data_sources.rs:28-36` defines
  `CreateDataSourcePresetRequest` without `confirmed`.
  `crates/api/src/routes_inventory.rs:284-295` accepts create requests and
  writes immediately. `crates/api/src/repository_data_source_presets.rs:99-116`
  updates an existing non-built-in preset in the create path, and the Postgres
  branch at `crates/api/src/repository_data_source_presets.rs:133-154` finds
  an existing preset before running an update.
  `frontend/src/panels/DataSourcePresetPanel.tsx:253-266` submits create
  directly without a review prompt, while the normal lifecycle update path at
  `frontend/src/panels/DataSourcePresetPanel.tsx:438-454` captures a reviewed
  update snapshot.
- Notes: The clean fix should make create fail on duplicate names, or rename
  the route/flow to explicit upsert and require the same diff, affected-client
  count, confirmation, and config-write authority as the lifecycle update path.

### AUD-054: Hot-Config Rule-Template Mutations Lack Confirmation And Audit Records

- Severity: Medium/High
- Status: Confirmed
- Area: API/Config/Hot Config
- Context: Hot-config rule templates generate reusable TOML patches for agent
  runtime configuration, including telemetry, execution policy, network
  adapters, and autonomous updater settings. Operators can render these
  templates and then apply the generated patches to one or many VPSs.
- Root Cause: The API upsert request has no `confirmed` field, and the upsert
  route writes immediately after `config:write` authorization. The repository
  upsert path can update an existing template by ID, including predefined
  templates, and the delete path removes templates by ID. Neither upsert nor
  delete writes an audit-log record that captures the old/new generator body or
  deletion intent. The frontend clone action also calls upsert directly without
  a review step; deletion has a frontend prompt but the API delete route itself
  has no confirmation contract.
- Impact: A config writer can alter or remove reusable config generators
  without an explicit server-enforced confirmation and without durable audit
  evidence of what generator changed. A bad or accidental generator change can
  later produce incorrect hot-config patches for production VPSs, including
  update URL/interval toggles and runtime network or execution settings.
- Evidence: `crates/api/src/model_data_sources.rs:196-206` defines
  `UpsertHotConfigRuleTemplateRequest` without `confirmed`.
  `crates/api/src/routes_inventory.rs:242-257` accepts upserts and calls the
  repository directly. `crates/api/src/repository_hot_config_rule_templates.rs:83-108`
  updates existing memory records, and the Postgres branch at
  `crates/api/src/repository_hot_config_rule_templates.rs:111-150` uses
  `ON CONFLICT (id) DO UPDATE`. Deletion at
  `crates/api/src/routes_inventory.rs:278-292` and
  `crates/api/src/repository_hot_config_rule_templates.rs:197-220` has no
  request body, confirmation, or audit record. The frontend clone path at
  `frontend/src/panels/ConfigPanel.tsx:319-333` calls upsert directly.
- Notes: This is separate from read-scope leakage. The issue is mutation
  accountability for reusable config generators that can affect later
  production hot-config jobs.

### AUD-055: File Save Confirmation Can Mark Unsent Editor Changes As Saved

- Severity: Medium/High
- Status: Confirmed
- Area: Frontend/File Browser
- Context: The single-VPS file browser lets an operator open a remote text
  file, edit it in the browser, review a save confirmation, and dispatch a
  privileged `file_write_text` job.
- Root Cause: `saveEditor` builds a frozen `file_write_text` operation and
  opens a confirmation prompt, but editing the CodeMirror text or file mode
  after the prompt opens does not close the prompt. The confirm handler sends
  the frozen operation, but `executeConfirmedOperation` then calls
  `setEditorSavedContent(editorContent)` using the live editor state instead
  of the content that was actually sent in the confirmed operation.
- Impact: An operator can review save for file content A, edit the editor to
  content B while the prompt remains open, and confirm. The agent receives and
  writes A, but the frontend marks B as saved locally. The operator can leave
  the page or continue working believing the newer content was persisted when
  it was not. For config files, scripts, and service units on production VPSs,
  this is a practical consistency and operator-safety failure.
- Evidence: `frontend/src/panels/jobs/FileBrowserPanel.tsx:285-305` builds the
  save operation from the current editor content and opens confirmation.
  `frontend/src/panels/jobs/FileBrowserPanel.tsx:311-322` executes the frozen
  operation but updates saved editor state from live `editorContent`.
  `frontend/src/panels/jobs/FileBrowserPanel.tsx:710-721` keeps mode and
  editor content controls editable without clearing `pendingConfirmation`.
  `frontend/src/panels/jobs/FileBrowserPanel.tsx:972-988` confirms the frozen
  operation snapshot.
- Notes: The command payload is frozen, so this is not a wrong-payload
  dispatch bug. The production issue is the stale confirmation remaining open
  and the UI marking unsent post-review edits as saved.

### AUD-056: Backup Policy Retention Prune Drops Backup Metadata Before Object Deletion Succeeds

- Severity: Medium/High
- Status: Confirmed
- Area: API/Worker/Backups/Retention
- Context: Backup policy retention pruning removes old backup artifacts
  according to schedule retention rules, either manually through the API or
  automatically through the worker. Operators use backup request/artifact
  history to prove what backups existed and to download retained artifacts.
- Root Cause: Both manual and worker retention prune paths clear backup
  metadata before object deletion succeeds. The metadata prune updates backup
  requests to metadata-only, deletes `backup_artifacts`, and marks matching
  `server_artifacts` as `deleting`; only after that does the caller attempt
  `delete_confirmed` on the object store. If object deletion fails, the normal
  backup artifact record is already gone even though bytes may still exist.
- Impact: A retention run can remove backup artifact visibility and restore
  linkage before confirming the object was deleted. Operators lose the normal
  per-backup evidence and download path for an artifact that may still be
  present in storage. Recovery then depends on lower-level `server_artifacts`
  cleanup state rather than the backup domain model, which is fragile during
  object-store outages and retention failures.
- Evidence: Manual prune in `crates/api/src/routes_backups.rs:203-229` calls
  `prune_backup_policy_candidate_metadata` before
  `store.delete_confirmed`. Worker prune in
  `crates/worker/src/backup_policy_retention.rs:165-183` calls
  `prune_backup_policy_rows` before object deletion. The prune SQL at
  `crates/worker/src/backup_policy_retention.rs:237-275` clears
  `backup_requests.artifact_id`, changes request status to
  `requested_metadata_only`, deletes `backup_artifacts`, and only marks
  `server_artifacts` as `deleting`.
- Notes: This is the backup-policy-retention variant of the object-delete
  ordering problem. A clean fix should preserve normal backup metadata until
  object deletion succeeds, or introduce an explicit retryable backup artifact
  deleting state visible in backup views.

### AUD-057: User Management Can Remove The Last Active Admin

- Severity: High
- Status: Fixed
- Area: API/Auth/User Management
- Context: A default deployment starts with one bootstrap admin. Admin-only
  user/session management is then used to create, update, disable, delete, and
  repair operator accounts.
- Root Cause: Access-management routes require admin role, confirmation, and
  `admin_risk_acknowledged` for admin records, but the repository update paths
  do not enforce the invariant that at least one active admin must remain.
  Updating an admin can demote the last admin to a non-admin role, and lifecycle
  mutation can disable or delete the last admin while revoking that operator's
  sessions. Bootstrap cannot recover because it rejects when any operator row
  exists, including disabled, deleted, or non-admin rows.
- Impact: An operator can lock the deployment out of all admin-only management
  workflows. In the common one-admin deployment, disabling/deleting/demoting
  the sole admin leaves no authenticated principal able to create a replacement
  admin, re-enable users, inspect operator sessions, or manage auth events.
  Recovery then requires direct database surgery instead of normal operator
  tooling.
- Evidence: `crates/api/src/routes_auth.rs:337-365` updates operator role and
  scopes after only confirmation/admin-risk checks. `crates/api/src/routes_auth.rs:394-415`
  changes admin lifecycle status through the same acknowledgement gate.
  `crates/api/src/repository_auth.rs:1167-1202` updates role/scopes without
  checking remaining active admins. `crates/api/src/repository_auth.rs:1266-1316`
  disables or deletes the row and revokes sessions without a last-admin guard.
  Bootstrap rejects when `operator_count() > 0`, and Postgres counts all
  `operators` rows in `crates/api/src/repository_auth.rs:60-64` and
  `crates/api/src/repository_auth.rs:107-112`.
- Notes: Frontend prompts are not sufficient for this invariant because CLI/API
  callers can perform the same confirmed mutations. The clean fix should make
  last-active-admin preservation a server-side transactional constraint for
  role changes and lifecycle changes.
- Resolution: Fixed by enforcing the one-active-admin invariant in repository
  role/status mutations before state changes. The Postgres path locks the
  operator table and target row in the same transaction; the memory path checks
  under the write lock. Route error mapping returns
  `last_active_admin_required`.
- Verification: `admin_user_routes_preserve_one_active_admin`.

### AUD-058: Integration Mutations Use Inventory-Write Instead Of An Integrations Write Boundary

- Severity: Medium/High
- Status: Confirmed
- Area: API/Integrations/Auth
- Context: Webhook rules, webhook delivery processing, alert notification
  channels, alert notification delivery processing, and alert policies are
  durable integration/control-plane configuration. They can define outbound
  HTTP targets, rendered payload delivery behavior, alert routing, retry
  processing, and fleet notification behavior.
- Root Cause: Read paths were correctly separated to `integrations:read`, but
  write paths still require `inventory:write`. There is no matching
  `integrations:write` boundary in the route checks, so an operator allowed to
  mutate inventory/tag/fleet metadata can also create, delete, dispatch,
  process, or rotate integration records.
- Impact: A scoped operator intended to manage fleet inventory can change
  outbound webhook endpoints and alert notification routing. That can leak
  future operational events to unintended endpoints, suppress or redirect
  notifications, or manually replay/process integration deliveries outside the
  intended integration-administration boundary.
- Evidence: Webhook rule upsert/delete/dispatch/rotation/process require
  `inventory:write` in `crates/api/src/routes_webhook_rules.rs:43-145`.
  Alert state, alert policy, notification channel, notification dispatch, and
  notification processing mutations require `inventory:write` in
  `crates/api/src/routes_alerts.rs:90-260`. The same files use
  `SCOPE_INTEGRATIONS_READ` for integration reads, demonstrating that these
  records are not plain inventory metadata.
- Notes: This is distinct from frontend confirmation gaps. The server-side
  authorization model should expose and enforce an integration write scope for
  integration configuration and delivery-control mutations.
  Reconfirmed after commit `07ecbe7`: this is not fixed. Integration reads use
  `integrations:read`, but webhook and alert integration mutations still
  require `inventory:write`.

### AUD-059: Command-Template Mutations Use Jobs-Write Instead Of A Templates Write Boundary

- Severity: Medium/High
- Status: Confirmed
- Area: API/Command Templates/Auth
- Context: Command templates are shared saved operation payloads shown to
  operators during dispatch. They can encode privileged job parameters,
  destructive file/process/update operations, target-specific defaults, and
  reusable workflows that other operators may later select.
- Root Cause: Command-template reads require `templates:read`, but user-defined
  template create/update/delete routes require only `jobs:write`. There is no
  separate templates write boundary, so the ability to dispatch jobs also
  grants authority to alter the shared template catalog.
- Impact: A jobs-only operator can persist or remove reusable operation
  templates that shape future dispatch choices for other operators. This can
  introduce misleading defaults, destructive payloads, or stale unsafe
  templates into normal UI/CLI workflows without explicit template
  administration permission.
- Evidence: `crates/api/src/routes_command_templates.rs:26-33` requires
  `SCOPE_TEMPLATES_READ` for template listing, while
  `crates/api/src/routes_command_templates.rs:43-86` uses `jobs:write` for
  upsert and delete. `docs/operator-access-scopes.md:24` classifies templates
  as their own read domain.
- Notes: This is not the same as ordinary job dispatch. A dispatched job is an
  immediate action by the caller, while a template mutation persists shared
  operational defaults for later use by other humans and automation.

### AUD-060: Update-Release Registry Mutations Use Jobs-Write Instead Of Config-Write

- Severity: Medium/High
- Status: Confirmed
- Area: API/Agent Updates/Auth
- Context: The agent update release registry stores shared external release
  metadata: release name, version, channel, artifact SHA-256, artifact URL hash,
  rollback artifact metadata, size, and notes. Operators and tooling use it to
  discover the current registered update artifacts before dispatching update
  checks or manual updates.
- Root Cause: Release listing and latest-release lookup require `config:read`,
  but recording a release requires `jobs:write`. The write path therefore treats
  shared update-source metadata as ordinary job dispatch authority instead of
  config/update-registry administration.
- Impact: A jobs-only operator can persist misleading or unsafe release metadata
  into the shared update catalog. Other operators may later select or trust that
  registered release during normal update workflows, causing wrong update
  artifacts, wrong rollback metadata, or polluted fleet update history. This is
  a durable shared state change, not just an immediate job submitted by the
  caller.
- Evidence: `crates/api/src/routes_update_releases.rs:23-52` requires
  `SCOPE_CONFIG_READ` for list/latest update release reads, while
  `crates/api/src/routes_update_releases.rs:74-85` requires `jobs:write` for
  `create_agent_update_release`. `docs/operator-access-scopes.md:27-29`
  classifies private agent-update release metadata under `config:read`.
  Frontend and CLI submit persistent release records through
  `frontend/src/panels/jobs/AgentUpdateReleasesPanel.tsx:61-94` and
  `crates/vpsctl/src/commands_config.rs:284-330`.
- Notes: Immediate update jobs can remain `jobs:write`; recording reusable
  release metadata should require `config:write` or a dedicated update registry
  write authority.

### AUD-061: User-Management Confirmations Remain Armed After Editor Or Selection Changes

- Severity: High
- Status: Fixed
- Area: Frontend/System Users
- Context: System > Users manages operator creation, role/scope changes,
  session TTL changes, password resets, TOTP clearing, disabling, and deletion.
  Confirmations are intended to be a reviewed snapshot for privileged account
  mutations, especially admin-targeted actions.
- Root Cause: The Users panel stores a frozen `pendingAction`, but ordinary
  editor inputs, scope shortcut buttons, row open/edit actions, and table bulk
  actions do not clear an already open confirmation. The confirmation prompt is
  an inline section, not a blocking modal, so operators can continue changing
  the visible editor/table context while the old confirmation remains armed.
- Impact: An operator can review an action for one user or one set of role/scope
  values, then change the selected user or visible editor fields and still click
  the stale confirm button. The backend receives the old frozen action, while
  the screen can be showing a different user or different values. For account
  management this can disable/delete/reset/clear the wrong account from the
  operator's current visible context, or apply old role/scope/TTL values after
  further edits.
- Evidence: `frontend/src/panels/SystemPanel.tsx:438-460`,
  `frontend/src/panels/SystemPanel.tsx:467-498`, and
  `frontend/src/panels/SystemPanel.tsx:505-521` create `pendingAction`
  snapshots. Inputs and shortcuts at
  `frontend/src/panels/SystemPanel.tsx:686-762`, row edit/open handlers at
  `frontend/src/panels/SystemPanel.tsx:591-602` and
  `frontend/src/panels/SystemPanel.tsx:651`, and action buttons at
  `frontend/src/panels/SystemPanel.tsx:775-845` do not close an existing
  confirmation. The inline `ConfirmationPrompt` is rendered at
  `frontend/src/panels/SystemPanel.tsx:851-861` and its component/CSS show no
  blocking overlay in `frontend/src/components/ConfirmationPrompt.tsx:32-84`
  and `frontend/src/styles/shell.css:803-919`.
- Notes: The clean fix is to clear `pendingAction` whenever a user-management
  input, scope shortcut, selected row, opened row, or new user action changes
  the reviewed intent, forcing operators to review and confirm the current
  snapshot again.
- Resolution: Fixed by closing pending user and session confirmations whenever
  the selected operator, operator/session lists, or review-relevant editor
  fields change, and by submitting only the frozen review snapshot.

### AUD-062: Artifact Creation Can Commit Metadata Or Bytes Without Cleanup-Registry Consistency

- Severity: High
- Status: Confirmed
- Area: API/Object Storage/Artifacts
- Context: Backup artifact upload, chunked backup upload, retained backup
  promotion, file-transfer source upload, and file-transfer handoff all create
  durable object-store bytes that operators expect to be visible, downloadable,
  and reclaimable through the shared `server_artifacts` cleanup registry.
- Root Cause: Domain metadata and `server_artifacts` registration are not
  committed atomically. Backup metadata is inserted and the backup request is
  linked in one Postgres transaction, then `register_server_artifact` is called
  afterward. File-transfer source metadata is also inserted before the cleanup
  registry row is registered. Callers receive one generic repository error and
  cannot tell whether the domain metadata already committed. Some callers then
  delete object bytes after a post-commit registry failure, while others leave
  bytes or metadata outside the cleanup registry.
- Impact: A transient database failure between domain-metadata commit and
  cleanup-registry registration can create inconsistent durable artifact state.
  Direct backup upload, backup handoff, and automatic backup artifact recording
  can delete object bytes after backup metadata is already linked, leaving
  visible backup artifact rows that point to missing payloads. Chunked backup
  upload and file-transfer source/handoff paths can leave retained payload
  bytes or visible artifact metadata without a `server_artifacts` row, so normal
  artifact cleanup cannot find those objects. On filesystem-default stores this
  can produce broken restores/downloads, untracked disk growth, and misleading
  operator evidence during ordinary backup/file-transfer use.
- Evidence: `crates/api/src/repository_backup_artifacts.rs:284-351` commits
  backup artifact metadata before calling `register_server_artifact`.
  `crates/api/src/routes_backups.rs:427-449`,
  `crates/api/src/routes_backups.rs:689-720`, and
  `crates/api/src/backup_auto_artifacts.rs:109-128` delete the object on any
  repository error, including a post-commit registry failure.
  `crates/api/src/routes_backups.rs:539-588` performs chunked commit object
  creation but does not clean up on metadata/registry error.
  `crates/api/src/repository_file_transfer_sources.rs:115-162` commits source
  artifact metadata before registering the shared artifact row, and
  `crates/api/src/routes_file_transfers.rs:122-145` returns the repository
  error without cleaning or distinguishing post-commit failure.
  `crates/api/src/routes_file_transfers.rs:218-250` writes a handoff object and
  then registers only the shared artifact row, leaving no cleanup-visible record
  if that registration fails after the object write.
- Notes: This is distinct from retention/delete ordering issues. It happens at
  artifact creation time, before the shared cleanup registry is guaranteed to
  contain the object.

### AUD-063: Schedule Confirmations Remain Armed After Form, Defer, Or Table Context Changes

- Severity: High
- Status: Confirmed
- Area: Frontend/Schedules
- Context: The Schedules page creates and updates recurring jobs, applies a
  schedule immediately, updates the saved fixed target snapshot, defers
  execution, enables/disables schedules, and deletes schedules. These actions
  can repeatedly dispatch privileged jobs across many VPSs.
- Root Cause: Schedule confirmations store frozen state, but inputs and table
  actions that change the visible operator context do not close the open
  confirmation. The create/update confirmation keeps `pendingScheduleSnapshot`
  while name, template, command argv, cron, enabled state, catch-up settings,
  retry settings, max failures, and audit selector remain editable. Separate
  schedule row actions keep `scheduleAction` while the operator can open/edit
  another schedule, start or edit a defer draft, refresh, or choose another
  table action.
- Impact: An operator can review a recurring schedule or a schedule row action,
  then change the visible schedule form or table context and still submit the
  old frozen action. That can save an old cron/selector/operation snapshot
  after visible edits, apply a job from a schedule that is no longer the
  operator's current focus, replace fixed targets from an old review, or delete
  the previously reviewed schedule after moving on to another one. Because
  schedules recur and can target 20+ VPSs, this can repeatedly dispatch or
  suppress work contrary to the current screen state.
- Evidence: `frontend/src/panels/SchedulesPanel.tsx:382-456` creates and later
  saves `pendingScheduleSnapshot`. Form inputs at
  `frontend/src/panels/SchedulesPanel.tsx:1023-1178` update live form state
  without clearing `confirmationOpen` or `pendingScheduleSnapshot`.
  `frontend/src/panels/SchedulesPanel.tsx:526-529` stores `scheduleAction`, but
  row edit/defer/table actions at `frontend/src/panels/SchedulesPanel.tsx:487-538`
  and `frontend/src/panels/SchedulesPanel.tsx:762-854` do not close an already
  open action confirmation. Defer inputs at
  `frontend/src/panels/SchedulesPanel.tsx:929-981` mutate `deferDraft` without
  clearing an existing `scheduleAction`. The confirmation prompts at
  `frontend/src/panels/SchedulesPanel.tsx:983-1011` and
  `frontend/src/panels/SchedulesPanel.tsx:1211-1239` remain open until
  explicit cancel or confirm.
- Notes: The fix should follow the current confirmation rule: editing any input
  or changing any selected schedule/action context that affects an open
  confirmation must close the prompt and require a fresh review.

### AUD-064: Release-Registry Manual Update Shortcut Cannot Provide The Artifact URL It Requires

- Severity: Medium/High
- Status: Confirmed
- Area: Frontend/Agent Updates
- Context: Operators use Jobs > Updates to record external agent release
  metadata and then dispatch manual update jobs or update checks across selected
  VPSs. The API correctly treats the artifact URL as an external operator-hosted
  URL, not as a public API download.
- Root Cause: The release registry intentionally returns only artifact SHA-256
  values and hashes of artifact URLs, but the dashboard still exposes a
  "Manual update" shortcut from that registry. That shortcut passes only the
  latest artifact SHA into the job dispatch preset. The dispatch composer then
  clears the artifact URL field for `agent_update`, while `agent_update` job
  construction requires a concrete `https://` artifact URL.
- Impact: The visible manual update workflow is not practically executable from
  the recorded release row. An operator can click Manual update from the latest
  registered release, land in Dispatch with the SHA prefilled but the required
  URL blank, and then fail validation unless they manually reconstruct the exact
  external artifact URL from some other source. This is production-impacting for
  fleet update operations because the UI presents the registry as a dispatch
  entry point while withholding the URL needed to perform the dispatch.
- Evidence: `frontend/src/panels/jobs/AgentUpdateReleasesPanel.tsx:144-155`
  opens a preset with `mode: "agent_update"` and
  `updateSha256Hex: latestRelease?.artifact_sha256_hex`, but no
  `updateArtifactUrl`. `frontend/src/panels/JobDispatchPanel.tsx:333-335` applies
  that preset by setting `updateArtifactUrl` to an empty string when the preset
  lacks a URL. `frontend/src/panels/jobDispatchModel.ts:157-165` rejects manual
  update operation creation unless `updateArtifactUrl` starts with `https://`.
  The registry row type in `frontend/src/types.ts:1372-1375` exposes only
  `artifact_sha256_hex` and `artifact_url_sha256_hex`, not the raw URL.
- Notes: This is distinct from the private-API artifact fix. API-hosted artifacts
  should stay private; the frontend either needs to dispatch via manifest/check
  using the official GitHub version URL or retain an operator-safe external URL
  source for the shortcut without turning the API into public artifact hosting.

### AUD-065: Delivery Queue Confirmations Are Not Bound To Previewed Rows

- Severity: High
- Status: Confirmed
- Area: Frontend/Integrations
- Context: Alert notification and webhook delivery screens let operators
  preview queued work, then confirm queueing or delivery. Delivery processing
  can contact configured external endpoints and records durable delivery
  attempts.
- Root Cause: The confirmation prompts store only the requested action, not a
  frozen preview row set, delivery IDs, event input snapshot, or preview hash.
  Confirm handlers call the same dispatch/process functions again with
  `confirmed: true`, and the backend re-lists the current matching alerts,
  channels, webhook rules, or queued deliveries at execution time.
- Impact: Operators can preview one set of notification/webhook delivery rows
  and later confirm a different live set. New queued rows, changed rules,
  changed alert state, or changed event inputs can cause delivery attempts to
  external targets that were not in the reviewed result. This is production
  relevant because delivery attempts are side effects outside the system and
  the audit trail then shows intentional operator delivery for rows that were
  not actually reviewed.
- Evidence: Alert notification dispatch/process previews and confirms through
  live requests at `frontend/src/panels/FleetWorkspace.tsx:3717-3787`, while
  `crates/api/src/fleet_alert_notifications.rs:33-69` rebuilds candidates from
  current alerts/channels and
  `crates/api/src/fleet_alert_notifications.rs:71-131` re-lists current queued
  deliveries before sending. Webhook delivery processing has the same preview
  and confirm shape at `frontend/src/panels/FleetWorkspace.tsx:4640-4678`, and
  `crates/api/src/webhook_rules.rs:140-196` re-lists current queued/failed
  webhook deliveries before sending HTTP. Webhook event dispatch also reads
  live event inputs at `frontend/src/panels/FleetWorkspace.tsx:4611-4623`; the
  event-input half is already captured separately in AUD-036.
- Notes: The fix should freeze delivery IDs or a server-issued preview token
  and require confirmed processing to operate on that reviewed set. If the queue
  changes before confirmation, close the prompt or force a fresh preview.

### AUD-066: API Binary And Suite Config Default To All-Interface Binding

- Severity: High
- Status: Fixed
- Area: API/Deploy/Security
- Context: The API is a private operator/control-plane service. Operators may
  run the released server binaries directly, adapt the compose deployment, or
  place services into another supervisor/orchestrator while reusing the shipped
  suite config.
- Root Cause: The API CLI default bind is `0.0.0.0:8080`, and the shipped
  `deploy/config/vpsman.toml` also sets `[api].bind = "0.0.0.0:8080"`. Compose
  currently masks this by publishing the container port only on
  `127.0.0.1:8080`, but the service inside the container and the standalone
  binary default remain all-interface.
- Impact: A direct binary run, host-network container, Kubernetes/service
  deployment, or small compose edit can expose the operator API to a reachable
  network by default. That contradicts the project security model that API and
  gateway control are private-only services and increases the blast radius of
  any credential, scope, or operator-session weakness.
- Evidence: `crates/api/src/main.rs:211` declares
  `VPSMAN_API_BIND` with default `0.0.0.0:8080`. `deploy/config/vpsman.toml:4`
  sets `bind = "0.0.0.0:8080"`. `crates/api/src/main.rs:307-312` applies the
  suite config bind when `VPSMAN_API_BIND` is absent, and
  `crates/api/src/main.rs:633-637` binds the listener to the resulting address.
  `README.md:123-126` says compose keeps API and gateway host ports on
  `127.0.0.1`, but that relies on compose port publishing rather than the API
  service's own default.
- Notes: This is distinct from public artifact URL handling. The API should
  default to loopback/private binding at the process/config level; any
  all-interface API bind should require a deliberate, visible operator override.
- Fix: The API CLI default and shipped suite config now bind to
  `127.0.0.1:8080`. Compose sets `VPSMAN_API_BIND=0.0.0.0:8080` only inside the
  Docker service so Nginx can reach `api:8080`, and the API service no longer
  publishes a host port.

### AUD-067: Public Frontend Proxy Exposes Private API And WebSocket Routes

- Severity: High
- Status: Fixed
- Area: Deploy/Nginx/API Boundary
- Context: The default compose deployment publishes the frontend/Nginx service
  on all host interfaces. Current project policy says the API and gateway
  control plane are private operator services and must not be exposed publicly;
  public URLs, such as update artifact URLs, are supplied separately.
- Root Cause: The shipped Nginx config proxies `/api/`, `/ws`, and `/health`
  from the public frontend listener directly to the API service. Compose binds
  the frontend to `0.0.0.0:5173:80`, so the API HTTP and WebSocket surfaces are
  reachable wherever the frontend is reachable, even though the API container
  port itself is only host-published on loopback.
- Impact: Operators following the default compose shape can unintentionally
  make the private API authentication, WebSocket event stream, and every
  operator API route Internet-reachable behind the same public Nginx endpoint
  as the dashboard. This contradicts the private-control-plane model and makes
  credential, session, scope, CSRF-like browser-origin, and brute-force issues
  production-exposed instead of local/VPN-only. It also conflicts with the
  requirement that public URLs are provided separately and never by the API or
  dashboard.
- Evidence: `deploy/compose.yml:64-69` publishes the frontend service as
  `"0.0.0.0:5173:80"`. `deploy/nginx.conf:8-14` proxies `/api/` to
  `http://api:8080`, `deploy/nginx.conf:16-22` proxies `/health`, and
  `deploy/nginx.conf:24-32` proxies `/ws`. `README.md:123-126` states that API
  and gateway host ports stay localhost-bound by default, but the public Nginx
  proxy still forwards those API paths.
- Notes: This is separate from AUD-066. AUD-066 covers the API process/config
  bind default; this issue covers the public frontend reverse proxy making the
  API reachable even when the API service itself is not directly host-published.
- Fix: The compose frontend binding now defaults to
  `${VPSMAN_FRONTEND_BIND:-127.0.0.1:5173}:80`. Operators must explicitly set a
  wider dashboard bind or provide their own private access path; the API remains
  Docker-internal and unpublished on the host.

### AUD-068: Schedule Mutations Lack An Explicit Backend Confirmation Contract

- Severity: High
- Status: Fixed
- Area: API/CLI/Schedules
- Context: Schedules persist recurring job operations against fixed target
  snapshots or selector expressions. A single schedule can repeatedly run
  privileged or destructive jobs across 20+ VPSs, so schedule create/update,
  retarget, enable/disable, defer, apply-now, and delete are operator-impacting
  control-plane mutations.
- Root Cause: Schedule mutation request models carry privilege assertions but
  do not carry or enforce a `confirmed` boolean. The API routes perform privilege
  verification and then immediately mutate durable schedule state or dispatch
  apply-now jobs. The CLI schedule commands likewise have no `--confirmed`
  option for these mutations, unlike job dispatch, backups, file transfer,
  network apply, history prune, and other mutating workflows.
- Impact: A direct API client or CLI command can create or change recurring
  fleet work without an explicit reviewed-confirmed contract at the backend
  boundary. Frontend prompts cannot compensate for this because the API accepts
  the mutation without knowing whether the exact submitted schedule definition
  was reviewed. This weakens auditability and makes automation or scripting
  mistakes more dangerous: an operator can persist a recurring job, retarget an
  existing schedule, or apply it immediately with only a privilege assertion and
  no separate confirmation bit tied to the reviewed payload.
- Evidence: `crates/api/src/model.rs:585-606` defines
  `CreateScheduleRequest` without `confirmed`,
  `crates/api/src/model.rs:610-631` defines `UpdateScheduleRequest` without
  `confirmed`, `crates/api/src/model.rs:635-648` defines defer/privilege
  mutation requests without `confirmed`, and
  `crates/api/src/routes_schedules.rs:41-64`, `67-93`, `96-144`, `147-196`,
  `199-240`, and `242-300` mutate schedules or dispatch apply-now without a
  confirmation guard. The CLI constructs these requests without any
  confirmation flag at `crates/vpsctl/src/commands_schedules.rs:66-135` and
  `180-335`, and the command dispatcher passes no confirmed option at
  `crates/vpsctl/src/commands_dispatch_access.rs:523-590`.
- Notes: This is distinct from AUD-063. AUD-063 covers frontend schedule
  confirmations staying open after mutable UI context changes; this issue is
  the missing API/CLI confirmation contract itself.
- Resolution: Fixed by adding `confirmed` to schedule mutation request models
  and requiring it for create, update, target update, enable, disable, defer,
  apply-now, and delete before schedule state changes. CLI and VTY schedule
  mutations now require/pass `--confirmed`; frontend schedule requests include
  `confirmed: true`.
- Verification: `schedule_mutations_require_explicit_confirmation`, frontend
  schedule request assertions, stale script/docs search.

### AUD-069: Chunked Backup Artifact Commit Ignores The Confirmation Flag

- Severity: Medium/High
- Status: Fixed
- Area: API/Backups
- Context: Operators can upload encrypted backup artifacts into object storage
  and attach them to backup request records. The chunked upload path exists for
  large artifacts and ends with a commit step that records durable metadata and
  publishes the artifact as available for restore.
- Root Cause: The chunked upload commit request model includes `confirmed`, and
  frontend/CLI clients send that flag, but the API commit route never checks it.
  Instead, after object storage write succeeds, it constructs
  `RecordBackupArtifactMetadataRequest { confirmed: true, ... }` internally.
  This bypasses the same confirmation gate used by direct artifact metadata and
  single-request artifact upload paths.
- Impact: A direct API client with `backups:write` can commit a staged backup
  artifact and attach it to a backup request without the backend enforcing a
  final reviewed confirmation. That is production-relevant because the commit
  changes durable restore inventory and can make a large object available for
  later destructive restore workflows. Frontend and CLI currently behave more
  carefully, but the API boundary itself accepts an unconfirmed commit.
- Evidence: `crates/api/src/model_backups.rs:191-195` defines
  `BackupArtifactUploadCommitRequest.confirmed`. The frontend sends it at
  `frontend/src/hooks/useBackupsData.ts:182-186`, and the CLI requires and sends
  `confirmed` for chunked artifact upload at
  `crates/vpsctl/src/commands_backups.rs:364-399`. The API commit route at
  `crates/api/src/routes_backups.rs:502-564` accepts the request but does not
  test `request.confirmed`; it hardcodes `confirmed: true` in the metadata
  request before calling `record_backup_artifact_metadata`.
- Notes: This is separate from artifact cleanup/order issues. The minimal fix is
  to reject unconfirmed commit requests before object-store publication or
  before metadata recording, matching the direct upload/metadata confirmation
  behavior.
- Resolution: Fixed by rejecting unconfirmed chunked backup artifact commit
  requests before object-store/session commit. Frontend and CLI chunked upload
  paths continue to require and submit `confirmed`.
- Verification:
  `backup_artifact_upload_session_stages_chunks_and_commits_artifact`,
  chunked backup smoke-script confirmed-path search.

### AUD-070: Tunnel-Plan Save And Lifecycle Mutations Lack A Backend Confirmation Contract

- Severity: High
- Status: Fixed
- Area: API/Frontend/CLI/Network
- Context: Tunnel plans are the canonical network topology source used for
  later apply, rollback, OSPF-cost, monitoring, adapter, and topology graph
  workflows. Saving or toggling a plan can affect future production network
  operations across both tunnel endpoints.
- Root Cause: Several durable network-plan mutations have no `confirmed`
  field or backend confirmation check. `CreateTunnelPlanRequest` and
  `PromoteTelemetryTunnelRequest` contain only topology payload fields, while
  enable/disable uses a path-only POST. The repository save path also behaves
  as an upsert by plan name: a "create" request updates an existing non-deleted
  plan with the same name, resets statuses to `planned`, and clears last apply
  and rollback job IDs.
- Impact: A direct API or CLI caller can persist or overwrite canonical tunnel
  topology, promote observed telemetry into a saved plan, or enable/disable a
  plan without a server-enforced reviewed confirmation. In the dashboard,
  `Save plan`, `Enable plan`, `Disable plan`, and observed import submit
  immediately. In production, a mistaken save or name collision can replace the
  source-of-truth tunnel payload that later jobs apply to VPSs, and a mistaken
  enable/disable can alter which plans are eligible for normal apply workflows.
- Evidence: `crates/api/src/model.rs:478-529` defines create and telemetry
  promotion requests without `confirmed`, while only adapter promotion includes
  `confirmed` at `crates/api/src/model.rs:535-545`. The API writes without
  confirmation in `crates/api/src/routes_network.rs:38-55`, toggles plans in
  `crates/api/src/routes_network.rs:94-127`, and promotes telemetry in
  `crates/api/src/routes_network.rs:130-164`; only adapter promotion checks
  confirmation at `crates/api/src/routes_network.rs:226-233`. The repository
  upserts by name in `crates/api/src/repository_network.rs:148-183` before
  falling back to insert at `crates/api/src/repository_network.rs:186-220`.
  The frontend submits save/toggle/import directly at
  `frontend/src/panels/TopologyPanel.tsx:397-415`,
  `frontend/src/panels/TopologyPanel.tsx:288-300`,
  `frontend/src/panels/TopologyPanel.tsx:976-983`, and
  `frontend/src/panels/topology/TopologyPromotionPanel.tsx:140-147`. The CLI
  `tunnel-plan --save` posts the plan without confirmation at
  `crates/vpsctl/src/commands_network.rs:888-897`.
- Notes: This is distinct from stale frontend confirmation issues for network
  apply and OSPF jobs. Those concern confirmed job dispatch snapshots; this
  issue is the missing backend confirmation contract for durable topology
  state itself.
- Resolution: Fixed by requiring backend `confirmed` for tunnel-plan save,
  enable/disable, and telemetry promotion. Frontend now opens reviewed
  confirmation prompts and submits frozen snapshots. CLI and VTY
  `tunnel-plan --save` and `tunnel-promote-telemetry` require/pass
  `--confirmed`.
- Verification: `create_tunnel_plan_requires_explicit_confirmation`, VTY
  tunnel-plan/network parser tests, frontend topology confirmation tests and
  screenshot audit.

### AUD-071: Job And Server-Job Cancellation Bypass The Confirmation Contract

- Severity: Medium/High
- Status: Fixed
- Area: API/Frontend/CLI/Jobs
- Context: Job cancellation can mark queued targets canceled and send cancel
  requests to agents running backups, restores, file operations, updates,
  process changes, terminal sessions, or network jobs. Server-job cancellation
  can stop queued server-side maintenance such as artifact cleanup.
- Root Cause: The normal job cancel request model has only an optional reason
  and no `confirmed` field. The route requests cancellation immediately after
  `jobs:write` authorization. Server-job cancellation is a path-only POST with
  no request body, reason, or confirmation. Frontend and CLI/VTY server-job
  cancel callers post an empty object directly.
- Impact: A direct API client, CLI command, VTY command, or dashboard click can
  cancel production work without the same reviewed-confirmed contract used for
  job creation, artifact cleanup creation, backups, restores, file transfers,
  schedules, and other mutating workflows. This can interrupt long-running
  fleet operations, leave operator-visible work in canceled/partial state, and
  make audit records show an intentional cancel even though no final reviewed
  snapshot was required.
- Evidence: `crates/api/src/model.rs:910-915` defines `CancelJobRequest`
  without `confirmed`. `crates/api/src/routes_jobs.rs:47-63` authorizes and
  immediately requests cancel, while `crates/api/src/repository_jobs.rs:2177-2237`
  cancels queued targets, marks active targets cancel-requested, and writes a
  cancel audit record. `crates/api/src/routes_server_jobs.rs:77-88` cancels
  server jobs with only `jobs:write`. The frontend server-job cancel path posts
  directly at `frontend/src/panels/jobs/ServerJobsPanel.tsx:85-89` and
  `frontend/src/hooks/useJobsData.ts:525-532`. CLI/VTY server-job cancel sends
  `{}` without a confirmation option at `crates/vpsctl/src/commands_jobs.rs:379-388`
  and `crates/vpsctl/src/vty_direct.rs:519-525`.
- Notes: The clean contract should require an explicit confirmation bit and a
  reviewed snapshot for cancelable job/server-job state, including at least job
  ID, command/server-job type, current status, affected target count, and
  reason when provided.
- Resolution: Fixed by requiring `confirmed` for normal job cancellation and
  server-job cancellation before repository/gateway state changes. Frontend
  server-job cancel now uses a visible reviewed confirmation prompt; CLI and
  VTY server-job cancel require/pass `--confirmed`.
- Verification: `job_cancel_routes_require_explicit_confirmation`, server-job
  cancellation prompt screenshot audit, CLI/VTY confirmed-path search.

### AUD-072: Non-Unique VPS Display Names Make Name Selectors Ambiguous For Production Jobs

- Severity: Medium/High
- Status: Confirmed
- Area: API/Frontend/CLI/Inventory/Selectors
- Context: Operators can target jobs, schedule target updates, VTY commands,
  and dashboard previews with selectors such as `name:<display-name>` or
  `name = "..."`. Display names are also edited from Fleet details and can be
  imported during direct identity management.
- Root Cause: `clients.display_name` is ordinary text with no uniqueness
  constraint among visible clients. Identity import and alias update accept any
  valid display name without checking collisions, and the alias update route
  writes immediately without a reviewed confirmation. The selector evaluator
  maps `name` to `vps.display_name` and bulk resolution filters every visible
  client with that predicate, so duplicate display names are valid and all
  duplicates match.
- Impact: A practical operator action can make `name:db` or `name:"prod edge"`
  resolve to more VPSs than intended. Destructive jobs, file operations,
  updates, restores, config applies, schedule target updates, and CLI/VTY
  commands that rely on display-name selectors can then affect the wrong
  machines or an unexpectedly broad set. This is especially risky in a 20+ VPS
  fleet where copied names, replacement hosts, and manual renames are normal.
- Evidence: The canonical schema defines `clients(id TEXT PRIMARY KEY,
  display_name TEXT NOT NULL, ...)` without a display-name uniqueness
  constraint in `migrations/0001_identity_access.sql:37-61`. Identity import
  derives an arbitrary display name at
  `crates/api/src/repository_key_lifecycle.rs:31-36` and inserts it directly at
  `crates/api/src/repository_key_lifecycle.rs:212-224`. Alias update takes only
  `display_name` in `crates/api/src/model.rs:793-796`, validates only shape in
  `crates/api/src/routes_inventory.rs:924-933`, and updates the row directly in
  `crates/api/src/routes_inventory.rs:64-82` and
  `crates/api/src/repository_inventory.rs:939-972`. The backend selector path
  resolves visible clients and applies `agent_matches_selector_expression` at
  `crates/api/src/repository_inventory.rs:1044-1129`; that wrapper builds
  `VpsMetadata.display_name` at `crates/api/src/selector_expression.rs:15-28`.
  The shared evaluator maps `name:` and `name =` to `vps.display_name` at
  `crates/common/src/expression.rs:691-696` and
  `crates/common/src/expression.rs:774-778`, then reads display-name values at
  `crates/common/src/expression.rs:1016-1020`. The VTY help explicitly
  advertises `name:<display-name>` targeting at
  `crates/vpsctl/src/vty.rs:148-162`.
- Notes: Either visible display names must be unique if they remain selector
  keys, or name selectors must fail closed on ambiguity and force operators to
  use explicit IDs/tags. Rename/import paths should also use the same
  confirmation and collision-preview discipline as other target-affecting
  inventory changes.
  Reconfirmed after commit `07ecbe7`: this is not fixed. Display names remain
  non-unique in the canonical schema and rename/import paths, while `name:`
  selectors still resolve through `vps.display_name`.

### AUD-073: Live Terminal Output Can Grow API Job-Output Storage Without A Retention Ceiling

- Severity: High
- Status: Confirmed
- Area: API/Agent/Terminal/Storage
- Context: Operators can open interactive terminal sessions from the dashboard
  or CLI. Those sessions may run noisy commands such as `tail -f`, package
  managers, build tools, or accidental infinite output. A 20+ VPS fleet can
  have several terminal sessions active during incident response.
- Root Cause: `flow_window_bytes` bounds the agent-side terminal replay buffer,
  but the gateway forwards each live terminal stream chunk to the API and the
  API appends every chunk into `job_outputs`. There is no per-session,
  per-terminal-job, per-client, or global terminal-output retention ceiling at
  ingestion time. The replay endpoint has a response-size cap, but that only
  limits bytes returned to the operator; it does not cap stored bytes.
- Impact: A single high-output terminal command can continuously grow Postgres
  rows and/or object-store data until history retention later catches up. With
  defaults, terminal stream chunks are up to roughly 16 KiB and the configured
  job-output artifact threshold is 32 KiB, so normal live terminal chunks stay
  inline in Postgres. Multiple noisy sessions across VPSs can create control
  plane storage pressure, slow job-output queries, make terminal history costly,
  and interfere with unrelated job history and operator workflows.
- Evidence: Agent terminal sessions are capped only in memory by
  `TerminalOutputBuffer::new(input.flow_window_bytes as usize)` at
  `crates/agent/src/terminal.rs:223-229`. The live reader coalesces PTY output
  up to `TERMINAL_READ_CHUNK_BYTES * 2` and emits stream chunks repeatedly at
  `crates/agent/src/terminal.rs:878-919`, while
  `TERMINAL_READ_CHUNK_BYTES` is 8192 at
  `crates/agent/src/terminal.rs:31-33`. The protocol exposes
  `flow_window_bytes` on `TerminalOpen` at
  `crates/common/src/protocol.rs:2303-2306`, with max flow-window constants at
  `crates/common/src/protocol.rs:28-31`. API terminal ingest appends the
  streamed `CommandOutput` with `append_job_output_chunk_with_config` at
  `crates/api/src/routes_ingest.rs:329-354`. The append helper allocates
  `MAX(seq)+1` and inserts into `job_outputs` without checking accumulated
  terminal bytes at `crates/api/src/repository_job_outputs.rs:409-550`.
  `job_outputs` stores inline data by default in
  `migrations/0002_jobs_schedules_commands.sql:147-163`, and the deploy
  config sets `job_output_artifact_min_bytes = 32768` at
  `deploy/config/vpsman.toml:3-7`. The replay route caps returned bytes at
  4 MiB in `crates/api/src/routes_terminal_sessions.rs:16-18` and
  `crates/api/src/routes_terminal_sessions.rs:80-105`, but that happens after
  storage has already grown.
- Notes: This is distinct from terminal replay authorization. The expected
  production behavior is an explicit terminal-output retention policy, such as
  per-session stored-byte limits, rolling persisted windows, forced object-store
  offload, or terminal auto-closure when retained API-side bytes exceed a safe
  bound.

### AUD-074: Job-Output Object Artifacts Can Be Committed Without Cleanup-Registry Repair

- Severity: Medium/High
- Status: Confirmed
- Area: API/Object Storage/Job Outputs
- Context: Large job-output chunks are externalized to the filesystem-default
  object store when they exceed `job_output_artifact_min_bytes`. This includes
  normal production output such as large stdout/stderr, file-download payloads,
  retained output streams, and noisy terminal chunks when configured below their
  size.
- Root Cause: The job-output row is committed before the corresponding
  `server_artifacts` cleanup-registry row is registered. If registration fails
  after the `job_outputs` insert commits, the ingest path returns an error, but
  the durable output row and object bytes already exist. A later retry sees the
  same `(job_id, client_id, seq)` as `DuplicateIdentical`, accepts it, and does
  not call the registration helper because no new output was inserted, so the
  missing cleanup-registry row is never repaired.
- Impact: A transient database failure at the wrong point can leave job-output
  object bytes visible through job-output metadata but invisible to shared
  artifact cleanup. History retention can delete the `job_outputs` metadata and
  attempt to mark a matching `server_artifacts` row, but if the registry row was
  never created, the filesystem/object-store payload is orphaned. In long-running
  20+ VPS use with large command outputs or file-transfer outputs, this creates
  untracked disk growth and breaks the expectation that operator-visible
  artifact bytes are always cleanup-visible.
- Evidence: `append_job_output_chunk_with_config` inserts and commits
  `job_outputs` at `crates/api/src/repository_job_outputs.rs:434-506`, then
  registers `server_artifacts` afterward at
  `crates/api/src/repository_job_outputs.rs:527-545`.
  `record_job_outputs_starting_at` inserts committed rows at
  `crates/api/src/repository_job_outputs.rs:627-744`, then registers only
  `accepted_persisted` outputs afterward at
  `crates/api/src/repository_job_outputs.rs:780-818`. Identical duplicates are
  classified without adding to `accepted_persisted` at
  `crates/api/src/repository_job_outputs.rs:638-676`, so retry cannot repair a
  missing registry row. Job-output externalization is enabled by
  `should_externalize_output` at
  `crates/api/src/repository_job_outputs.rs:1040-1045`, and the default
  threshold is `job_output_artifact_min_bytes = 32768` in
  `deploy/config/vpsman.toml:6`. Cleanup depends on `server_artifacts` for
  job-output objects at `crates/worker/src/main.rs:1234-1292`; history
  retention only marks a matching registry row if one exists at
  `crates/api/src/repository_history.rs:778-794`.
- Notes: This is distinct from AUD-062, which covers backup, retained backup,
  file-transfer source, and handoff artifact creation. This issue is specific
  to job-output object artifacts and the idempotent duplicate retry path that
  prevents later registry repair.

### AUD-075: Audit Logs Are Readable And Exportable With Fleet-Read Scope

- Severity: Medium/High
- Status: Confirmed
- Area: API/History/Auth
- Context: Audit logs are the operator forensic record for privileged and
  destructive activity. They include who did the action, session identity,
  target IDs, job selectors, backup paths, cleanup/prune requests, lifecycle
  outcomes, and integration/configuration action metadata. In a system with
  granular read scopes, fleet inventory readers should not automatically get
  the full administrative audit trail.
- Root Cause: The direct audit-log route and the history-export path both
  authorize the `audit_logs` domain with `fleet:read`. The returned model
  includes the full `metadata` JSON for every audit row. There is no separate
  audit/history read boundary and no redacted audit-list view for lower-trust
  fleet readers.
- Impact: Any operator token intended only for fleet observation can read or
  export sensitive operational history: backup source paths, reviewed target
  snapshots, destructive-job context, cancellation reasons, cleanup selectors,
  update/release metadata, operator usernames, roles, and session IDs. This
  weakens the permission model already used to separate job payloads,
  integrations, templates, schedules, config, backups, terminals, and network
  plans. It also makes least-privilege delegation harder in production because
  broad audit visibility must be bundled with basic fleet visibility.
- Evidence: `list_audit_logs` requires only `SCOPE_FLEET_READ` and returns
  `AuditLogView` directly at `crates/api/src/routes_job_history.rs:1009-1017`.
  `AuditLogView` exposes `metadata: serde_json::Value` at
  `crates/api/src/model.rs:374-382`, and `query_audit_logs` loads the stored
  metadata without redaction at `crates/api/src/repository_jobs.rs:845-868`.
  History export authorizes `HistoryDomain::AuditLogs` with `SCOPE_FLEET_READ`
  at `crates/api/src/routes_history.rs:334-342` and serializes
  `list_audit_logs` for that domain at `crates/api/src/routes_history.rs:229-243`.
  Job audit records include selector expressions, resolved targets, operator
  username/role, and session ID at `crates/api/src/repository_jobs.rs:888-903`
  and `crates/api/src/repository_jobs.rs:1076-1088`. Backup audit records
  include source paths at `crates/api/src/repository_backups.rs:705-728`.
  History-retention prune audit records include selected domains and
  operator/session metadata at `crates/api/src/repository_history.rs:497-507`.
- Notes: This is separate from AUD-048, which covers who can prune history, and
  from AUD-026, which covers history export defaults for payload domains. The
  issue here is that full audit history itself is treated as ordinary fleet
  metadata.

### AUD-076: Terminal Stream Output Retries Are Not Idempotent

- Severity: Medium/High
- Status: Confirmed
- Area: API/Gateway/Terminal/Reliability
- Context: Interactive terminal sessions stream PTY output through the agent,
  gateway, and API. Gateway delivery to the API is at-least-once: API response
  timeouts, connection resets, bounded restarts, or transient API failures can
  cause the same queued terminal-output event to be retried.
- Root Cause: Terminal stream events carry terminal-session identity and
  `terminal_seq`, but API ingestion ignores those values for persistence
  idempotency. It appends every accepted terminal event as a new `job_outputs`
  row with the next available sequence. Unlike command-output ingestion, there
  is no fixed sequence, duplicate check, or conflict handling keyed by
  `(job_id, client_id, session_id, terminal_seq)`.
- Impact: If the API durably appends a terminal chunk but the gateway does not
  observe the success response, the gateway retries the same event and the API
  stores it again as a different job-output row. Operators can see duplicated
  terminal replay chunks, job-output storage grows faster than the real PTY
  stream, and terminal forensic history becomes less trustworthy. This is
  practical in 20+ VPS operation because transient API restarts and read
  timeouts are normal, and terminal streams can be high volume.
- Evidence: The agent emits `TerminalStreamOutput` with `job_id`, `session_id`,
  `terminal_seq`, retained range, and `CommandOutput` at
  `crates/agent/src/terminal.rs:596-616`; the protocol includes those fields at
  `crates/common/src/protocol.rs:3002-3011`. The gateway forwards terminal
  output through the normal forward queue at `crates/gateway/src/main.rs:935-950`.
  The queue retries a failed event until delivered, shutdown-deferred, or
  expired at `crates/gateway/src/api_client.rs:1529-1605`, and response-read
  timeout is treated as a retryable error at
  `crates/gateway/src/api_client.rs:1638-1695`. API terminal ingest validates
  the event but then calls `append_job_output_chunk_with_config` and publishes
  the newly allocated API output sequence at
  `crates/api/src/routes_ingest.rs:329-365`; it does not pass `session_id` or
  `terminal_seq` to a duplicate-aware write path. Command-output ingestion, by
  contrast, records with caller-provided `event.seq` and returns conflict for a
  duplicate mismatch at `crates/api/src/routes_ingest.rs:137-171`. Terminal
  replay hydrates from the persisted appended chunks at
  `crates/api/src/routes_terminal_sessions.rs:80-124`, so duplicate ingests
  become duplicate replay/storage records.
- Notes: This is distinct from AUD-073. AUD-073 is about missing storage
  ceilings for live terminal output; this issue is about at-least-once delivery
  creating duplicate terminal output because the API has no idempotency key.

### AUD-077: Terminal Final Stream Status Can Expire As Noncritical Output

- Severity: Medium/High
- Status: Confirmed
- Area: Gateway/API/Terminal/Lifecycle
- Context: A terminal session can end without an explicit operator close
  command, for example when the shell exits, the remote command terminates, or
  the agent idle-timeout reaper kills the PTY. Operators rely on the dashboard
  and CLI terminal-session list to show that the session is closed, exited, or
  idle-timed-out.
- Root Cause: Automatic terminal lifecycle evidence is delivered as
  `/internal/v1/gateway/terminal-output`, the same gateway event kind used for
  ordinary PTY stream chunks. The gateway classifies terminal output as
  noncritical and gives it a 120 second TTL. If the API is down or slow beyond
  that TTL, the final lifecycle event is dropped instead of retained like
  command output or lifecycle events.
- Impact: The API can keep the last known terminal session state as `open`
  even though the agent has removed the session after process exit or idle
  timeout. Operators can see stale open sessions, attempt input or resize
  actions against sessions that no longer exist, and lose the forensic reason
  for closure. At fleet scale this becomes noisy during API restarts, deploys,
  or network interruptions while terminals are active.
- Evidence: The agent idle reaper emits final `terminal_stream` status
  `exited` or `idle_timeout` and then removes the session at
  `crates/agent/src/terminal.rs:933-958`. `emit_stream_status` encodes those
  final statuses as `TerminalStreamOutput` with `done = true` at
  `crates/agent/src/terminal.rs:562-616`. The gateway forwards all terminal
  stream events through `/internal/v1/gateway/terminal-output` at
  `crates/gateway/src/main.rs:935-950`. Gateway event classification marks
  only command output and lifecycle events as critical at
  `crates/gateway/src/api_client.rs:1714-1718`, and assigns terminal output the
  `NONCRITICAL_EVENT_TTL` of 120 seconds at
  `crates/gateway/src/api_client.rs:434-435` and
  `crates/gateway/src/api_client.rs:1720-1725`. Expired noncritical events are
  dropped by the forward queue at `crates/gateway/src/api_client.rs:1357-1376`
  and during retry at `crates/gateway/src/api_client.rs:1569-1587`. API
  terminal-session state is derived from stored terminal status outputs in
  `terminal_sessions_from_outputs` and `build_terminal_sessions` at
  `crates/api/src/repository_terminal_sessions.rs:255-366` and
  `crates/api/src/repository_terminal_sessions.rs:576-610`, so if the final
  status is never stored, the materialized session can remain on its previous
  state. There is an earlier agent-side loss point as well: terminal stream
  delivery uses a process-wide `mpsc::channel::<TerminalStreamOutput>(64)` at
  `crates/agent/src/runtime.rs:662-664`, and `try_emit_stream_output` drops the
  event on `try_send` failure at `crates/agent/src/terminal.rs:600-616`.
- Notes: This is distinct from AUD-076. AUD-076 is about duplicate storage on
  retry after a successful append; this issue is about loss of final lifecycle
  evidence because terminal final statuses are not treated as critical control
  events.

### AUD-078: OSPF Update-Plan Reads Expose Generated Bird2 Snippets With Fleet-Read Scope

- Severity: Medium/High
- Status: Confirmed
- Area: API/Network/Auth
- Context: OSPF update plans are used to review network-cost changes before
  privileged `network_ospf_cost_update` jobs. They are not just telemetry
  summaries: each response includes the proposed Bird2 interface snippets that
  would be applied to both tunnel endpoints.
- Root Cause: `/api/v1/network/ospf-update-plans` is authorized with
  `fleet:read`, but its response model includes generated network configuration
  snippets and file paths. The earlier network scope split moved full tunnel
  plan reads to `network:read`, but this derived update-plan endpoint still
  exposes generated config under the lower fleet metadata scope.
- Impact: A fleet-read operator can inspect proposed routing configuration and
  managed Bird2 file details without `network:read`. That weakens the intended
  separation between ordinary fleet observation and network configuration
  review. In production this matters because tunnel names, endpoint clients,
  interfaces, managed routing file paths, and generated Bird2 interface blocks
  are operational network configuration, not just health telemetry.
- Evidence: `NetworkOspfUpdatePlanView` contains `bird2_file`,
  `proposed_left_bird2_interface_snippet`, and
  `proposed_right_bird2_interface_snippet` at `crates/api/src/model.rs:351-369`.
  The route requires only `SCOPE_FLEET_READ` at
  `crates/api/src/routes_network.rs:374-384`. The repository builds this view
  by cloning the full tunnel plan, changing the proposed OSPF cost, rendering
  endpoint config, and returning both rendered Bird2 snippets at
  `crates/api/src/repository_network_recommendations.rs:161-216`.
- Notes: This is narrower than AUD-023. AUD-023 covered direct full tunnel-plan
  reads and is marked fixed. This issue covers a remaining derived network
  review endpoint that still returns generated config under `fleet:read`.

### AUD-079: Network Observations Expose Runtime Command Reports With Fleet-Read Scope

- Severity: High
- Status: Confirmed
- Area: API/Network/Auth
- Context: Network observations are exposed as historical telemetry. Operators
  with ordinary fleet visibility need health summaries, trends, and high-level
  tunnel state, but should not automatically receive raw command-report payloads
  produced by runtime and Bird2 probes.
- Root Cause: `NetworkObservationView` includes the complete parsed
  `metadata` JSON from network status/probe job outputs, and the route is
  authorized with `fleet:read`. That metadata can include rendered probe argv,
  stdout, stderr, runtime adapter status command reports, Bird2 status command
  reports, managed-file inspection details, and runtime topology details. There
  is no redacted/summarized observation view for fleet readers and no
  `network:read` boundary for raw observation metadata.
- Impact: A fleet-read operator can inspect command-report payloads and network
  runtime details that are closer to job output/config evidence than ordinary
  fleet health. In production this can expose executable probe paths,
  adapter-status command arguments, Bird2 parser output, runtime interface
  details, managed file paths, stdout/stderr text, and topology metadata. This
  undermines the same permission split that moved full tunnel plans and job
  output payloads behind narrower scopes.
- Evidence: `list_network_observations` requires only `SCOPE_FLEET_READ` at
  `crates/api/src/routes_job_history.rs:1020-1030`. `NetworkObservationView`
  exposes `metadata: serde_json::Value` at `crates/api/src/model.rs:273-291`.
  `parse_network_observation` stores the complete status/probe payload as
  `metadata` at `crates/api/src/repository_network_observations.rs:424-481`.
  The history export route also authorizes `HistoryDomain::NetworkObservations`
  with `SCOPE_FLEET_READ` at `crates/api/src/routes_history.rs:334-342` and
  serializes `list_network_observations` directly at
  `crates/api/src/routes_history.rs:274-280`.
  Agent network status builds payloads containing runtime summaries at
  `crates/agent/src/network_status.rs:96-120`, managed-file inspection records
  at `crates/agent/src/network_status.rs:129-197`, runtime/Bird2 status
  reports at `crates/agent/src/network_status.rs:534-650`, and probe reports
  with `argv`, `stdout`, and `stderr` at
  `crates/agent/src/network_status.rs:906-987`. Runtime tunnel command reports
  also include `argv`, `stdout`, and `stderr` at
  `crates/agent/src/network_runtime/command_runner.rs:46-143`.
- Notes: This is separate from AUD-078. AUD-078 covers generated proposed OSPF
  config in update-plan reads; this issue covers persisted observation
  metadata that can include raw runtime command reports.

### AUD-080: Gateway Spool Files Persist The Internal API Bearer Token

- Severity: High
- Status: Confirmed
- Area: Gateway/Spool/Security
- Context: Gateway forwarder spool files are written during queue pressure,
  bounded graceful shutdown, and replayable delivery windows. They can contain
  command output, lifecycle events, terminal events, telemetry, and other
  gateway-to-API bodies. In production the spool directory is configured under
  persistent gateway storage.
- Root Cause: The spool header serializes `internal_token` for each queued
  event, and the writer creates directories/files with ordinary
  `create_dir_all`/`File::create` permissions. There is no explicit
  owner-only directory mode, no owner-only file mode, and no avoidance of
  storing the bearer token in the cache. On a typical umask, spool files may be
  group/world-readable or at least easier to leak than the original secret file.
- Impact: A local low-privilege account, backup process, log/artifact collector,
  or accidental support bundle on the gateway host can read a broad internal
  API bearer token from a spool file. That token is accepted by internal API
  ingest routes and can be used to impersonate the gateway for agent hello,
  telemetry, lifecycle, command-output, and terminal-output posts. The same
  files can also contain cached command-output and terminal payload bytes. This
  breaks the expectation that secret refs under `deploy/config/secrets/` are
  the only place the internal control token is persisted.
- Evidence: `GatewayForwardEvent` carries `internal_token` at
  `crates/gateway/src/api_client.rs:311-319`, and
  `SpooledGatewayForwardHeader` persists it at
  `crates/gateway/src/api_client.rs:376-386`. Command-output and other
  forwarder events populate that field from the configured gateway internal
  token at `crates/gateway/src/api_client.rs:166-175` and similar enqueue
  paths. `spool_event` writes the serialized header/body to disk after
  `tokio::fs::create_dir_all` and `tokio::fs::File::create`, then fsyncs and
  renames, but never sets restrictive modes at
  `crates/gateway/src/api_client.rs:914-1000`. Replay reads the header back and
  uses `header.internal_token` for reposting at
  `crates/gateway/src/api_client.rs:1770-1811`. The API accepts this token for
  internal gateway routes via `require_internal_gateway` at
  `crates/api/src/state.rs:416-425`, and those routes gate all gateway ingest
  endpoints in `crates/api/src/routes_ingest.rs`.
- Notes: The spool should either avoid persisting the token and reattach the
  current runtime secret on replay, or persist cache files only under explicit
  owner-only permissions. The file body also needs the same sensitivity
  treatment as job output and terminal replay data.

### AUD-081: Filesystem Object-Store Artifacts Rely On Default Filesystem Permissions

- Severity: High
- Status: Confirmed
- Area: API/Object Storage/Security
- Context: The filesystem object store is the default object-store mode. It is
  used for backup artifacts, file-transfer source and handoff artifacts, large
  job-output chunks, retained file-download payloads, and terminal/job-output
  object payloads when output externalization applies.
- Root Cause: The filesystem object-store writer creates parent directories and
  object files with default `create_dir_all`, `OpenOptions::create_new`, copy,
  and hard-link behavior. It does not explicitly set owner-only modes on the
  object root, nested object directories, temporary object files, or committed
  object files. Effective access therefore depends on the process umask and the
  permissions of pre-existing parent directories.
- Impact: On a typical service umask such as `022`, local users or host-side
  tooling can read object-store files directly from disk, bypassing API scopes
  such as `jobs:read`, `backups:read`, and `terminal:read`. This can expose
  plaintext job output, terminal replay chunks, file-transfer payloads, and
  retained file downloads. Even encrypted backup artifacts still leak object
  existence, key layout, sizes, and hashes outside the operator API boundary.
  This matters because the deploy/default workflow intentionally uses local
  filesystem object storage unless operators configure S3.
- Evidence: `FilesystemBackupObjectStore::put_new` creates parent directories
  with `tokio::fs::create_dir_all`, writes a temporary object with
  `OpenOptions::create_new`, and commits by hard link at
  `crates/object-store/src/lib.rs:198-231`, without setting restrictive
  permissions. `put_file_idempotent` copies a source file to a temp object and
  hard-links it into place at `crates/object-store/src/lib.rs:235-293`, again
  without mode hardening. The API builds a filesystem object store from
  `VPSMAN_BACKUP_OBJECT_STORE_DIR` or falls back to
  `deploy/runtime/data/objects/backups` at `crates/api/src/main.rs:688-711`.
  The deploy suite config defaults the object store to
  `/var/lib/vpsman/objects/backups` at `deploy/config/vpsman.toml:59`.
  Job-output externalization writes object-store artifacts via
  `externalize_output_if_needed` and `store.put_new` at
  `crates/api/src/repository_job_outputs.rs:1018-1038`; file-transfer source
  and handoff artifacts use the same store at
  `crates/api/src/routes_file_transfers.rs:105-127` and
  `crates/api/src/routes_file_transfers.rs:200-234`.
- Notes: The filesystem object store should create and verify owner-only
  directories and files, or fail closed when the configured root is not private.
  This issue is independent of object-store cleanup registry correctness.

### AUD-082: Transient Payload Spool Files In Temp Directories Rely On Default Permissions

- Severity: Medium/High
- Status: Confirmed
- Area: API/Downloads/Security
- Context: The API materializes job-output downloads, file-download bundles,
  job-output archives, file-transfer handoff reassembly, and S3 object streaming
  through local temporary files before sending or committing the payload.
  Chunked backup artifact uploads also stage uploaded artifact bytes in a temp
  directory until commit, abort, or TTL cleanup.
- Root Cause: These temporary payload files are created under `std::env::temp_dir`
  or another ordinary staging directory with `OpenOptions::create_new` and no
  explicit owner-only permissions. Names include UUIDs, but the parent temp
  directory is normally listable by local users and the files remain readable
  according to the service umask while the response, archive, handoff, or S3
  stream is being prepared.
- Impact: A local account, host backup/collection process, or compromised
  colocated service on the API host can read transient plaintext job output,
  terminal output, file-download contents, file-transfer handoff payloads, or
  S3-fetched artifacts directly from temporary files. This bypasses API scopes
  such as `jobs:read` and `terminal:read`. The exposure window is shorter than
  persistent object-store files, but the affected bytes are often exactly the
  sensitive payloads an operator requested for download.
- Evidence: `TempDownloadFile::new` builds paths in `std::env::temp_dir` at
  `crates/api/src/routes_job_history.rs:385-397`. Job-output stream and
  file-download spooling create those files with default `OpenOptions` at
  `crates/api/src/routes_job_history.rs:480-505` and
  `crates/api/src/routes_job_history.rs:517-545`; archive writers create temp
  tar files the same way at `crates/api/src/routes_job_history.rs:588-682`.
  `streaming_temp_file_body` keeps the temporary file alive while streaming at
  `crates/api/src/routes_job_history.rs:711-731`. File-transfer handoff
  reassembly writes a temp file under `std::env::temp_dir` at
  `crates/api/src/routes_file_transfers.rs:210-219` and creates it with
  default `OpenOptions` at `crates/api/src/routes_file_transfers.rs:471-505`.
  S3 object streaming writes fetched object bytes to
  `std::env::temp_dir()/vpsman-object-store-spool` with default permissions at
  `crates/object-store/src/lib.rs:489-552`. Chunked backup uploads default
  their staging root to
  `std::env::temp_dir()/vpsman-backup-upload-sessions` at
  `crates/api/src/backup_upload_sessions.rs:30-38`, create the staging file
  with default `OpenOptions` at `crates/api/src/backup_upload_sessions.rs:84-108`,
  and write session manifests with `tokio::fs::write` at
  `crates/api/src/backup_upload_sessions.rs:251-260`.
- Notes: Use private temp directories and owner-only temp files for payload
  spooling, or use anonymous/unlinked temp files where available. This is
  distinct from AUD-081, which covers persistent filesystem object-store files.

### AUD-083: Agent File-Upload Staging Exposes Payloads Before Final Modes Are Applied

- Severity: High
- Status: Fixed
- Area: Agent/File Transfer/Security
- Context: Operators use file push and resumable file-transfer jobs to place
  configuration files, credentials, scripts, backup material, or other sensitive
  payloads onto managed VPSs. The requested final mode may be private, for
  example `0600`, and operators expect that mode to protect the bytes being
  delivered.
- Root Cause: The agent creates upload staging files with default filesystem
  creation permissions, writes payload bytes into them, and only applies the
  requested final mode later. Inline/chunked file push writes the full temp file
  before `set_permissions`; resumable file transfer creates a `.part` file and
  fills it chunk by chunk, then applies the final mode only during commit.
  Effective read access therefore depends on the agent process umask and the
  destination directory permissions during the staging window.
- Impact: On a typical umask such as `022`, a file intended to become private
  can be staged as group/world-readable while it is being uploaded. Local users,
  colocated workloads, backup collectors, or support tooling on the target VPS
  can read `.vpsman-upload-*` or `.vpsman-transfer-*.part` files before the
  final rename. Resumable transfers make the exposure window practical because
  staging files can persist across chunk retries or interrupted uploads. This
  bypasses the operator's requested file mode and is serious for pushed secrets,
  service configs, deployment scripts, and restore material.
- Evidence: Resumable transfer start creates `paths.temp` with
  `tokio::fs::File::create` at `crates/agent/src/file_push.rs:190-198`, writes
  chunk bytes into that same file at `crates/agent/src/file_push.rs:250-255`,
  and applies `metadata.mode` only during commit at
  `crates/agent/src/file_push.rs:299-306`. Non-resumable file push builds a
  destination-adjacent `.vpsman-upload-*` path, writes all payload bytes with
  `tokio::fs::write`, and sets the requested mode only afterwards at
  `crates/agent/src/file_push.rs:384-415`.
- Notes: Staging files should be created with restrictive owner-only modes from
  the start, then widened only after validation and immediately before final
  commit if the operator requested a broader final mode. The resumable metadata
  file under `std::env::temp_dir` also exposes destination path, size, hash, and
  temp path and should receive the same local-permission review.
- Resolution: Fixed by creating file-push and resumable upload staging files
  with owner-only modes from the first byte, applying final mode and ownership
  through opened descriptors, and storing resumable metadata in a private
  agent-owned directory.

### AUD-084: Agent Updater Cannot Follow The Official GitHub Release Redirects

- Severity: High
- Status: Fixed
- Area: Agent/Updates/Reliability
- Context: The default agent configuration and operator update workflows use
  the official GitHub Releases manifest URL
  `https://github.com/mnihyc/vpsman/releases/latest/download/version.json`.
  Manual update-check jobs and the autonomous updater both rely on the agent
  being able to fetch that manifest, then fetch the asset URLs contained in it.
- Root Cause: The agent uses a custom minimal HTTP client for update downloads.
  It sends one `GET`, reads one response, and `decode_http_response` rejects any
  status other than `200`. It does not parse `Location` or follow 301/302/303/
  307/308 redirects. GitHub's `latest/download` release URL is redirect-based,
  and release asset downloads also redirect to GitHub's release-asset storage.
- Impact: The default update URL that operators are expected to use can fail
  before the manifest is parsed. That makes autonomous updater checks and
  manual update-check jobs report update-fetch failures instead of staging the
  official release. At fleet scale this blocks routine agent upgrades through
  the documented/default workflow and can leave 20+ VPSs on an old agent until
  operators manually replace URLs with already-resolved, short-lived asset URLs.
- Evidence: `crates/agent/src/update.rs:455-479` performs a single HTTP(S)
  request and calls `decode_http_response`; `crates/agent/src/update.rs:567-579`
  extracts the status and bails unless it is exactly `200`. The default
  manifest URL is emitted by `crates/common/src/config/models.rs:101-103`,
  `docs/agent-config.example.toml:37`, `deploy/install-agent.sh:218`, and the
  frontend update controls. On 2026-06-18,
  `curl -I https://github.com/mnihyc/vpsman/releases/latest/download/version.json`
  returned `HTTP/2 302` with `Location:
  https://github.com/mnihyc/vpsman/releases/download/v0.1.1/version.json`;
  following redirects then produced another `302` to
  `release-assets.githubusercontent.com` before the final `200`.
- Notes: The updater should support bounded same-scheme-or-HTTPS redirects for
  manifest, checksum, and artifact downloads, while preserving the existing
  size limits, HTTPS policy, localhost-only HTTP exception, and checksum
  verification. Redirect handling is also needed for manifest-provided asset
  URLs, not only the `latest/download/version.json` entry point.
- Fix: The agent updater now uses async `reqwest` for manifest, checksum, and
  artifact HTTP(S) downloads. Redirects are bounded and allowed only to HTTPS
  or localhost HTTP development URLs, response bodies remain capped at 16 MiB,
  and the existing SHA256 verification remains mandatory before staging.

### AUD-085: vpsctl Local Download Staging Uses Default-Readable Named Temp Files

- Severity: Medium/High
- Status: Confirmed
- Area: CLI/Downloads/Security
- Context: Operators use `vpsctl` to download job-output payloads, target
  status bundles, file-transfer source artifacts, handoff artifacts, and
  resumable remote files from managed VPSs onto an operator workstation or
  bastion host. Those payloads can include logs, config files, credentials,
  scripts, backup material, or terminal/job output that required scoped API
  access to retrieve.
- Root Cause: The CLI writes downloaded bytes through destination-adjacent named
  temp files using `File::create` or `OpenOptions::create(true)` without
  explicit owner-only modes. The final destination is only reached after the
  temp file is fully written and renamed. Effective local readability therefore
  depends on the operator process umask and parent directory permissions.
- Impact: On a typical umask such as `022`, a downloaded secret or payload can
  be readable by other local users, host backup/indexing tooling, or colocated
  processes while `vpsctl` is streaming it. Resumable file-transfer downloads
  make this more practical because `.part` files can persist across retries or
  interrupted runs. This bypasses the intent of API scopes such as `jobs:read`,
  `backups:read`, and `terminal:read` once bytes reach the official CLI path.
- Evidence: Generic API download-to-file creates a destination-adjacent temp
  path and opens it with `File::create` at `crates/vpsctl/src/http.rs:356-384`.
  That helper is used for job-output chunk downloads and target-status archives
  at `crates/vpsctl/src/commands_jobs.rs:179-191` and
  `crates/vpsctl/src/commands_jobs.rs:293-311`, and for file-transfer source
  and handoff artifact downloads at `crates/vpsctl/src/commands_file_transfers.rs:119-121`
  and `crates/vpsctl/src/commands_file_transfers.rs:204-207`. Resumable remote
  file downloads create `.vpsman-download-*.part` with
  `OpenOptions::create(true)` at `crates/vpsctl/src/commands_file_transfer_download.rs:480-487`,
  write chunks into it at `crates/vpsctl/src/commands_file_transfer_download.rs:274-307`,
  and rename it only after hash verification at
  `crates/vpsctl/src/commands_file_transfer_download.rs:323-330`.
- Notes: CLI download temp files should be created owner-only from the start,
  preferably with `create_new` and explicit permissions, then renamed into
  place. If a destination should intentionally be group/world-readable, that
  should be an explicit post-download operator action rather than the staging
  default.

### AUD-086: Agent Restore Staging Exposes Restored Payloads Before Archive Modes Are Applied

- Severity: High
- Status: Fixed
- Area: Agent/Restore/Security
- Context: Operators restore selected files and agent config from encrypted
  backup artifacts. Restored files can include private service configs, keys,
  credentials, or application data, and the backup manifest carries the mode
  that should protect each restored file.
- Root Cause: The restore writer materializes restored bytes into a
  destination-adjacent `.vpsman-restore-*.tmp` file with `tokio::fs::write`,
  then applies the archive mode with `set_permissions`, and only then renames
  into place. The temporary file's initial readability is controlled by the
  agent process umask rather than the archived mode. Successful-restore
  rollback similarly copies rollback payloads through a named
  `.vpsman-restore-rollback-*.tmp` file before setting the rollback
  permissions.
- Impact: A restored file intended to be private, such as a `0600` config or
  key file, can be temporarily group/world-readable on the target VPS during
  restore. Local users, colocated services, backup/indexing agents, or support
  tooling can read the staging file before the final mode is applied. This is
  production-relevant because restores are normally performed precisely for
  sensitive operational state and may run across multiple VPSs during incident
  recovery.
- Evidence: `restore_entry` calls `write_restored_file` with the decoded
  archive bytes and manifest mode at `crates/agent/src/restore.rs:421-429`.
  `write_restored_file` creates the destination directory, optionally copies an
  existing file to a rollback snapshot, writes restored bytes to
  `.vpsman-restore-*.tmp` with `tokio::fs::write`, and only afterwards applies
  `std::fs::Permissions::from_mode(mode)` at
  `crates/agent/src/restore.rs:580-628`. The explicit restore-rollback command
  copies a rollback snapshot to `.vpsman-restore-rollback-*.tmp` and applies
  permissions only after the copy at `crates/agent/src/restore_rollback.rs:157-205`.
- Notes: Restore and rollback staging files should be created owner-only from
  the first byte, then widened only immediately before final rename if the
  archive mode intentionally allows broader access. Existing rollback snapshots
  should also be reviewed so private original contents are never staged with
  weaker default permissions.
- Resolution: Fixed by creating restore, rollback snapshot, and explicit
  rollback temp files with owner-only modes before writing bytes, then applying
  the archived/original mode on the opened descriptor before final rename.

### AUD-087: Restore Destination Roots Can Be Escaped Through Symlinked Parent Components

- Severity: High
- Status: Fixed
- Area: Agent/Restore/Safety
- Context: Restore jobs can map backed-up absolute paths under a reviewed
  `destination_root`, for example restoring `/etc/app.conf` into
  `/restore-root/etc/app.conf` instead of overwriting the live path. Operators
  use this to rehearse, inspect, or stage restores safely before activation.
- Root Cause: Restore path validation is lexical only. It rejects `.` and `..`
  segments, then computes `destination_root + relative_from_absolute(entry.path)`.
  The write path uses ordinary `create_dir_all`, `metadata`, temp-file writes,
  copies, and renames against that destination. It does not verify that each
  existing parent directory below `destination_root` is a real directory rather
  than a symlink, and it does not verify the final canonical destination remains
  under the reviewed root.
- Impact: A symlink already present under the restore root can redirect restore
  writes outside the reviewed destination tree. For example, if
  `/restore-root/etc` is a symlink to `/`, restoring `/etc/app.conf` under
  `/restore-root` writes through `/restore-root/etc/app.conf` to `/app.conf` or
  creates directories outside the reviewed root, depending on the symlink
  target and remaining path. Because agents commonly run with elevated file
  privileges, this can overwrite or create files outside the operator-reviewed
  restore scope. This is practical on shared, compromised, or application-owned
  restore roots and undermines safe restore rehearsals across a fleet.
- Evidence: `validate_restore_scope` and `validate_safe_absolute_path` only
  reject unsafe path segments at `crates/agent/src/restore.rs:394-408` and
  `crates/agent/src/restore.rs:688-702`. `destination_path_for_entry` joins a
  selected-path entry under `destination_root` with
  `relative_from_absolute(entry.path)` at `crates/agent/src/restore.rs:561-574`.
  `write_restored_file` then creates parent directories and writes/copies by
  path at `crates/agent/src/restore.rs:580-628` without no-follow or canonical
  containment checks. A local reproduction with a symlinked child under a temp
  restore root showed `os.makedirs(root/etc/app)` creating `app` under the
  symlink target, matching the behavior this path relies on.
- Notes: Restore should walk the destination tree with no-follow checks or
  equivalent `openat`/directory-fd semantics, reject symlink components below a
  `destination_root`, and verify any final canonical path remains contained in
  the reviewed root before writing rollback snapshots or restored bytes.
- Resolution: Fixed by resolving and creating restore destination parents with
  no-follow directory traversal. Existing symlink components below a reviewed
  restore root now fail closed before restored or rollback bytes are written.

### AUD-088: Backup Jobs Follow Selected-Path Symlinks Without An Explicit Operator Choice

- Severity: High
- Status: Confirmed
- Area: Agent/Backup/Safety
- Context: Manual and scheduled backup jobs capture operator-selected absolute
  file paths from managed VPSs. These backups often run with the agent's
  elevated file privileges and may target application-owned directories across
  many VPSs.
- Root Cause: The backup reader validates only that selected path strings are
  absolute. It then uses `tokio::fs::metadata` and normal file reads, both of
  which follow symlinks. There is no `follow_symlinks` field, no no-follow
  default, and no check that the file read is the same non-symlink path the
  operator reviewed.
- Impact: A selected backup path can be replaced with a symlink before a manual
  or scheduled run, causing the agent to archive the symlink target instead of
  the reviewed path. On a root-running agent, an application user who can write
  inside a backed-up directory can redirect scheduled backups toward sensitive
  local files that the operator did not intend to capture. This can leak
  unexpected secrets into backup artifacts, break restore assumptions, and make
  backup evidence misleading for fleet operations.
- Evidence: `validate_backup_scope` only checks absolute paths at
  `crates/agent/src/backup.rs:448-458`. `read_backup_file` validates the
  archived selected path string, then calls `tokio::fs::metadata(path)` at
  `crates/agent/src/backup.rs:319-339`, which follows symlinks. It later reads
  the same path with normal file I/O through `read_backup_file_bounded` at
  `crates/agent/src/backup.rs:348` and `crates/agent/src/backup.rs:405-418`,
  so the archived bytes come from the symlink target while the manifest still
  records the original selected path.
- Notes: Backup should either reject symlink selected paths by default, preserve
  symlink metadata without following, or require an explicit operator
  `follow_symlinks` choice with clear manifest/audit evidence. The default
  should match the safer file-operation behavior already used elsewhere in the
  agent.

### AUD-089: Text-File Edit Staging Exposes Payloads Before Final Modes Are Applied

- Severity: Medium/High
- Status: Fixed
- Area: Agent/File Browser/Security
- Context: Operators can edit a single VPS text file from the file browser, or
  dispatch bulk text writes from the multi-file panel. These workflows are used
  for service configs, scripts, credentials, and other small files where the
  requested final mode may be private, for example `0600`.
- Root Cause: The agent implements `file_write_text` through a
  destination-adjacent `.vpsman-edit-*` temporary file created by
  `tokio::fs::write`. The requested mode is applied only after the payload bytes
  have already been written. Effective staging-file readability is therefore
  controlled by the agent process umask and destination directory permissions
  during the write window.
- Impact: On a typical umask such as `022`, an edited file intended to become
  private can be briefly staged as group/world-readable on the target VPS before
  the final chmod and rename. Local users, colocated workloads, backup
  collectors, or support tooling can read `.vpsman-edit-*` payloads while a
  privileged agent writes them. This bypasses the operator's requested file
  mode and is practical for secrets, deployment scripts, and production
  configuration edited through the console.
- Evidence: `execute_file_write_text` validates the requested mode and then
  calls `atomic_write` at `crates/agent/src/file_browser.rs:397-496`.
  `atomic_write` creates a temp path named `.vpsman-edit-{file_name}-{uuid}`,
  writes all payload bytes with `tokio::fs::write`, and only afterwards calls
  `tokio::fs::set_permissions` at `crates/agent/src/file_browser.rs:937-952`.
  The command is exposed by `JobCommand::FileWriteText` at
  `crates/common/src/protocol.rs:2463-2475` and is reachable from both the
  single-file editor (`frontend/src/panels/jobs/FileBrowserPanel.tsx:285-305`)
  and bulk multi-file write action
  (`frontend/src/panels/jobs/MultiFileActionsPanel.tsx:1246-1265`).
- Notes: This is separate from AUD-083, which covers file-push and resumable
  file-transfer upload staging. `file_write_text` should create its staging file
  owner-only from the start, then widen only immediately before final commit if
  the operator requested a broader final mode.
- Resolution: Fixed by routing text writes through owner-only descriptor-held
  temp files, applying the final mode with `fchmod`, syncing, and committing by
  fd-relative rename.

### AUD-090: Chown On A Symlink Reports Success While Changing Nothing

- Severity: Medium/High
- Status: Fixed
- Area: Agent/File Browser/Ownership
- Context: Operators can run single-VPS or bulk `file_chown` jobs to repair
  ownership on files and directories before service restarts, deployments, or
  restore validation. Symlink paths are common in production config trees, for
  example enabled-site links, release-current links, and shared application
  config links.
- Root Cause: The `file_chown` implementation validates only that the path
  exists, then calls `chown_path_recursive`. That helper immediately returns
  `Ok(())` when the operand is a symlink. The caller does not distinguish
  "skipped symlink" from a real ownership mutation and always emits a
  successful `status: "changed"` response when owner/group resolution produced
  at least one ID.
- Impact: An operator can chown a symlink path and receive a successful changed
  result even though neither the symlink nor its target was changed. In normal
  operational use this can leave service files, deployed release targets, or
  restored configs owned by the wrong user while the job result and audit trail
  imply the repair succeeded. The issue is especially misleading for bulk
  actions where one symlinked path can be part of a larger production run.
- Evidence: `execute_file_chown` checks `symlink_metadata` only for existence at
  `crates/agent/src/file_browser.rs:679-704`, then invokes
  `chown_path_recursive` and unconditionally reports `status: "changed"` at
  `crates/agent/src/file_browser.rs:724-746`. `chown_path_recursive` returns
  `Ok(())` for any symlink before calling the platform `chown` helper at
  `crates/agent/src/file_browser.rs:1224-1238`. The UI prompts say
  "Apply owner/group" for chown in
  `frontend/src/panels/jobs/FileBrowserPanel.tsx:448-465`, and the bulk panel
  exposes the same `file_chown` action without a symlink-specific policy.
- Notes: The clean behavior should be explicit and auditable: reject top-level
  symlink operands by default, or add a deliberate symlink policy and return a
  skipped result when nothing was changed. A successful `changed` result should
  mean at least one path was actually mutated.
- Resolution: Fixed by routing chown through no-follow descriptor traversal and
  reporting `unchanged` when the selected path resolves to no actual ownership
  mutation, including top-level symlink operands.

### AUD-091: Agent-Local Restore Archives Are Not Required To Be Hash-Bound

- Severity: High
- Status: Confirmed
- Area: Agent/Restore/Safety
- Context: Operators can run a destructive restore by giving the agent an
  absolute path to a restore archive already present on the target VPS. This is
  useful for large or externally staged artifacts, but the reviewed operation
  should still bind to the exact artifact bytes that will be restored.
- Root Cause: When `archive_path` is supplied, the API and frontend allow
  `archive_sha256_hex` and `archive_size_bytes` to be absent. The agent then
  reads whatever file exists at that path during execution and validates the
  hash or size only if optional values were supplied. The confirmation snapshot
  therefore binds only to a mutable filesystem path, not to immutable archive
  content.
- Impact: A local archive can be replaced or modified after operator review but
  before the agent reads it, causing a destructive restore from bytes the
  operator did not verify. This is practical when the staging directory is
  application-writable, shared with deployment tooling, populated by a separate
  transfer workflow, or reused across restore rehearsals. It can restore stale,
  wrong-client, corrupted, or attacker-controlled content while the job record
  still points to the reviewed path and source backup request ID.
- Evidence: `validate_restore_operation` accepts an `archive_path` and only
  validates `archive_size_bytes` and `archive_sha256_hex` when they are present
  at `crates/api/src/job_request.rs:497-515`. The frontend path-based restore
  operation sends `archive_size_bytes: null` and only sends
  `archive_sha256_hex` if the operator fills the optional field at
  `frontend/src/panels/BackupsPanel.tsx:851-864`. The agent reads the path and
  conditionally checks size/hash only when values are present at
  `crates/agent/src/restore.rs:260-284`.
- Notes: Destructive agent-local restores should require at least a SHA-256
  binding, and preferably size plus hash, before dispatch. The reviewed
  confirmation should show those immutable values, and the agent should refuse
  to restore path-based archives when the digest is missing.

### AUD-092: Agent-Local Restore Reads The Entire Archive Into Memory Without A Cap

- Severity: High
- Status: Confirmed
- Area: Agent/Restore/Reliability
- Context: Agent-local restore archives are the practical path for artifacts
  too large to inline through the API. These restores can run on small VPSs and
  may be triggered during incident recovery or fleet migration.
- Root Cause: The agent uses `tokio::fs::read` for `archive_path`, loading the
  entire archive into memory before decoding or validating it. There is no
  maximum byte cap for path-based archives, and no streaming decoder. The API
  only checks that an optional size, if provided, is nonzero; it does not impose
  a restore archive maximum for path-based input.
- Impact: Pointing a restore job at a large file, sparse-expanded file,
  accidental log, or maliciously staged artifact can allocate the whole file in
  the agent process. On small VPSs this can exhaust memory, kill the agent, and
  leave restore state ambiguous or partially recovered. This is especially risky
  because path-based restore is the only apparent way to avoid the API's inline
  restore archive size limit.
- Evidence: `archive_bytes_from_source` calls `tokio::fs::read(path)` at
  `crates/agent/src/restore.rs:260-271`, then hashes and decodes the in-memory
  `Vec<u8>`. `validate_restore_operation` accepts path-based archives at
  `crates/api/src/job_request.rs:497-515` without an upper bound, while inline
  artifact preparation explicitly caps archives to `MAX_INLINE_FILE_PUSH_BYTES`
  at `crates/api/src/backup_artifact_crypto.rs:83-88`.
- Notes: Restore should either stream path-based archives with bounded memory
  and size accounting, or enforce a configured maximum before reading. Hash
  validation should be performed incrementally so large artifacts do not require
  a whole-file allocation.

### AUD-093: Hot Config Rewrites Can Lose Restrictive Config-File Permissions

- Severity: Medium/High
- Status: Confirmed
- Area: Agent/Config/Security
- Context: Agent hot config and incremental config-patch jobs persist the
  agent's own TOML configuration. That file contains immutable identity
  material such as `client_id`, `noise.client_private_key_hex`, and
  `noise.server_public_key_hex`; the one-line installer deliberately writes the
  config under a private directory and applies `chmod 600` to the file.
- Root Cause: `persist_config_update` writes rollback and replacement config
  files with `fs::copy`/`fs::write` and then renames the temporary file into the
  live config path. The replacement temp file is created with process-umask
  defaults rather than inheriting the existing config mode or explicitly setting
  owner-only permissions before rename. When no source config exists, the
  rollback copy is also created with default permissions.
- Impact: A hot-config or data-source config-patch job can silently replace a
  previously restrictive agent config with a more permissive file. On a default
  `0022` umask this is commonly `0644`. If the config directory is not private
  in a custom deployment, local users can read agent identity material after the
  update. Even with a private default directory, this breaks the installer's
  explicit file-mode contract and makes future packaging or operator path
  changes riskier.
- Evidence: `deploy/install-agent.sh:202-243` writes the initial config and
  applies `chmod 600`. `docs/agent-config.example.toml:9-14` shows the stored
  Noise identity fields. `crates/agent/src/config_update.rs:282-320` writes the
  rollback and replacement files using `fs::copy`/`fs::write` and renames the
  temp file over the live config without setting a restrictive mode.
- Notes: Config persistence should create rollback and temp files with explicit
  owner-only permissions, preserve the existing config mode when safe, and
  fsync/rename without widening access to secret-bearing TOML.

### AUD-094: Suite Config Saves Can Widen Secret-Bearing Config-File Permissions

- Severity: High
- Status: Confirmed
- Area: API/Suite Config/Security
- Context: Admin operators can edit the canonical suite TOML from the dashboard.
  That file is mounted into API, worker, and gateway services and contains
  private control-plane posture: database URLs, object-store locations, gateway
  identity settings, secret-file references, bind addresses, and other restart
  required service configuration. Deploy examples also carry a Postgres URL with
  credentials in the suite config.
- Root Cause: The suite-config update route writes the submitted TOML to a
  destination-adjacent temporary file with `fs::write`, then renames that file
  over the configured suite config path. It does not preserve the existing file
  mode, set owner-only permissions on the temp file before writing, or otherwise
  enforce a restrictive mode after rename. The resulting mode is controlled by
  process umask and parent-directory defaults.
- Impact: Saving the suite config through the operator UI can silently replace a
  previously restrictive `/etc/vpsman/vpsman.toml` with a group/world-readable
  file. On a default `0022` umask this is commonly `0644`. In custom deploys or
  shared admin hosts, local users or colocated processes can read database
  credentials, secret-file paths, internal service URLs, and private operational
  topology from the central config. This is production-impacting because the
  suite config is the single control-plane configuration shared by long-running
  API, worker, and gateway services.
- Evidence: `write_suite_config_atomically` creates the parent directory, builds
  a `*.toml.tmp-<uuid>` path, calls `fs::write(&tmp_path, text)`, and renames it
  over `state.suite_config_path` at `crates/api/src/routes_suite_config.rs:157-167`.
  The route exposes the raw TOML to admins and writes submitted TOML at
  `crates/api/src/routes_suite_config.rs:80-121`. The deploy config includes
  `postgres_url` and secret-file references at `deploy/config/vpsman.toml:54-83`,
  and `deploy/.env.example:5-10` describes the mounted suite config and secret
  files.
- Notes: Suite-config saves should create the temp file with owner-only mode
  before writing, preserve the existing config mode when it is already safe, and
  fsync/rename without widening access. This should be handled separately from
  AUD-093 because it affects the central API/worker/gateway control-plane
  config rather than per-agent TOML.

### AUD-095: Suite Config Audit Redaction Leaves Database URLs Visible

- Severity: High
- Status: Confirmed
- Area: API/Suite Config/Audit
- Context: Admin operators can save suite TOML through the dashboard. The API
  records a `suite_config.updated` audit entry containing the old and new config
  after applying the suite-config redactor. Audit logs are operational records
  that can be viewed, exported, retained, or copied during incident review.
- Root Cause: The suite-config redactor only redacts keys whose names contain
  `secret`, `token`, or `password`, or end in `_key`/`_key_hex`, while leaving
  other URL-valued fields untouched. The canonical suite config uses
  `database.postgres_url` for the Postgres connection string, and deploy
  examples include credentials inside that URL. `record_suite_config_audit`
  persists the redacted old/new config objects directly into audit metadata.
- Impact: Saving suite config can write database credentials into audit logs
  under `old.database.postgres_url` and `new.database.postgres_url`. Those
  records can then be exposed to anyone who can read or export audit history,
  copied into support bundles, retained longer than the config itself, or
  replicated into downstream log-processing systems. This is production-impacting
  because the Postgres URL is a control-plane credential and the audit trail is
  supposed to reduce risk around privileged config changes, not create another
  credential disclosure path.
- Evidence: `redact_suite_config_value` delegates to `redact_value` at
  `crates/common/src/suite_config.rs:378-405`; that function does not redact
  keys containing `url` and therefore leaves `postgres_url` intact.
  `update_suite_config` computes `old_redacted` and `new_redacted` from raw TOML
  at `crates/api/src/routes_suite_config.rs:102-115`, and
  `record_suite_config_audit` stores those values under `old` and `new` at
  `crates/api/src/repository_suite_config.rs:20-52`. The deploy suite config
  contains `postgres_url = "postgres://vpsman:vpsman@postgres:5432/vpsman"` at
  `deploy/config/vpsman.toml:54-56`.
- Notes: The redactor should treat connection URLs and DSNs as secret-bearing by
  default, especially `postgres_url`, and should avoid relying only on literal
  `password` key names. Secret-file path references can remain visible if that
  is the intended operational model, but credential-bearing URLs should not be
  persisted into audit metadata.

### AUD-096: Suite Config Can Be Applied Without A Durable Audit Record

- Severity: Medium/High
- Status: Confirmed
- Area: API/Suite Config/Audit
- Context: Saving suite TOML is a privileged control-plane operation. It can
  change API, gateway, worker, database, storage, timeout, and secret-reference
  settings. Operators rely on the response and audit trail to know exactly which
  central config changes were applied and by whom.
- Root Cause: `update_suite_config` performs the filesystem write first, then
  records the `suite_config.updated` audit entry in Postgres. The file rename
  and audit insert are not atomic with each other. If the audit insert fails
  after the rename, the route returns an error even though the suite config file
  has already been replaced, and no durable audit row records the change.
- Impact: A central config change can take effect or be picked up by hot-reload
  paths while the operator sees a failed save and the audit log has no matching
  record. During incident response or multi-operator administration, this makes
  the actual control-plane state disagree with both the user-visible response
  and audit history. It can also cause an operator to retry a change that was
  already written, compounding confusion around restart-required settings.
- Evidence: `update_suite_config` reads and redacts the old/new TOML, calls
  `write_suite_config_atomically(&state, &request.toml)?`, and only afterwards
  awaits `state.repo.record_suite_config_audit(...)` at
  `crates/api/src/routes_suite_config.rs:102-116`. The audit method inserts into
  `audit_logs` in a separate database operation at
  `crates/api/src/repository_suite_config.rs:39-52`. There is no rollback or
  reconciliation path if the second step fails.
- Notes: A clean fix should make the outcome explicit and auditable: either
  persist an intent/audit record before the filesystem mutation and follow up
  with the applied result, or make post-write audit failure non-ambiguous with a
  durable recovery marker. Returning a plain failure after replacing the config
  is the unsafe part.

### AUD-097: Suite Config Changed-Key Detection Runs After Redaction

- Severity: Medium/High
- Status: Confirmed
- Area: API/Suite Config/Audit
- Context: Suite config validation and save responses show changed keys, and the
  save path records those changed keys in the `suite_config.updated` audit entry.
  Operators use that list to understand whether a central control-plane change
  affects hot-reload fields, restart-only fields, database posture, gateway
  identity, or secret-bearing settings.
- Root Cause: The API computes `changed_keys` by comparing the redacted old and
  new TOML JSON values instead of comparing the parsed raw structure and
  redacting only values in the preview/audit payload. Any field whose value is
  replaced with the same placeholder on both sides becomes invisible to
  `changed_json_paths`.
- Impact: A real suite-config change can be applied while validation, the
  confirmation summary, the save response, and audit metadata omit the changed
  key. Today this affects fields such as `gateway.expect_client_public_key_hex`.
  It also means the correct fix for AUD-095, redacting connection URLs such as
  `database.postgres_url`, would make database URL changes disappear from
  changed-key reporting unless change detection is separated from value
  redaction. For operators, that creates an inaccurate central-config audit trail
  and can hide restart-required changes during incident review.
- Evidence: `update_suite_config` computes `old_redacted` and `new_redacted`,
  then calls `changed_json_paths(&old_redacted, &new_redacted)` at
  `crates/api/src/routes_suite_config.rs:102-105`. The validation route does the
  same with `changed_json_paths(&old_redacted, &redacted)` at
  `crates/api/src/routes_suite_config.rs:135-142`. The redactor replaces keys
  containing `secret`, `token`, or `password`, or ending in `_key`/`_key_hex`,
  at `crates/common/src/suite_config.rs:382-405`. The canonical restart-required
  fields include `gateway.expect_client_public_key_hex` and
  `database.postgres_url` at `crates/common/src/suite_config.rs:312-326`.
- Notes: Compute changed key paths from parsed unredacted TOML, then use the
  redactor only for displayed and persisted values. The changed-key list should
  include field names even when the values themselves are hidden.

### AUD-098: Suite Config Save Review Can Use A Stale Validation Result For A Newer Draft

- Severity: High
- Status: Confirmed
- Area: Frontend/Suite Config
- Context: The System Config panel requires operators to validate suite TOML,
  review changed keys and hot-reload/restart impact, unlock privilege, then
  confirm a save. This is the central config path for API, gateway, worker,
  database, storage, timeout, secret-ref, and capacity settings.
- Root Cause: The frontend stores only the latest validation response, not the
  TOML text or hash that produced it. `validateDraft` sends the current
  `draftToml` asynchronously and stores the returned result when it resolves.
  If the operator edits the TOML while validation is in flight, the edit handler
  clears validation, but the older validation response can later arrive and
  repopulate `validation` for a different current draft. The save path then
  submits the mutable current `draftToml`, not a frozen validated snapshot.
- Impact: An operator can review changed-key counts, redacted JSON, hot-reload
  fields, and restart-required fields for draft A, then save draft B without a
  fresh validation/review if the validation response for A wins the race after
  editing. The API reparses draft B, so malformed TOML is rejected, but the
  operator-visible review can describe a different central config from the one
  written. For production control-plane settings, this undermines the reason the
  suite-config save has a validation and confirmation workflow.
- Evidence: `validateDraft` awaits `onValidate(draftToml)` and then stores the
  result with `setValidation(result)` at `frontend/src/panels/SystemPanel.tsx:1468-1480`.
  TOML and structured field edits clear `validation` and close confirmation at
  `frontend/src/panels/SystemPanel.tsx:1521-1531` and
  `frontend/src/panels/SystemPanel.tsx:1643-1650`, but there is no request
  generation, draft hash, abort controller, or equality check before accepting
  the async validation result. `reviewDisabled` is driven by `validation` at
  `frontend/src/panels/SystemPanel.tsx:1456`, and `saveDraft` sends the current
  `draftToml` to `onUpdate` at `frontend/src/panels/SystemPanel.tsx:1484-1511`.
  The hook posts that TOML with `confirmed: true` at
  `frontend/src/hooks/useSystemData.ts:65-72`.
- Notes: Bind validation to a draft hash or request generation, ignore stale
  validation responses, and freeze the exact TOML plus validation result when
  opening confirmation. Confirm should submit only that reviewed snapshot, and
  any edit should close the prompt and require a fresh validation.

### AUD-099: Suite Config File Replacement Is Rename-Only Without Fsync Durability

- Severity: Medium/High
- Status: Confirmed
- Area: API/Suite Config/Durability
- Context: System Config can save the central suite TOML that controls API,
  gateway, worker, database, storage, timeout, capacity, auth-throttle, and
  secret-file reference settings. Operators rely on that file surviving service
  restarts and host/container crashes after the API reports a successful save.
- Root Cause: The suite-config write helper creates a temporary path, writes the
  full TOML with `fs::write`, and renames it over the configured suite config
  path. It does not fsync the temporary file before rename, fsync the parent
  directory after rename, or clean up a failed temporary replacement. The
  function name says atomic, but the implementation only gives namespace
  atomicity, not crash-durable replacement.
- Impact: A crash, power loss, filesystem error, or container/runtime failure
  around the save can leave the operator-visible response, audit record, and
  actual suite config file out of agreement. In the worst case, the central
  control-plane TOML can revert, disappear, or be truncated after a reported
  save. For production fleets, that can lose emergency timeout/auth/storage
  changes or leave restarted services using older database, gateway, object
  store, or secret-ref settings than the operator believes are active.
- Evidence: `write_suite_config_atomically` creates the parent directory, writes
  `state.suite_config_path.with_extension("toml.tmp-<uuid>")` with `fs::write`,
  then calls `fs::rename` at `crates/api/src/routes_suite_config.rs:157-168`.
  No `File::sync_all`, directory fsync, or durable-write helper appears in that
  path. By contrast, other reliability work explicitly documents durable
  temp-file/fsync/rename behavior for gateway controlled-shutdown cache in
  `docs/job-status-model.md:85-90`.
- Notes: This is separate from AUD-094 and AUD-096. AUD-094 covers file mode
  widening; AUD-096 covers audit/file transaction ordering. This issue is the
  crash-durability of the replacement itself.

### AUD-100: Locked Login Attempts Can Still Flood Durable Audit Logs

- Severity: Medium/High
- Status: Confirmed
- Area: API/Auth/Audit
- Context: Operator login and TOTP failures are now throttled with durable
  username and IP buckets. The audit log is used for security review, operator
  accountability, and incident response. A throttled source should not be able
  to keep producing unbounded durable audit rows while locked.
- Root Cause: The login flow records an audit event for every invalid login
  attempt before recording the throttle failure, and also records an audit event
  for every request that is already locked. The durable throttle prevents
  authentication work, but it does not rate-limit or coalesce the audit writes.
  Lockout creation is audited separately, so per-attempt throttled audit rows
  are not needed to preserve the important security event.
- Impact: A misconfigured client, scanner, exposed private API, or malicious
  internal actor can keep writing `operator_auth.login_failure` and
  `operator_auth.login_throttled` rows after lockout. That can grow
  `audit_logs`, increase database write load, and bury the useful lockout and
  later recovery events operators need during an incident. The risk is practical
  because auth endpoints are reachable before operator authentication and the
  default deploy still includes known public-boundary concerns recorded
  separately in AUD-066 and AUD-067.
- Evidence: `login_operator_with_throttle` writes a throttled audit event when
  `operator_auth_throttle_locked` returns true at
  `crates/api/src/repository_auth.rs:183-200`. Unknown user, disabled/deleted
  user, bad password, missing TOTP, missing TOTP secret, TOTP decrypt failure,
  and bad TOTP each call `record_operator_auth_event(..., "failure", ...)`
  before `record_operator_auth_failure` at
  `crates/api/src/repository_auth.rs:202-344`. `record_operator_auth_event`
  always inserts an `audit_logs` row for success, throttled, or failure at
  `crates/api/src/repository_auth.rs:593-640`. Lockout creation is already
  audited through `operator_auth.lockout_created` in
  `crates/api/src/repository_auth.rs:2216-2247`.
- Notes: Keep lockout creation and successful-login-after-failures audit
  records. Coalesce or rate-limit repeated failed/throttled audit rows by
  username/IP/window, or store high-cardinality counters in the throttle table
  rather than appending one durable audit row per rejected request.

### AUD-101: Official Compose Mounts The Dashboard-Editable Suite Config Read-Only

- Severity: Medium/High
- Status: Confirmed
- Area: Deploy/API/Suite Config
- Context: The released Docker Compose deployment is the documented operator
  runtime path. The dashboard System Config panel exposes a privileged workflow
  to edit, validate, review, and save the central suite TOML that controls API,
  gateway, worker, database, storage, auth-throttle, timeout, and capacity
  settings.
- Root Cause: Compose sets `VPSMAN_SUITE_CONFIG=/etc/vpsman/vpsman.toml` for
  API, gateway, and worker, then mounts `./config/vpsman.toml` at that path
  with `:ro`. The API save route writes to `state.suite_config_path`, which is
  therefore the read-only mounted file in the normal compose deployment. The UI
  still presents this path as editable and the API has no deployment-mode guard
  or alternate writable config target.
- Impact: In the official deployment, an operator can unlock privilege, edit
  config, validate, and confirm a System Config save, but the write will fail
  at runtime because the API container cannot create/rename the replacement
  file over the read-only bind mount. This makes the primary operator config UI
  nonfunctional in the default deployment and can block emergency production
  changes to dispatcher limits, auth throttling, artifact thresholds, alert
  thresholds, gateway timing, worker retention, and schedule behavior. It also
  creates an operator expectation mismatch: docs say the compose suite config is
  the central file, while the dashboard offers to save it through an API process
  that cannot write it.
- Evidence: `deploy/compose.yml:20-27` sets
  `VPSMAN_SUITE_CONFIG=/etc/vpsman/vpsman.toml` and mounts
  `./config/vpsman.toml:/etc/vpsman/vpsman.toml:ro` for the API. The same file
  is mounted read-only into gateway and worker at `deploy/compose.yml:37-44`
  and `deploy/compose.yml:54-59`. `update_suite_config` writes the submitted
  TOML through `write_suite_config_atomically` to `state.suite_config_path` at
  `crates/api/src/routes_suite_config.rs:94-111` and
  `crates/api/src/routes_suite_config.rs:157-168`. The dashboard exposes the
  editable save workflow at `frontend/src/panels/SystemPanel.tsx:1484-1511`
  and `frontend/src/panels/SystemPanel.tsx:1539-1735`. README documents the
  compose suite config path at `README.md:77-100`.
- Notes: This is not a request to expose the API publicly. The fix should keep
  API/gateway private while making the intended operator config workflow and
  deployment mounts agree, or explicitly make the compose dashboard config view
  read-only with a documented external edit/restart flow.

### AUD-102: Suite Config Privilege Assertion Is Not Bound To The TOML Payload

- Severity: High
- Status: Fixed
- Area: API/Frontend/Suite Config/Privilege
- Context: Saving System Config requires admin auth plus the local privilege
  unlock because the suite TOML can change API/gateway/worker bind addresses,
  database URLs, storage locations, secret-file references, dispatcher capacity,
  auth-throttle limits, update policy, and operational timeouts. The privilege
  assertion is intended to bind what the operator approved to what the API
  applies.
- Root Cause: The suite-config save path verifies a generic
  `DbPrivilegeIntent` with action `suite_config.update`, target
  `suite_config`, no selector, no resolved targets, and no TOML hash. The
  frontend builds the same generic intent before submitting the current
  `draftToml`. Unlike job and schedule privilege intents, the DB privilege
  intent type has no payload hash field, so the gateway verifies only that the
  operator approved some suite-config update, not this exact TOML content.
- Impact: Any frontend race, compromised browser code, malicious extension,
  request tampering inside the private operator environment, or future UI bug
  that swaps the TOML after privilege assertion creation can send a different
  suite config under the same approved privilege intent. The API will parse and
  write the altered TOML because the gateway-approved assertion does not cover
  the body. For a central control-plane config, this defeats the strongest
  reason to require a local privilege unlock and makes AUD-098 materially worse:
  stale or mismatched review can still carry a valid privilege assertion.
- Evidence: `update_suite_config` verifies
  `DbPrivilegeIntent::new("suite_config.update", "suite_config", None, &[], true)`
  at `crates/api/src/routes_suite_config.rs:94-100`, then writes
  `request.toml` at `crates/api/src/routes_suite_config.rs:105-111`. The
  frontend builds the matching generic intent at
  `frontend/src/panels/SystemPanel.tsx:1499-1508` and sends the TOML separately
  through `onUpdate(draftToml, privilegeAssertion)` at
  `frontend/src/panels/SystemPanel.tsx:1509`. The shared
  `DbPrivilegeIntent` contains only version, action, target,
  selector_expression, resolved_targets, and confirmed at
  `crates/common/src/protocol.rs:1628-1650` and
  `frontend/src/privilege.ts:289-302`. In contrast, job and schedule intents
  explicitly include `operation_payload_hash` at
  `crates/common/src/protocol.rs:1477-1511` and
  `frontend/src/privilege.ts:241-284`.
- Notes: Add a content hash or purpose-specific suite-config privilege intent
  that covers the exact TOML bytes or canonical parsed config plus any reviewed
  changed-key metadata. The API should recompute the hash from `request.toml`
  and verify the gateway assertion against that exact payload.
- Resolution: Fixed by extending DB privilege intent with optional
  `payload_hash`. Frontend computes the hash from the draft TOML used for the
  reviewed save, and the API recomputes it from `request.toml` before verifying
  the gateway privilege assertion.
- Verification: `db_privilege_intent_binds_optional_payload_hash`, frontend
  privilege canonicalization test, System config save request assertion.

### AUD-103: Login Throttling And Auth History Use Proxy IP Instead Of The Operator IP

- Severity: Medium/High
- Status: Confirmed
- Area: API/Auth/Deploy
- Context: Operator login and TOTP failures are throttled by username and IP,
  and authentication history displays `remote_ip` for incident review. In the
  released compose deployment, the browser talks to Nginx/frontend and Nginx
  proxies `/api`, `/health`, and `/ws` to the private API container while
  setting `X-Forwarded-For`.
- Root Cause: The login route uses Axum `ConnectInfo<SocketAddr>` and passes
  `peer.ip().to_string()` into the throttle/audit path. It does not parse a
  trusted forwarded-client-IP header, and there is no trusted-proxy config.
  When the API is behind the official Nginx proxy, the TCP peer is the proxy or
  Docker-network address, not the real operator IP. Nginx already sets
  `X-Forwarded-For`, but the API ignores it.
- Impact: In the default proxied deployment, eight failed attempts from any
  source can lock the broad proxy-IP bucket and block unrelated operators who
  share that proxy path, even when their usernames are different. Conversely,
  audit history shows the proxy/container address rather than the real source,
  reducing incident usefulness. This is practical because login is the one
  unauthenticated operator endpoint and auth throttling is meant to be a
  production safety boundary. AUD-067 separately covers the broader public API
  proxy exposure; this issue remains relevant for any intended private reverse
  proxy deployment unless the API has a trusted-proxy real-IP policy.
- Evidence: `login_operator` extracts `ConnectInfo(peer)` and calls
  `login_operator_with_throttle(..., &peer.ip().to_string(), ...)` at
  `crates/api/src/routes_auth.rs:45-62`. Auth audit metadata stores that
  `remote_ip` at `crates/api/src/repository_auth.rs:593-640`, and throttle
  buckets normalize it at `crates/api/src/repository_auth.rs:2022-2026`.
  `deploy/nginx.conf:7-14`, `deploy/nginx.conf:16-23`, and
  `deploy/nginx.conf:25-32` proxy operator paths and set
  `X-Forwarded-For`, but no API route reads that header.
- Notes: Do not blindly trust `X-Forwarded-For` from arbitrary clients. Use an
  explicit trusted-proxy allowlist or deployment-mode setting, then choose the
  validated original client IP for login throttle and auth-history records.

### AUD-104: Authenticated TOTP Management Is An Unthrottled Password And Code Oracle

- Severity: Medium/High
- Status: Confirmed
- Area: API/Auth/TOTP
- Context: Operator login failures are durably throttled, but an already
  authenticated operator session can manage its own TOTP settings. These routes
  are available to all active operator roles because users must be able to set
  up, confirm, and disable their own MFA.
- Root Cause: `setup_operator_totp`, `confirm_operator_totp`, and
  `disable_operator_totp` call `require_operator`, then verify the submitted
  password and/or TOTP code directly in `repository_operator_totp`. Invalid
  password, invalid code, decrypt failure, and not-configured paths return
  errors without recording durable auth-throttle failures, using the login
  throttle buckets, or auditing repeated failures. Successful TOTP changes are
  audited, but failed attempts are not bounded.
- Impact: If an access token is stolen from any operator browser session, the
  attacker can use these authenticated TOTP management endpoints as an
  unthrottled online password oracle for that operator, bypassing the durable
  username/IP login throttling added for AUD-002. For accounts with pending or
  enabled TOTP state, the same paths also allow repeated TOTP-code attempts
  without lockout. This does not give access without a session, but it weakens
  the post-compromise boundary that should protect password material and MFA
  settings on the private operator control plane.
- Evidence: `setup_operator_totp`, `confirm_operator_totp`, and
  `disable_operator_totp` require only an authenticated operator at
  `crates/api/src/routes_auth.rs:91-149`. Invalid password/code outcomes map
  to unauthorized responses there, but no throttle or failure audit is called.
  `Repository::setup_operator_totp` verifies the password and returns
  `InvalidPassword` on failure at
  `crates/api/src/repository_operator_totp.rs:20-76`. `update_operator_totp`
  calls `verify_totp_operator_code` and returns `InvalidCredentials` on failure
  at `crates/api/src/repository_operator_totp.rs:93-180`; that helper verifies
  password, decrypts the TOTP secret, and checks the code at
  `crates/api/src/repository_operator_totp.rs:236-251`. The durable auth
  throttle path is implemented separately in
  `crates/api/src/repository_auth.rs:173-345` and is not used by these TOTP
  management routes.
- Notes: Reuse the operator auth throttle buckets for password/TOTP failures
  on these authenticated routes, keyed by username and validated client IP.
  Keep responses generic enough that failure reason does not leak which factor
  was correct. Successful TOTP changes should continue to audit normally.

### AUD-105: Derived Session Records Can Outlive The Job-Output Evidence They Require

- Severity: Medium/High
- Status: Confirmed
- Area: API/File Transfers/Terminal/Retention
- Context: File-transfer sessions and terminal sessions are materialized from
  job-output status rows so operators can list resumable uploads/downloads,
  create handoffs from completed downloads, and replay terminal sessions.
- Root Cause: `file_transfer_sessions` and `terminal_sessions` are denormalized
  tables refreshed after job-output ingestion, but history retention prunes
  `job_outputs` without rebuilding, expiring, or marking the derived session
  rows as history-pruned. The derived rows keep their last observed state even
  after the source status/output chunks they depend on are gone.
- Impact: A retained file-transfer session can still show a completed download
  and `handoff_available`, but handoff creation then fails because the download
  chunks were pruned from `job_outputs`. A terminal session can still advertise
  retained replay ranges and byte counts while replay returns empty or partial
  history because the underlying output rows were removed. In long-running
  fleets with normal job-output retention, this creates misleading operational
  state and broken operator workflows long after the original transfer or
  terminal job completed.
- Evidence: File-transfer session rows are read directly from
  `file_transfer_sessions` at `crates/api/src/repository_file_transfers.rs:69-111`
  and refreshed from recent `job_outputs` at
  `crates/api/src/repository_file_transfers.rs:115-205`. Handoff creation uses
  that derived completed-session row at
  `crates/api/src/routes_file_transfers.rs:445-468`, then reads actual chunks
  from `job_outputs` at
  `crates/api/src/repository_file_transfers.rs:208-310` and fails when chunks
  are missing at `crates/api/src/routes_file_transfers.rs:471-525`.
  Terminal session rows are similarly read from `terminal_sessions` at
  `crates/api/src/repository_terminal_sessions.rs:69-114`, refreshed from
  recent `job_outputs` at
  `crates/api/src/repository_terminal_sessions.rs:255-404`, and replay is
  reconstructed from current `job_outputs` at
  `crates/api/src/repository_terminal_sessions.rs:118-253`. Job-output
  retention deletes `job_outputs` at
  `crates/api/src/repository_history.rs:1105-1136` and
  `crates/api/src/repository_history.rs:766-802`; no matching retention path
  updates `file_transfer_sessions` or `terminal_sessions`.
- Notes: The expected behavior is not necessarily to keep all payload bytes
  forever. The production requirement is that derived session rows remain
  truthful: either prune them with their evidence, mark replay/handoff history
  as unavailable, or preserve the minimum durable evidence needed for listed
  sessions.

### AUD-106: Backup Artifact Metadata Can Be Recorded Without Object-Store Verification

- Severity: High
- Status: Confirmed
- Area: API/Backups/Object Storage
- Context: Backup artifact metadata links a backup request to encrypted
  artifact bytes that operators later download, restore, prune, and audit.
  The API also supports a metadata-recording route for artifact bytes that were
  placed into the configured backup object store outside the inline/chunked
  upload paths.
- Root Cause: `record_backup_artifact_metadata` validates only the submitted
  object key, SHA-256 string, size, encryption flag, and confirmation flag
  before inserting `backup_artifacts` and linking the backup request. It does
  not require a configured object store, verify that the object exists, verify
  the object's size/hash, or reject reuse of an object key already associated
  with another artifact record. The inline and chunked upload paths verify
  bytes before recording metadata, but the direct metadata route can publish
  unverified metadata as a durable backup artifact.
- Impact: A malformed client, operator mistake, or automation bug can mark a
  backup request as having a recorded artifact even when the object is missing,
  points at the wrong bytes, has the wrong hash/size, or is already used by a
  different backup artifact. The normal backup list then shows a restorable
  artifact, but download/restore later fails or conflicts. In production this
  can create false backup evidence, broken restore inventory, cleanup
  ambiguity, and delayed discovery that a supposedly retained backup has no
  usable bytes.
- Evidence: The metadata route calls
  `validate_backup_artifact_metadata_request` and then
  `record_backup_artifact_metadata` at
  `crates/api/src/routes_backups.rs:337-360`; unlike upload and handoff routes,
  it never opens `state.backup_object_store`. Validation only checks key shape,
  SHA-256 format, encryption flag, size range, and confirmation at
  `crates/api/src/routes_backups.rs:961-978`. The repository inserts the
  `backup_artifacts` row, links `backup_requests.artifact_id`, writes audit,
  and registers a server artifact at
  `crates/api/src/repository_backup_artifacts.rs:243-357` without object
  existence/hash verification or an object-key uniqueness guard. Later reads
  enforce the missing integrity check only after the artifact is already
  operator-visible: download verifies object size/hash at
  `crates/api/src/routes_backups.rs:726-799`, and restore preparation does the
  same through `stored_backup_artifact_bytes` at
  `crates/api/src/routes_backups.rs:845-882`.
- Notes: The route can still support externally staged artifacts, but recording
  metadata should be a validation step against the configured store. A clean
  fix should verify object existence, exact size, and exact hash before linking
  the backup request, and should prevent conflicting object-key reuse.

### AUD-107: Stale Fixed Targets Can Block Schedule Management And Apply-Now

- Severity: High
- Status: Confirmed
- Area: API/Schedules/Client Lifecycle
- Context: Schedules store a fixed target snapshot so long-lived recurring
  work does not silently change when tags, display names, or inventory state
  drift. The due-run worker now materializes hidden, deleted, revoked,
  never-connected, and missing fixed targets as visible skipped results.
- Root Cause: Schedule management routes still verify privilege by resolving
  every saved fixed target through the live target resolver. That resolver only
  returns visible clients. If any saved fixed target has since become hidden,
  deleted, revoked, or missing, `resolved_schedule_targets` returns
  `schedule_fixed_targets_not_found` before the requested schedule mutation can
  proceed. The worker due-run path has separate stale-target materialization,
  but `apply now`, enable/disable, defer, delete, and full update do not use
  that tolerant schedule snapshot model.
- Impact: A long-lived production schedule can continue to exist with stale
  fixed targets, but operators may be unable to disable, defer, delete, or
  manually apply it through normal API/UI/CLI workflows after a VPS is removed
  or hidden. Manual apply-now can also fail the whole run instead of producing
  the same skipped target rows that an automatic due run would produce. For
  20+ VPS fleets, host replacement and inventory cleanup are ordinary; schedule
  administration must remain possible even when saved targets are stale.
- Evidence: `apply_schedule_now` builds a normal `CreateJobRequest` from the
  saved schedule snapshot and calls `create_job_from_saved_schedule` at
  `crates/api/src/routes_schedules.rs:199-240`. Normal job creation rejects
  fixed targets not present in the live resolver at
  `crates/api/src/routes_jobs.rs:198-209`, so hidden/deleted/missing fixed
  targets abort apply-now instead of becoming skipped. Schedule mutations
  verify saved definitions through `verify_schedule_privilege_for_view` at
  `crates/api/src/routes_schedules.rs:147-260`; that calls
  `resolved_schedule_targets`, which resolves fixed IDs through
  `resolve_bulk_targets` and returns `schedule_fixed_targets_not_found` when
  any saved ID is no longer visible at
  `crates/api/src/routes_schedules.rs:408-450`. The automatic due-run worker
  handles the same stale states explicitly at
  `crates/worker/src/main.rs:1804-1862` and records skipped target outputs at
  `crates/worker/src/main.rs:1911-1963`.
- Notes: Privilege verification can still bind the saved target IDs, but it
  should not require stale IDs to resolve as dispatchable live clients for
  management actions. Apply-now should share the worker's fixed-snapshot
  materialization semantics or call a backend helper that records stale targets
  as skipped.

### AUD-108: Terminal Targets Can Leave The Parent Job Active After A Crash Or Side-Effect Error

- Severity: High
- Status: Fixed
- Area: API/Jobs/State Machine
- Context: Final agent output, dispatcher completion, timeout expiry, and
  operator cancellation all terminalize job targets. Once every target for a
  job is terminal, operators expect the parent job row and any schedule outcome
  to become terminal as well.
- Root Cause: Target terminalization and parent job aggregate completion are
  separate durability steps. `update_job_target_result` first commits the
  target terminal state, then performs audit, update-lifecycle, and webhook
  side effects; only the caller later invokes `refresh_job_status_from_targets`
  to finish the parent job. Queued-target cancellation and timeout expiry have
  the same split. There is no periodic repair path that finds jobs whose
  targets are all terminal while the parent job is still `queued` or `running`.
- Impact: If the API process crashes, is killed for deploy, loses the database
  connection, or hits a side-effect error after a target terminal update but
  before aggregate recomputation, the target can be permanently terminal while
  the parent job remains active. A retry of the same final output will not
  repair it because the target compare-and-set already lost and the ingest path
  skips aggregate recomputation when `update_job_target_result` returns
  `false`. This leaves completed work visible as an active job, prevents normal
  schedule outcome accounting, and pollutes fleet state for long-running 20+
  VPS operations.
- Evidence: `refresh_job_status_from_targets` is the only helper that
  aggregate-finishes a job after all targets are terminal at
  `crates/api/src/repository_jobs.rs:1526-1554`. The Postgres
  `update_job_target_result` path updates `job_targets.completed_at` at
  `crates/api/src/repository_jobs.rs:2448-2472`, then performs audit and
  lifecycle/webhook side effects at
  `crates/api/src/repository_jobs.rs:2476-2576` before returning to callers.
  Final output ingest only calls `refresh_job_status_from_targets` when
  `update_job_target_result` returned `true` at
  `crates/api/src/routes_ingest.rs:175-185`; a duplicate replay after the
  target is already terminal returns `false` and does not repair the parent.
  Dispatcher completion follows the same pattern at
  `crates/api/src/job_dispatcher.rs:418-453`. Queued cancellation commits
  target cancellation in `request_job_cancel` at
  `crates/api/src/repository_jobs.rs:2176-2237`, then the route recomputes the
  job later at `crates/api/src/routes_jobs.rs:120`.
- Notes: A clean fix should make target terminalization and aggregate job
  completion crash-recoverable. Options include finishing the job in the same
  transaction whenever the last active target is terminalized, or adding an
  idempotent reconciler that periodically finalizes active jobs with no active
  targets and runs the same schedule/webhook side effects exactly once.
- Fix: Terminal target writers now finish the parent job row in the same short
  Postgres transaction when no active targets remain. Final output, dispatcher
  outcomes, timeout expiry, cancellation, and agent-lost reconciliation use the
  shared transactional aggregate helper; post-commit side effects remain
  idempotent.

### AUD-109: Gateway Spool Replay Treats Sequence Existence As Full Output Acknowledgement

- Severity: Medium/High
- Status: Fixed
- Area: Gateway/API/Job Outputs
- Context: Gateway controlled-restart and disk-spool replay protect pending
  forwarder events, including command output. The output ingestion model
  intentionally distinguishes `Inserted`, `DuplicateIdentical`, and
  `DuplicateConflict` so a conflicting duplicate sequence cannot terminalize a
  target and should be audited as protocol corruption.
- Root Cause: On spool replay startup, the gateway asks the API whether a
  spooled command-output sequence is already acknowledged using only
  `(job_id, client_id, seq)`. The replay header keeps only those fields, and
  the API ACK route returns a sequence as acked if any qualifying output row
  exists. It does not compare stream, bytes/object metadata, exit code, or
  `done`. Therefore the gateway can delete a spooled event before the API has a
  chance to classify it as `DuplicateIdentical` or `DuplicateConflict`.
- Impact: A controlled gateway restart can silently discard a conflicting
  command output that should have reached the API conflict path. A practical
  failure mode is: sequence `N` non-final output reaches the API; a later
  spooled sequence `N` final or different payload exists on gateway disk; the
  gateway restarts; sequence-only ACK says `N` exists; the gateway deletes the
  spooled final/conflicting event. The target then lacks the final evidence and
  will likely fall back to timeout, while the protocol corruption is not
  audited. This weakens the output consistency fix and makes replay problems
  harder to diagnose during API/gateway instability.
- Evidence: Before the fix, the protocol request/response contained only
  `seqs` and `acked` sequence numbers, gateway startup replay called a
  sequence-only ACK helper, and the API returned existing sequences without
  checking output identity. The fixed implementation removed that endpoint and
  helper; this evidence is retained as historical context for the issue.
- Notes: Replay ACK should either be removed for command outputs, or it should
  use the same field-for-field identity comparison as normal output ingestion.
  A stronger spool header should include enough digest/metadata to distinguish
  identical replay from conflict before deletion.
- Fix: The command-output ACK route and sequence-only replay preflight were
  removed. Gateway startup replay now reposts spooled command-output bodies
  through normal ingest, where the API classifies inserted, identical duplicate,
  conflicting, late-terminal, and payload-mismatch outcomes.

### AUD-110: Bundled Migration-Run Can Persist A Migration Link Before Restore Dispatch Succeeds

- Severity: High
- Status: Confirmed
- Area: Frontend/CLI/Backups/Migrations
- Context: Operators can run a migration workflow that is presented as one
  combined action: create the migration link for a metadata-only restore plan
  and dispatch the corresponding restore job. The migration link table has a
  unique `restore_plan_id`, so once a plan is linked the same bundled workflow
  cannot create a second link for that plan.
- Root Cause: The bundled migration-run implementations perform two separate
  side effects in order: create the migration link, then create the restore
  job. There is no server-side atomic operation that records the link only
  after the restore job is durably accepted, and there is no compensating
  rollback if job creation fails. The CLI path is worse because it creates the
  link before downloading/decrypting the backup artifact and before building
  the restore operation.
- Impact: A failed migration run can leave durable migration evidence without
  the restore job it was supposed to accompany. Practical failures include
  wrong backup private key, missing or corrupt backup artifact, API/gateway
  rejection during job creation, operator token expiry, network failure between
  calls, or a frontend/browser interruption after the link call. The operator
  then sees a `linked_metadata_only` migration record, but no restore dispatch
  exists. Because `migration_links.restore_plan_id` is unique, retrying the
  bundled run for the same plan can be blocked by the already-created link,
  forcing manual cleanup or an out-of-band restore run and weakening the audit
  story for production migrations.
- Evidence: The schema enforces one link per restore plan with
  `restore_plan_id UUID NOT NULL UNIQUE` in
  `migrations/0004_backups_restores.sql:66-79`. The API creates links as
  `linked_metadata_only` at `crates/api/src/routes_migrations.rs:28-57` and
  `crates/api/src/repository_migrations.rs:260-340`. The CLI bundled path
  posts `/api/v1/migration-links` before calling
  `restore_run_with_credentials` at `crates/vpsctl/src/commands_migrations.rs:107-145`;
  that restore helper downloads/decrypts the artifact and only then posts
  `/api/v1/jobs` at `crates/vpsctl/src/commands_backups.rs:757-805`. The
  frontend bundled path also creates the link before creating the job at
  `frontend/src/panels/BackupsPanel.tsx:1126-1141`.
- Notes: The clean fix should make migration-run a single backend operation, or
  otherwise create the migration link only after restore job creation succeeds
  and make retries idempotent against the same reviewed plan/job intent.

### AUD-111: Restore Plans Can Record Config-Restore Intent That Later Restore-Run Rejects

- Severity: Medium/High
- Status: Confirmed
- Area: API/CLI/Backups/Restore Plans
- Context: Restore plans are durable operator metadata used by migration links
  and migration-run workflows. A plan with `include_config = true` represents
  a future config restore, and actual restore-run dispatch requires a
  destination root so config files are restored into an explicit reviewed tree.
- Root Cause: Restore-plan validation is weaker than executable restore
  validation. The API accepts `include_config = true` with
  `destination_root = NULL`, and the CLI/VTY restore-plan parser also allows
  that combination. Actual restore-run validation rejects the same intent
  because config restore requires a destination root.
- Impact: Operators can persist and link a restore plan that cannot be executed
  through the normal restore-run or migration-run path. In production, this can
  appear as a valid planned migration until the dispatch step fails, at which
  point operators must edit/recreate metadata or work around the failed plan.
  Combined with AUD-110, a linked but non-executable plan can also consume the
  unique migration-link slot for that restore plan and make the migration state
  more confusing during recovery.
- Evidence: `validate_create_restore_plan` checks target, scope, paths,
  destination path syntax, note size, and confirmation, but does not require
  `destination_root` when `include_config` is true at
  `crates/api/src/routes_restores.rs:104-142`. The restore job validator
  rejects config restores without a destination root at
  `crates/api/src/job_request.rs:489-494`. The frontend restore-run builder
  mirrors that rule with `Config restore requires a destination root` at
  `frontend/src/panels/BackupsPanel.tsx:820-824`, while restore-plan creation
  builds and submits `destination_root: restoreDestinationRoot.trim() || null`
  without the same guard at `frontend/src/panels/BackupsPanel.tsx:740-794`.
  The VTY parser for `restore-plan` enforces only backup scope and absolute
  destination syntax at `crates/vpsctl/src/vty_backups.rs:479-546`.
- Notes: Restore-plan validation should use the same safety invariants as a
  future executable restore for fields it records. Config-restore plans should
  require an absolute destination root before being persisted or linked.

### AUD-112: Deleting Or Revoking A Client Can Leave Already-Created Queued Targets Unclaimable Forever

- Severity: High
- Status: Fixed
- Area: API/Jobs/Client Lifecycle
- Context: Operators can delete a VPS inventory record or revoke its current
  key while jobs are queued for dispatch. This is practical in production when
  a host is being retired during incident response, a key is compromised, a
  bulk job was just submitted, or the dispatcher has not claimed every target
  yet.
- Root Cause: Job creation materializes every target as `queued` with
  `started_at = NULL`, `deadline_at = NULL`, and
  `process_incarnation_id = NULL`. The dispatch claim later excludes hidden
  clients and requires a live client incarnation, so a target whose client is
  deleted after job creation but before first dispatch is no longer claimable.
  The timeout sweeper only expires `dispatching` or `running` targets that have
  `started_at` and a deadline, so it never rescues this unstarted queued state.
  Client deletion and current-key revocation hide the client and end gateway
  sessions, but neither lifecycle path terminalizes or skips existing active
  job targets for that client.
- Impact: A job can remain active indefinitely with a queued target that will
  never dispatch and never timeout. In a 20+ VPS fleet, deleting one selected
  VPS shortly after submitting a bulk job can leave the entire parent job open
  forever, pollute fleet state, block schedule outcome accounting, and force
  operators into manual database repair or cancellation workflows. This is
  distinct from never-connected target handling at job creation: the client was
  valid and dispatchable when the job was created, then became unavailable
  before the dispatcher claimed it.
- Evidence: `record_dispatching_job_with_source` inserts new job targets as
  `queued` with no `started_at` or process incarnation at
  `crates/api/src/repository_jobs.rs:1095-1148`. The Postgres dispatch claim
  excludes hidden clients and requires `clients.process_incarnation_id IS NOT
  NULL` at `crates/api/src/repository_jobs.rs:1324-1344`. The timeout sweeper
  only selects `dispatching` or `running` targets with `deadline_at` and
  `started_at` at `crates/api/src/repository_jobs.rs:1968-1985`. Deleting a
  client sets `hidden_at`, clears its public key, marks status `deleted`, and
  ends gateway sessions, but does not update `job_targets` at
  `crates/api/src/repository_inventory.rs:847-930`. Revoking the current
  client key similarly sets `hidden_at`, marks status `revoked`, and ends
  gateway sessions without updating `job_targets` at
  `crates/api/src/repository_key_lifecycle.rs:714-755`. Parent job refresh keeps
  any active target status open at
  `crates/api/src/repository_jobs.rs:1526-1554`.
- Notes: A clean fix should terminalize active unstarted targets for the
  deleted client as skipped or agent_lost/control-unavailable in the same
  lifecycle operation, or add an explicit queued-unavailable sweeper. The
  terminal transition should append durable synthetic output before aggregate
  job recomputation.
- Fix: Client deletion and current-key revocation now append synthetic skipped
  status output and compare-and-set unstarted `queued` targets for that client
  to `skipped` before recomputing parent job aggregate status. Key replacement,
  current-key revocation, and client deletion now also mark old-incarnation
  `dispatching`/`running` targets as `agent_lost` with durable synthetic output.

### AUD-113: Replacing A Client Public Key Does Not Invalidate The Old Live Gateway Session

- Severity: High
- Status: Fixed
- Area: API/Gateway/Key Lifecycle
- Context: Operators can use identity upsert with `replace_existing_key` to
  rotate the stored Noise public key for a VPS. In production this is used when
  reinstalling an agent, repairing identity material, or replacing a key that
  should no longer be trusted. Operators expect the new stored key to become
  the authority for future command delivery.
- Root Cause: Key replacement updates `clients.public_key`, but it does not
  end the currently connected gateway session for that client, clear or change
  `clients.process_incarnation_id`, or mark active targets under the old
  process as lost. The gateway validates the public key only during handshake
  and then stores live sessions by `client_id` and `process_incarnation_id`.
  Dispatch delivery checks only the expected process incarnation, not the
  public-key fingerprint that authenticated the live session.
- Impact: An agent process authenticated with the old key can remain connected
  and continue receiving newly dispatched commands after the operator rotates
  the client key in the API. Because `clients.process_incarnation_id` remains
  bound to the old process, a new job can be claimed with that old incarnation
  and the gateway will deliver it to the old session. This defeats the
  operational meaning of key rotation and is dangerous if the old key or host
  is being replaced due to compromise, rebuild, or ownership transfer.
- Evidence: The replace-existing-key path updates only display name/public key
  and stale fields at `crates/api/src/repository_key_lifecycle.rs:165-215`.
  It does not update `gateway_sessions`, `clients.process_incarnation_id`, or
  `job_targets`. Gateway handshake validates the current stored public key
  through `validate_agent_public_key` at
  `crates/api/src/routes_ingest.rs:27-43` and
  `crates/api/src/repository_ingest.rs:230-270`; after that,
  `handle_agent_frame` registers the session in memory by client ID and process
  incarnation at `crates/gateway/src/main.rs:766-794`. Command dispatch checks
  only `session.process_incarnation_id` against
  `expected_process_incarnation_id` before sending the frame at
  `crates/gateway/src/control.rs:253-277`. The dispatch claim binds new
  targets to `clients.process_incarnation_id`, which key replacement leaves
  unchanged, at `crates/api/src/repository_jobs.rs:1324-1455`.
- Notes: Replacing a client key should either require the old session to be
  absent, or explicitly terminate the old gateway session, clear the current
  process incarnation, and treat active old-incarnation targets with the same
  durable lifecycle handling used for detected agent loss. Future dispatch
  should not be possible until a hello authenticated by the new key establishes
  a new incarnation.
- Fix: Key replacement now requires request-bound DB privilege verification,
  preflights the requested key change before any live-session side effect,
  asks the gateway control plane to disconnect the live session, clears the
  stored process incarnation, ends active gateway-session records, and marks
  active targets bound to the old incarnation as `agent_lost` with durable
  synthetic output before aggregate recomputation. Rejected rotations, such as
  attempts to rotate to a revoked key, do not request gateway disconnect.

### AUD-114: Delete And Key-Revoke Mark Sessions Ended Without Disconnecting The Live Gateway Session

- Severity: High
- Status: Fixed
- Area: API/Gateway/Client Lifecycle
- Context: Deleting a VPS inventory record or revoking its current client key is
  presented as access deactivation. The frontend describes key revocation as
  hiding the VPS and ending active gateway sessions, and operators use these
  workflows during compromise response, host retirement, or key replacement.
- Root Cause: The API repository paths update `gateway_sessions` rows to
  `ended`, but they do not send a control-plane disconnect command to the
  gateway process that owns the live TCP/Noise session. The gateway has an
  in-memory disconnect helper, but it is used for gateway shutdown and critical
  forwarding failures, not for API-side delete or key-revoke lifecycle
  operations.
- Impact: A revoked or deleted client can continue using its already-open
  gateway transport until that connection drops naturally. Any command already
  delivered to the agent can continue running and can still send output through
  the gateway. The API/UI can simultaneously show the gateway session as ended,
  which gives operators false evidence that access was cut off. In incident
  response this weakens the meaning of revoke/delete and can allow a
  compromised or retired VPS process to keep interacting with the private
  control plane longer than expected.
- Evidence: Client deletion updates `gateway_sessions` to `status = 'ended'`
  and `end_reason = 'vps_deleted'` at
  `crates/api/src/repository_inventory.rs:874-889`, but no gateway-control
  disconnect is called. Current-key revocation performs the same database-only
  session end at `crates/api/src/repository_key_lifecycle.rs:738-751`.
  Gateway live sessions are held in `GatewayState.sessions` and are closed by
  sending `GatewaySessionMessage::Disconnect` at
  `crates/gateway/src/main.rs:669-690`; that helper is called for configured
  shutdown and critical forwarding failure paths at
  `crates/gateway/src/main.rs:188-204`, not by the API delete/revoke routes.
  The frontend revoke confirmation text states that active gateway sessions are
  ended at `frontend/src/panels/AccessPanel.tsx:1033`.
- Notes: Delete/revoke should use an explicit gateway control operation to
  disconnect the live session, and the API should keep DB session state aligned
  with the actual gateway result. Active target handling should still follow
  durable terminal-output rules rather than relying on the transport close
  alone.
- Fix: Delete and key-revoke routes now require request-bound DB privilege
  verification and call the gateway control plane to disconnect the affected
  live session before mutating repository state. Repository lifecycle handling
  clears the client process incarnation, ends recorded sessions, skips
  unstarted queued targets, and marks old-incarnation active targets
  `agent_lost` with synthetic output.

### AUD-115: Fleet WebSocket Streams Continue After Token Expiry, Session Revocation, Or Scope Removal

- Severity: Medium/High
- Status: Confirmed
- Area: API/WebSocket/Auth
- Context: The dashboard opens a fleet WebSocket after operator login and keeps
  it connected for live fleet snapshots and events. Admins can revoke sessions,
  disable users, reset passwords, or change scopes while another browser is
  already connected.
- Root Cause: The WebSocket handler authenticates the initial `auth` message
  once, reduces the result to a boolean, and then streams hello, snapshot, and
  broadcast events indefinitely. It does not retain the authenticated session
  ID or periodically re-check access-token expiry, `operator_sessions.revoked_at`,
  operator status, role, or `fleet:read` scope.
- Impact: A revoked, disabled, demoted, or expired operator session can keep
  receiving live fleet events until the socket disconnects for some unrelated
  reason. This weakens user/session management because revocation immediately
  stops normal HTTP API requests but not the long-lived live stream. In a
  production operations room, a removed operator or stolen browser session can
  continue watching fleet status transitions, job lifecycle events, and
  topology/inventory changes after the admin intended access to end.
- Evidence: `handle_socket` calls `authenticate_socket` once, then sends
  `Hello`, `fleet_snapshot`, and subscribed broadcast events in a loop at
  `crates/api/src/routes_ws.rs:35-84`. `authenticate_socket_token` checks the
  current access token and `fleet:read` scope only at connection time at
  `crates/api/src/routes_ws.rs:86-105`. Normal HTTP request authorization calls
  `authenticate_access_token`, which checks token expiry, `revoked_at`, and
  active operator status at `crates/api/src/state.rs:429-476` and
  `crates/api/src/repository_auth.rs:748-820`. The frontend opens one socket
  and sends the access token only on `open` at
  `frontend/src/hooks/useDashboardData.ts:193-204`.
- Notes: The WebSocket should either use short server-side leases with periodic
  auth/session revalidation, subscribe to session/operator revocation events and
  close affected sockets, or require periodic authenticated refresh messages.
  Scope changes and operator disable/delete should stop the stream just like
  they stop HTTP routes.

### AUD-116: Alert And Webhook Configuration Delete Routes Lack Backend Confirmation

- Severity: High
- Status: Confirmed
- Area: API/Integrations/Confirmation
- Context: Fleet alert policies, alert notification channels, and webhook rules
  are saved production integration configuration. They decide which events are
  turned into alerts, which delivery targets receive notifications, and which
  external webhook endpoints are called by workers.
- Root Cause: The API requires explicit confirmation for upserting alert
  policies, notification channels, and webhook rules, but the matching delete
  routes accept only the path ID plus operator write authority. There is no
  delete request body, `confirmed` flag, reviewed object fingerprint, or
  equivalent backend confirmation contract for deleting these saved records.
- Impact: Direct API callers or any CLI/frontend path that reaches these
  routes can remove production alerting or webhook delivery configuration
  without server-enforced review. A wrong ID, stale UI selection, scripted
  mistake, or compromised write-scoped operator session can delete the rule or
  channel that would page operators during an incident. Frontend prompts help
  the dashboard path, but they do not protect the API contract and can drift
  from CLI or automation behavior.
- Evidence: Alert-policy upsert validates `confirmed` at
  `crates/api/src/routes_alerts.rs:547-552`, but
  `delete_fleet_alert_policy` deletes by path ID without a confirmation request
  at `crates/api/src/routes_alerts.rs:146-158`. Notification-channel upsert
  validates `confirmed` at `crates/api/src/routes_alerts.rs:358-365`, but
  `delete_fleet_alert_notification_channel` deletes by path ID without
  confirmation at `crates/api/src/routes_alerts.rs:201-213`. Webhook-rule
  upsert validates `confirmed` at
  `crates/api/src/routes_webhook_rules.rs:156-159`, but
  `delete_webhook_rule` deletes by path ID without confirmation at
  `crates/api/src/routes_webhook_rules.rs:57-66`. The frontend has local
  deletion prompts at `frontend/src/panels/FleetWorkspace.tsx:3171-3194`,
  `frontend/src/panels/FleetWorkspace.tsx:4148-4170`, and
  `frontend/src/panels/FleetWorkspace.tsx:5041-5063`, which proves the product
  treats these deletes as review-worthy but the API does not enforce that
  invariant.
- Notes: Add typed delete request bodies for these routes with `confirmed`,
  target identity/name, and optionally an updated-at or config hash guard.
  Keep frontend prompts, but make the backend reject unconfirmed deletes so CLI
  and automation cannot bypass the review model.

### AUD-117: Alert Notification Webhooks Are Not Retried Automatically After Transient Failures

- Severity: Medium/High
- Status: Confirmed
- Area: Worker/Alerts/Reliability
- Context: Fleet alert notification channels can deliver alert payloads to
  webhook endpoints. Those endpoints are operational notification paths, so
  transient endpoint/network failure is normal in production and should not
  make the alert disappear from automatic delivery.
- Root Cause: The alert notification worker claims only `queued` rows and
  expired `in_progress` rows. When a webhook attempt fails, the worker updates
  the delivery to `failed`, increments `attempt_count`, clears the lease, and
  never schedules a next attempt. Manual processing can select `failed`
  deliveries, but the background worker does not. The alert notification
  schema also has no `next_attempt_at` or `permanently_failed` state, unlike
  webhook-rule delivery.
- Impact: A temporary receiver outage, DNS failure, network hiccup, or timeout
  can permanently stop automatic delivery of an alert notification after one
  attempt. Operators must notice the failed delivery record and manually
  reprocess it. In a 20+ VPS fleet, this can cause missed pages or external
  alerting gaps exactly when noisy or degraded infrastructure makes transient
  webhook failures more likely.
- Evidence: `crates/worker/src/alert_notifications.rs:111-124` claims only
  `status = 'queued'` or expired `in_progress` rows, and
  `crates/worker/src/alert_notifications.rs:151-181` records failures as
  `failed` without `next_attempt_at`. The table definition at
  `migrations/0003_telemetry_alerts_history.sql:203-225` includes only
  `queued`, `in_progress`, `failed`, `delivered`, and `matched_dry_run` and no
  retry scheduling column. Manual API processing can retry failed rows through
  `crates/api/src/fleet_alert_notifications.rs:80-124`, proving failed rows
  are retryable in principle but only through operator action. By contrast,
  webhook-rule delivery selects `failed` rows with `next_attempt_at <= now` at
  `crates/worker/src/webhook_rules.rs:704-709`, uses four attempts and
  backoff at `crates/worker/src/webhook_rules.rs:19-20`, and records
  `permanently_failed` after the retry budget at
  `crates/worker/src/webhook_rules.rs:742-755`.
- Notes: Give alert notification deliveries the same automatic retry model as
  webhook-rule deliveries, or document and expose a deliberately different
  retry policy. The important invariant is that transient webhook failures
  should not require manual discovery before the system retries critical alert
  notifications.

### AUD-118: Manual Delivery Processors Can Send In-Progress Webhooks Before Failing The State Update

- Severity: High
- Status: Confirmed
- Area: API/Integrations/Delivery State
- Context: Operators and automation can manually process queued or failed
  alert notification deliveries and webhook-rule deliveries. These routes
  perform outbound HTTP to configured integration endpoints and then record the
  delivery attempt.
- Root Cause: The shared process-status constants include `in_progress`, and
  both API validators accept that status. The processing functions list
  matching rows and perform outbound delivery before calling repository update
  helpers. Those helpers then reject `in_progress` rows because only `queued`
  and `failed` are retryable. The state guard is therefore too late: the
  external side effect has already happened.
- Impact: A direct API caller or integration automation can process
  `status=in_progress` while a worker is already delivering the same row. That
  can send the same alert/webhook payload twice to an external system, then
  return an API error and leave the database state owned by whichever actor
  later records its attempt. For paging, ticketing, deployment hooks, or
  incident automation, duplicate side effects are production-visible and can be
  more damaging than a clean rejection before delivery.
- Evidence: `FLEET_ALERT_NOTIFICATION_DELIVERY_PROCESS_STATUSES` includes
  `in_progress` at `crates/common/src/protocol.rs:1790-1794`, and
  `WEBHOOK_RULE_DELIVERY_PROCESS_STATUSES` includes `in_progress` at
  `crates/common/src/protocol.rs:1817-1821`. The alert process validator
  accepts those constants at `crates/api/src/routes_alerts.rs:487-492`, and
  the webhook process validator accepts them at
  `crates/api/src/routes_webhook_rules.rs:316-321`. The processors perform
  outbound delivery before the update call at
  `crates/api/src/fleet_alert_notifications.rs:108-123` and
  `crates/api/src/webhook_rules.rs:180-188`. The repository updates then
  accept only `queued` or `failed` rows at
  `crates/api/src/repository_alert_notifications.rs:570-579` and
  `crates/api/src/repository_webhook_rules.rs:614-623`.
- Notes: Reject `in_progress` at request validation for non-dry-run processing,
  or add a claim/lease transition before HTTP so manual processors use the same
  single-owner delivery semantics as workers. The fix should happen before the
  outbound HTTP call, not only at the final update.

### AUD-119: Agent Update Activation Can Replace The Binary Before Durable Heartbeat Evidence Exists

- Severity: High
- Status: Fixed
- Area: Agent/Updates/Lifecycle
- Context: Manual update-check activation, standalone update activation, and
  autonomous update activation replace the on-disk agent binary and then rely
  on a restart heartbeat marker to let the API complete the activation target
  after the new process reconnects.
- Root Cause: Activation verifies the staged binary hash, writes the new
  binary over the current executable path, then writes the activation heartbeat
  marker. These are separate filesystem side effects. If marker creation or
  rename fails after the executable replacement succeeds, the function returns
  an error and does not request restart, but the current executable path has
  already been changed. The same ordering exists without a durable fsync around
  either the binary replacement or marker replacement.
- Impact: An activation job can be reported failed even though the agent binary
  that will run on the next restart is already changed. For `restart_agent =
  true`, a marker failure prevents the normal supervised restart request; a
  later manual/service restart can boot the new binary with no matching
  activation heartbeat, leaving operators with misleading job history and no
  durable proof that the update was the source of the restart. At fleet scale,
  disk pressure, permission drift, read-only bind mistakes, or filesystem
  errors during rollout can leave some VPSs in this ambiguous updated-but-failed
  state.
- Evidence: `activate_staged_update` reads and verifies the staged artifact at
  `crates/agent/src/update_activation.rs:101-110`, then calls
  `replace_active_binary` before `write_activation_marker` at
  `crates/agent/src/update_activation.rs:128-131`. `replace_active_binary`
  writes a temporary executable and renames it over `current_exe` at
  `crates/agent/src/update_activation.rs:271-292`. Only after that does
  `write_activation_marker` create and rename the heartbeat marker at
  `crates/agent/src/update_activation.rs:226-253`. The restart request happens
  later at `crates/agent/src/update_activation.rs:132-136`, so marker failure
  returns before restart. The API treats a matching heartbeat as completion at
  `crates/api/src/repository_ingest.rs:58-115`, and treats a missing expected
  activation heartbeat by deadline as `agent_lost` at
  `crates/api/src/repository_jobs.rs:2000-2029`.
- Notes: Activation should have an explicit durable two-phase lifecycle so the
  API can distinguish `staged`, `replace_started`, `replace_committed`, and
  `restart_observed`, or otherwise make the marker and binary replacement
  recoverable as one operation. The operator-visible invariant should be that a
  failed activation did not silently change the next-start binary, and a
  changed binary has durable evidence tying it to the activation job.
- Fix: Activation now writes and fsyncs the heartbeat marker before replacing
  the active binary, then writes/fsyncs/renames the replacement binary and
  fsyncs the parent directory where practical. Rollback removes the marker
  before replacing the binary and reports marker cleanup failure instead of
  silently leaving stale completion evidence.

### AUD-120: Activation Heartbeat Completion Trusts Job ID Without Verifying The Artifact Hash

- Severity: High
- Status: Fixed
- Area: API/Agent Updates/Lifecycle
- Context: After an update activation with restart, the restarted agent sends
  an activation heartbeat marker containing the activation job ID and the
  activated binary SHA-256. The API uses that heartbeat to complete the
  `agent_update_activate` target that was running under the previous process
  incarnation.
- Root Cause: The API checks only that
  `heartbeat.activation_job_id == job_id`. It records both the heartbeat hash
  and the expected `staged_sha256_hex` in synthetic output, but it does not
  require them to match before marking the activation target completed. The
  agent rollback path also removes the activation marker only best-effort after
  replacing the binary with the rollback copy, so a stale marker can survive a
  rollback or cleanup failure.
- Impact: The API can mark an activation as completed even when the restarted
  agent reports a different binary hash from the one requested by the job. A
  stale marker left after rollback can make operators believe the update
  activation succeeded while the agent actually booted a rollback or otherwise
  different binary. In fleet update operations, this breaks the main forensic
  guarantee of the activation heartbeat: completed should mean this exact
  staged artifact was observed after restart.
- Evidence: `complete_old_incarnation_targets_in_tx` destructures
  `staged_sha256_hex` and `update_heartbeat`, but the only completion predicate
  is `heartbeat.activation_job_id == job_id` at
  `crates/api/src/repository_ingest.rs:58-66`. It writes
  `artifact_sha256_hex` from the heartbeat and `staged_sha256_hex` from the job
  into output at `crates/api/src/repository_ingest.rs:71-85`, then marks the
  target completed at `crates/api/src/repository_ingest.rs:89-115` without a
  hash equality check. The regression test only covers the matching-hash case
  at `crates/api/src/tests_postgres_reliability.rs:733-772`; there is no
  mismatch rejection test. The rollback path replaces the active binary and
  ignores activation-marker deletion failure at
  `crates/agent/src/update_activation.rs:188-190`.
- Notes: Activation heartbeat completion should require the heartbeat SHA-256
  to equal the job's expected staged SHA-256. Mismatch should terminalize or
  quarantine as explicit update-state corruption, not completion. Rollback
  should also make stale activation markers impossible or visibly failed before
  reporting rollback success.
- Fix: Activation heartbeat completion now requires both matching activation
  job ID and matching staged artifact SHA-256. The agent reports the actual
  active binary hash on hello; a job-ID match with hash mismatch appends
  synthetic integrity evidence and marks the activation target failed.

### AUD-121: Agent Trust-Root And Client Deletion Mutations Bypass Request-Bound Privilege Verification

- Severity: High
- Status: Fixed
- Area: API/Access/Privilege
- Context: Operators can import direct agent identities, rotate a client public
  key, revoke a current client key, and delete a client from fleet inventory.
  These workflows alter the gateway trust root for an agent or remove a client
  from normal fleet/job operation.
- Root Cause: These request models carry only a boolean `confirmed` flag and do
  not include a `privilege_assertion`. The routes require an operator token with
  `inventory:write`, but they do not build or verify a gateway-backed
  `DbPrivilegeIntent` before mutating the agent identity, key revocation, or
  client deletion state. Frontend and CLI callers therefore submit the same
  boolean confirmation without a request-bound assertion.
- Impact: Any bearer token/session that has `inventory:write` can register a
  direct agent identity, replace a client's public key, revoke the current key,
  or delete the client without proving possession of the super-password derived
  privilege material. The API and gateway are private services, but the current
  permission model relies on request-bound privilege assertions for high-risk
  operator actions. Bypassing that contract on trust-root mutations is a
  production security gap: a compromised or overly broad operator token can
  prepare agent impersonation on the next connection, lock out an existing
  agent by revoking its key, or remove a VPS from the visible fleet without the
  stronger approval path used by comparable job, schedule, tag, config, backup,
  and network workflows.
- Evidence: `DeleteAgentRequest` has only `confirmed` and `reason` at
  `crates/api/src/model.rs:74-79`. `UpsertAgentIdentityRequest` has
  `client_public_key_hex`, `replace_existing_key`, and `confirmed`, but no
  `privilege_assertion`, at `crates/api/src/model.rs:416-428`.
  `CreateClientKeyRevocationRequest` similarly has only `confirmed` and
  `reason` at `crates/api/src/model.rs:459-464`. The key lifecycle routes
  require `inventory:write` and call repository mutations directly at
  `crates/api/src/routes_key_lifecycle.rs:17-35` and
  `crates/api/src/routes_key_lifecycle.rs:51-79`; their validation only checks
  the boolean confirmation at `crates/api/src/routes_key_lifecycle.rs:89-130`.
  The client delete route follows the same pattern at
  `crates/api/src/routes_inventory.rs:85-104` and
  `crates/api/src/routes_inventory.rs:935-947`. The dashboard submits
  identity import/rotation and key revocation with `confirmed: true` but no
  privilege assertion at `frontend/src/panels/AccessPanel.tsx:315-322` and
  `frontend/src/panels/AccessPanel.tsx:357-360`; fleet deletion does the same
  at `frontend/src/panels/FleetWorkspace.tsx:543-551`. The CLI helpers also
  send only `confirmed` for identity upsert and key revocation at
  `crates/vpsctl/src/commands_keys.rs:32-44` and
  `crates/vpsctl/src/commands_keys.rs:70-77`.
- Notes: Add a request-bound privilege assertion to direct identity import,
  key rotation, key revocation, and client deletion. The API should recompute a
  canonical `DbPrivilegeIntent` from the actual request, including the target
  client ID and public-key hash where relevant, and verify it through the
  gateway before mutating state. Consider requiring admin role as well for
  trust-root changes, but the minimum fix is to make these mutations follow the
  same privilege-verification contract as other high-risk operator actions.
- Fix: `DeleteAgentRequest`, `UpsertAgentIdentityRequest`, and
  `CreateClientKeyRevocationRequest` now carry `privilege_assertion`. Routes
  recompute canonical DB privilege intents for identity import, key rotation,
  key revocation, and client deletion, verify through gateway control, and fail
  closed if gateway control is unavailable. Frontend, CLI, and VTY callers now
  submit request-bound assertions; VTY direct trust-root commands require
  privileged mode.

### AUD-122: Late Command Output Is Durably Accepted After The Target Is Already Terminal

- Severity: Medium/High
- Status: Fixed
- Area: API/Gateway/Job Outputs
- Context: A command can keep producing output after the API has already marked
  its target terminal through cancellation, control timeout, agent-lost
  reconciliation, or another winning final transition. This can happen during
  gateway/API flaps, reconnect/resume, delayed final output, or agent/runtime
  cleanup races.
- Root Cause: Command-output ingestion only checks that the job target exists.
  It writes the output chunk to durable job-output storage before checking
  whether the target is still active. The later target update is compare-and-set
  protected, so terminal status is not overwritten, but the normal output
  history has already been mutated and the request is ACKed as successful.
  Gateway reconnect resume can also recreate pending command state from an
  agent `CommandResume` without validating the job's current API state, so
  resumed or buffered outputs from an already-terminal target can continue to
  flow into the normal output stream.
- Impact: Operators can see a canceled, timed-out, or agent-lost target with
  later stdout/status/final output appended as if it were part of the accepted
  run. That weakens the forensic meaning of terminal states and can keep
  accumulating payload bytes for work the control plane considers finished.
  In fleet use, a stuck command, reconnecting VPS, or gateway retry can pollute
  durable job history after the business decision has already been made. The
  CAS protects terminal status, but not output storage, job-output downloads,
  comparisons, archives, or retained file-transfer/backup evidence derived
  from the output table.
- Evidence: Gateway accepts `CommandResume` by inserting or updating a local
  `PendingCommand` without consulting the API for target status or payload
  hash at `crates/gateway/src/main.rs:872-900`. Later command output is
  forwarded for any local pending command at
  `crates/gateway/src/main.rs:902-919`. The API route validates only gateway
  ID, client ID, nonnegative sequence, and matching job ID at
  `crates/api/src/routes_ingest.rs:404-415`. It then loads targets and checks
  only that a target row exists for the client at
  `crates/api/src/routes_ingest.rs:144-150`. The durable write happens before
  any active-state check at `crates/api/src/routes_ingest.rs:151-168`. Only
  after the write does non-final output attempt `mark_job_target_running`, whose
  Postgres update is guarded by `completed_at IS NULL` and active statuses at
  `crates/api/src/repository_jobs.rs:1589-1603`; final output similarly calls
  `update_job_target_result`, whose Postgres update is guarded at
  `crates/api/src/repository_jobs.rs:2448-2474`. If those updates affect zero
  rows, the already-written output remains durable and the ingest route still
  returns success at `crates/api/src/routes_ingest.rs:215-218`.
- Notes: Output ingestion should gate normal output writes on the target still
  being active and bound to the expected process incarnation/payload. If late
  output is useful for forensics, store it in an explicit quarantined
  late-output/audit path that does not feed normal job-output downloads,
  comparisons, terminalization logic, or artifact derivation. Gateway resume
  should also validate job ID, client ID, payload hash, and active target state
  with the API before accepting resumed output as normal command output.
- Fix: Gateway command-output ingest now carries and verifies the job payload
  hash. Normal output writes are guarded by an active target row lock before
  insertion; exact terminal duplicates are accepted as idempotent no-ops, while
  late new output, payload mismatch, and conflicting duplicates are rejected as
  non-retryable command-output outcomes.

### AUD-123: Process-Supervisor Inventory Exposes Job-Output-Derived Process Details With Fleet-Read Scope

- Severity: Medium/High
- Status: Confirmed
- Area: API/Process Supervisor/Auth
- Context: Process supervisor jobs can start, stop, restart, inspect, and tail
  managed processes on VPS agents. The inventory endpoint summarizes the latest
  supervisor job outputs so operators can inspect process state across the fleet.
- Root Cause: The process-supervisor inventory route is authorized with
  `fleet:read`, but the response is built by scanning `job_outputs` for
  `process_start`, `process_stop`, `process_restart`, `process_status`, and
  `process_logs` jobs. The returned model includes process names, PIDs, exit
  codes, source job IDs, source command type, stdout/stderr log paths, restart
  evidence, and cgroup resource details.
- Impact: A fleet-only reader can inspect process-supervisor operational data
  that is derived from job output records and should follow the same boundary as
  process/job payload metadata. In production this leaks managed service names,
  local log file paths, PIDs, restart history, and cgroup resource evidence to
  operators who were intended to see fleet state but not job-output-derived
  process details. It also keeps the authorization model inconsistent: direct
  job-output reads require `jobs:read`, while this derived read surface exposes a
  curated subset of the same evidence with only `fleet:read`.
- Evidence: The route is registered at
  `crates/api/src/routes.rs:359-360` and implemented with
  `SCOPE_FLEET_READ` at `crates/api/src/routes_job_history.rs:993-1005`.
  `ProcessSupervisorInventoryView` includes `source_job_id`,
  `source_command_type`, `stdout_log`, `stderr_log`, PID/exit status, restart
  fields, and cgroup fields at `crates/api/src/model.rs:248-269`. The repository
  implementation reads directly from `job_outputs` joined to `jobs` where
  `command_type` is one of the process supervisor commands at
  `crates/api/src/repository_job_outputs.rs:231-315`, then parses the job output
  JSON into inventory rows at
  `crates/api/src/repository_job_outputs.rs:1080-1165`. Frontend Jobs loading
  consumes `/api/v1/process-supervisor/inventory?limit=200` alongside other
  job-domain reads at `frontend/src/hooks/useJobsData.ts:58-66`, and vpsctl
  exposes the same endpoint at `crates/vpsctl/src/commands_process.rs:281-297`.
- Notes: Move this endpoint to `jobs:read` unless the response is reduced to
  pure fleet metadata. The current fields are useful and should probably remain,
  but their scope should match other job-output-derived read surfaces.

### AUD-124: Fleet Alert Evidence Exposes Backup Paths And Artifact IDs With Fleet-Read Scope

- Severity: Medium/High
- Status: Confirmed
- Area: API/Fleet Alerts/Auth
- Context: Fleet alert list/export is a broad operational health surface. Backup
  request paths, include-config choices, artifact IDs, and restore-adjacent
  evidence belong behind the narrower backup read boundary, because they reveal
  filesystem scope and backup artifact inventory.
- Root Cause: `list_fleet_alerts` and `export_fleet_alerts` require only
  `fleet:read`, but alert construction reads backup requests and embeds
  backup-specific fields directly into each failed/rejected backup alert's
  evidence.
- Impact: A fleet metadata reader can inspect backup source paths and artifact
  IDs through alert list/export even though the backup read routes themselves
  require `backups:read`. This undermines the scope separation used for
  operators who should see fleet health without seeing backup payload metadata.
- Evidence: `crates/api/src/routes_alerts.rs:33-67` authorizes fleet alert
  list/export with `SCOPE_FLEET_READ`. `crates/api/src/fleet_alerts.rs:233-278`
  builds fleet alerts by loading backup requests via
  `repo.list_backup_requests(200)`. `crates/api/src/fleet_alerts.rs:590-609`
  includes `backup.paths`, `backup.include_config`, and `backup.artifact_id` in
  the serialized alert evidence for failed or rejected backup requests.
- Notes: Keep ordinary fleet health alerts readable with `fleet:read`, but
  either redact backup payload metadata from fleet-scope responses or require
  `backups:read` for alert views/exports that include backup evidence.

### AUD-125: Fleet Alert Read Routes Can Enqueue Webhook Integration Events

- Severity: High
- Status: Confirmed
- Area: API/Fleet Alerts/Webhooks
- Context: Listing or exporting fleet alerts is a read-only operator workflow.
  Webhook event creation is an integration side effect that can later produce
  outbound HTTP deliveries.
- Root Cause: `AppState::list_fleet_alerts` builds current alerts and then calls
  `record_fleet_alert_webhook_events` before applying the caller's alert filters.
  That helper inserts `webhook_events` rows for open alerts. The API routes that
  call it require only `fleet:read`, not an integration write or delivery
  authority.
- Impact: A read-only fleet operator, dashboard refresh, or scripted alert
  export can create new webhook events and wake the webhook worker. The insert is
  idempotent per alert ID, but the first read still becomes the act that
  materializes integration work, and because recording happens before filters a
  narrow alert query can enqueue events for unrelated open alerts. In production
  this makes outbound integration delivery depend on who views the dashboard and
  gives a fleet reader a write-side effect in the integrations subsystem.
- Evidence: `crates/api/src/routes_alerts.rs:33-67` exposes alert list/export as
  `fleet:read` operations. `crates/api/src/fleet_alerts.rs:233-278` calls
  `record_fleet_alert_webhook_events(&alerts)` before `apply_alert_filters`.
  `crates/api/src/fleet_alerts.rs:280-306` records `alert.open` events through
  `repo.record_webhook_event`. `crates/api/src/repository_webhook_rules.rs:392-460`
  inserts a `webhook_events` row and sends a `pg_notify` for new events.
  `crates/worker/src/webhook_rules.rs:408-454` consumes unprocessed
  `webhook_events` and creates webhook-rule deliveries.
- Notes: Alert reads should be side-effect free. Alert-open event recording
  should move to a worker or state-transition path, or be explicitly authorized
  as integration work and run after the filtered target set is known.

### AUD-126: Data-Source Read Paths Persist Default Assignments For All Clients, Including Hidden Clients

- Severity: Medium/High
- Status: Confirmed
- Area: API/Data Sources/State
- Context: Data-source presets and assignments define generated agent hot-config
  behavior. Listing assignments or status should be a read-only inspection of
  current config state, and hidden/deleted clients should not keep influencing
  normal visible-fleet config counts.
- Root Cause: `list_data_source_assignments` lazily calls
  `ensure_default_data_source_assignments`, which inserts default assignment rows
  into `client_data_source_preset_assignments`. The Postgres insert selects from
  all `clients` rows and does not filter `hidden_at IS NULL`, so hidden,
  deleted, or revoked clients can receive or retain default assignment rows.
  These writes happen from read paths and have no operator audit record.
- Impact: A config-read or fleet-read status request can mutate durable
  data-source assignment state. Hidden clients can inflate
  `assigned_client_count` and affected-client counts used in preset update
  reviews, making operators review misleading production impact and leaving
  config state for clients no longer visible in normal fleet views. This is
  practical in long-running fleets where identities are deleted/revoked but
  retained as hidden rows for audit.
- Evidence: The canonical schema keeps hidden clients as rows using
  `clients.hidden_at` and `clients.status IN (..., 'revoked', 'deleted')` at
  `migrations/0001_identity_access.sql:37-65`. Data-source assignments reference
  clients at `migrations/0007_data_sources_file_transfer.sql:36-45`. The
  migration seeds default assignments from all clients without a hidden filter at
  `migrations/0007_data_sources_file_transfer.sql:313-318`. The runtime read
  path calls `ensure_default_data_source_assignments` from
  `Repository::list_data_source_assignments` at
  `crates/api/src/repository_data_source_presets.rs:500-505`, and the helper
  performs the same unfiltered `INSERT ... SELECT c.id ... FROM clients c` at
  `crates/api/src/repository_data_source_presets.rs:748-791`.
  `list_data_source_presets` reports `assigned_client_count` from assignment
  rows at `crates/api/src/repository_data_source_presets.rs:23-45`.
- Notes: Default assignment materialization should either be explicit
  write-time/bootstrap work or be computed without mutating durable state.
  Hidden/deleted clients should be excluded from visible-fleet assignment counts
  unless an audit/history view intentionally asks for them.

### AUD-127: Controlled Gateway Shutdown Can Lose Queued RAM Forwarder Events

- Severity: High
- Status: Confirmed
- Area: Gateway/Forwarder/Shutdown
- Context: Gateway controlled restart is expected to preserve pending
  gateway-to-API forwarder events with a bounded wait. This matters for
  production deploys and restarts because the gateway forwards command output,
  terminal output, lifecycle events, and telemetry to the private API.
- Root Cause: Controlled shutdown requests forwarder shutdown and waits only
  until `current_queue_depth` reaches zero or the timeout expires. The code does
  not snapshot every already queued in-memory `mpsc` item before exiting.
  Instead, a worker spools only the single event it is actively processing when
  `spool.shutdown_requested()` is observed. Queue items that remain behind that
  active item when the bounded wait expires stay only in RAM and are lost when
  the process exits.
- Impact: A normal controlled restart during API slowness or outage can still
  drop command-output, lifecycle, or terminal-output events that were accepted
  into the gateway forwarder but had not yet reached the per-target worker's
  active processing slot. For command output, this can make the API rely on
  timeout or stale state instead of the real final agent result. For lifecycle
  events, it can leave the API with misleading gateway/session state. This is
  practical in 20+ VPS operation during deploys, API maintenance, or transient
  database/API stalls.
- Evidence: Gateway shutdown calls `request_all_agent_disconnects`, then
  `shutdown_api_client.shutdown_flush(shutdown_flush)` at
  `crates/gateway/src/main.rs:200-206`. `shutdown_flush` only sets the shutdown
  flag and polls `current_queue_depth` until a deadline at
  `crates/gateway/src/api_client.rs:562-570`. Events are placed into per-target
  `mpsc` queues by `enqueue_queue_item` at
  `crates/gateway/src/api_client.rs:702-756`. The worker loop processes one
  queue item at a time at `crates/gateway/src/api_client.rs:1326-1381`; only
  when that item returns `DeferredForShutdown` does it write the current event
  to disk at `crates/gateway/src/api_client.rs:1392-1414`. There is no path that
  drains or snapshots the remaining queued `GatewayForwardQueueItem::Event`
  values before the process exits.
- Notes: Controlled shutdown should stop accepting new work, disconnect agents,
  then explicitly drain/snapshot every pending and in-flight forwarder event to
  the gateway spool using the existing atomic file format before exit. The
  hard-crash window can remain documented out of scope; this issue is about the
  controlled restart promise.

### AUD-128: Recursive File Delete Can Escape Through Symlink-Swap Races

- Severity: High
- Status: Fixed
- Area: Agent/File Browser/Safety
- Context: File delete is exposed as a destructive single-VPS and bulk
  operator workflow. When recursive delete is enabled, operators expect the
  reviewed path tree to be removed, not an attacker-controlled target outside
  that tree.
- Root Cause: The recursive delete implementation classifies each path with
  `symlink_metadata`, then later calls `read_dir`, recurses, and removes by
  pathname. Those later operations are not anchored to an open directory handle
  and do not use no-follow `openat`/`unlinkat` semantics. If a directory entry
  is replaced with a symlink after it was classified as a directory but before
  `read_dir` or nested removal runs, the walk can follow the new symlink target
  and delete outside the reviewed subtree.
- Impact: On a VPS where the agent runs with elevated privileges and the
  selected deletion tree contains writable directories, a local user or
  compromised process can race a recursive delete job into removing files
  outside the selected path. This is a practical production safety issue for
  cleanup operations under application upload/cache directories, shared
  workspaces, or user-writable trees.
- Evidence: `execute_file_delete` validates the requested path and then calls
  `remove_path(target, recursive, ...)` at
  `crates/agent/src/file_browser.rs:594-615`. `remove_path_blocking` first uses
  `std::fs::symlink_metadata` and treats a non-symlink directory as safe to
  recurse at `crates/agent/src/file_browser.rs:1020-1038`.
  `remove_dir_contents_checked` reads and stores child pathnames, then for each
  child repeats `symlink_metadata` and calls `read_dir`, `remove_dir`, or
  `remove_file` by path at `crates/agent/src/file_browser.rs:1043-1070`.
  Recursive delete is a first-class command payload through
  `JobCommand::FileDelete { path, recursive, policy }` at
  `crates/common/src/protocol.rs:2492-2498` and is reachable from the frontend
  file panels at `frontend/src/panels/jobs/FileBrowserPanel.tsx:417-419` and
  `frontend/src/panels/jobs/MultiFileActionsPanel.tsx:207`.
- Notes: Recursive deletion should either reject deleting trees containing
  writable race points when running privileged, or use descriptor-based traversal
  with `openat`/`unlinkat`, no-follow directory opens, and directory identity
  rechecks before descending. A symlink encountered at any point should be
  unlinked as the symlink itself or rejected according to an explicit operator
  policy; it should never redirect recursive deletion.
- Resolution: Fixed by moving recursive delete to descriptor-anchored
  traversal with no-follow directory opens and fd-relative child removal. Parent
  components are resolved as real directories before mutation.

### AUD-129: Terminal Output Forwarding Bypasses The Gateway RAM Spool Budget

- Severity: Medium/High
- Status: Confirmed
- Area: Gateway/Terminal/Resource Bounds
- Context: Gateway spool settings expose a RAM budget intended to keep the
  forwarder bounded while the API is slow or unavailable. Terminal output is a
  normal operator workflow and can produce sustained payload streams from
  interactive sessions.
- Root Cause: The forwarder applies `spool.try_reserve_ram` and disk-spool
  fallback only when `event.kind == CommandOutput`. All other non-telemetry
  events, including terminal output, call `reserve_ram_unchecked` and stay in
  RAM. Queue capacity is count-based, not byte-based, so the configured
  `spool_ram_max_bytes` does not bound terminal-output memory.
- Impact: During API outage or slow ingestion, active terminal sessions can
  enqueue thousands of terminal-output events in gateway memory, ignoring the
  configured RAM spool ceiling. This can create large memory spikes on the
  gateway and eventually drop terminal data once count limits or event TTLs are
  hit. In 20+ VPS operation, several high-output terminal sessions or log
  streams can make gateway resource use unpredictable exactly when API
  instability is already present.
- Evidence: Generic `post` serializes terminal output into a `GatewayForwardEvent`
  using the path-derived kind at `crates/gateway/src/api_client.rs:120-144`.
  Terminal stream frames are forwarded through
  `/internal/v1/gateway/terminal-output` at
  `crates/gateway/src/main.rs:935-950`, and that path maps to
  `GatewayForwardEventKind::TerminalOutput` at
  `crates/gateway/src/api_client.rs:1702-1711`. The queue preparation only
  spills command output when the RAM reservation fails at
  `crates/gateway/src/api_client.rs:675-688`; terminal output follows the
  `reserve_ram_unchecked` branch at
  `crates/gateway/src/api_client.rs:689-699`. The queue limits are
  `PER_TARGET_QUEUE_CAPACITY = 512` and `GLOBAL_QUEUE_CAPACITY = 10_000` at
  `crates/gateway/src/api_client.rs:429-430`, while normal agent terminal
  output can batch up to `TERMINAL_READ_CHUNK_BYTES * 2` bytes before encoding
  at `crates/agent/src/terminal.rs:31-32` and
  `crates/agent/src/terminal.rs:879-905`.
- Notes: Terminal output should either participate in the same byte-budget and
  spool policy as command output, or have an explicit smaller byte-based queue
  limit with visible dropped-output counters. The existing noncritical TTL is
  not a substitute for bounding memory while the event is queued.

### AUD-130: Copy, Chmod, And Chown Can Follow Symlinks After Validation Races

- Severity: High
- Status: Fixed
- Area: Agent/File Browser/Safety
- Context: File copy, chmod, and chown are exposed as single-VPS and bulk
  operator workflows. Operators can run them with elevated agent privileges
  against paths that include application-owned or user-writable directories.
- Root Cause: The agent validates operands with `symlink_metadata` and rejects
  literal symlinks in the default path, but later performs the actual operation
  by pathname. The final calls can follow a symlink that appears after
  validation: `std::fs::set_permissions` follows symlinks, Unix `chown` follows
  symlinks, and `File::open` follows symlinks when copying a source file.
  Recursive traversal also reads child pathnames and reuses pathname operations
  rather than descriptor-anchored no-follow operations.
- Impact: A local user or compromised process with write access under the
  selected tree can race an operator's privileged file job into chmod/chowning a
  file outside the reviewed target, or copying bytes from a substituted
  symlink target. This can weaken permissions on sensitive files, change their
  ownership, or leak/copy data into operator-selected destinations. The issue is
  practical in production cleanup, deployment, and repair workflows under
  writable application directories.
- Evidence: Chmod validation rejects an initial symlink at
  `crates/agent/src/file_browser.rs:637-649`, but the worker repeats
  `symlink_metadata` and then calls `std::fs::set_permissions(path, ...)` at
  `crates/agent/src/file_browser.rs:1073-1088`; recursive traversal uses
  pathname `read_dir` and recursive calls at
  `crates/agent/src/file_browser.rs:1097-1114`. Chown calls
  `chown_path_recursive` after only checking existence at
  `crates/agent/src/file_browser.rs:692-729`; the recursive helper checks
  `symlink_metadata` and then calls `platform_chown_path` at
  `crates/agent/src/file_browser.rs:1224-1245`, which uses Unix
  `libc::chown` at `crates/agent/src/platform_accounts.rs:239-252`. Copy
  rejects initial source symlinks unless `follow_symlinks` is set at
  `crates/agent/src/file_browser.rs:765-784`, repeats the source check at
  `crates/agent/src/file_browser.rs:1293-1304`, and then opens the source by
  pathname at `crates/agent/src/file_browser.rs:1433-1447`.
  The affected command payloads are `FileChmod`, `FileChown`, and `FileCopy` at
  `crates/common/src/protocol.rs:2499-2537`, and they are reachable from the
  frontend file panels at `frontend/src/panels/jobs/FileBrowserPanel.tsx:432-459`
  and `frontend/src/panels/jobs/MultiFileActionsPanel.tsx:187-201`.
- Notes: These operations need the same race-safe model as recursive delete:
  descriptor-anchored traversal, no-follow opens where the default policy says
  not to follow symlinks, and identity rechecks before mutating or reading a
  path. Explicit `follow_symlinks` should be reflected in audit/status output
  and should not be the accidental result of a race.
- Resolution: Fixed by routing copy, chmod, and chown through the shared
  no-follow parent resolver, fd-relative traversal, opened-descriptor
  mutation, and source identity checks. Explicit final symlink following remains
  opt-in where already supported.

### AUD-131: Read And Download Paths Can Dereference Symlinks After Validation

- Severity: High
- Status: Confirmed
- Area: Agent/File Read And Download/Safety
- Context: Operators can read text files, download regular files, and start
  resumable file-transfer downloads from VPS paths. These workflows are often
  used against application-owned directories where a local app user or
  compromised process can create or swap files.
- Root Cause: Direct text read and direct file download validate a pathname
  with `symlink_metadata` and reject a literal symlink by default, but they
  later open the same pathname with normal `File::open` / `tokio::fs::File::open`,
  which follows symlinks. A symlink substituted after validation can therefore
  supply bytes from a different target. Resumable file-transfer download start
  is looser: it uses `tokio::fs::metadata`, which follows symlinks, and the
  later chunk reads reopen the stored pathname by name, so that workflow has no
  explicit no-follow default or stable file identity.
- Impact: A local user or compromised process with write access under a
  reviewed directory can race a privileged operator read/download job into
  returning bytes from another readable file on the VPS. For resumable
  downloads, a file can also be replaced between start and chunk reads, so the
  stored `size_bytes` and `sha256_hex` can describe one file while chunks are
  read from a later pathname target. This can leak sensitive files, produce
  misleading job evidence, and break operator trust in file-download audit
  results during production triage or bulk file collection.
- Evidence: Text read validates with `path_metadata_for_follow` at
  `crates/agent/src/file_browser.rs:326-339`, then opens the pathname in
  `read_file_bounded` at `crates/agent/src/file_browser.rs:372-390`. Direct
  file download validates with `path_metadata_for_follow` at
  `crates/agent/src/file_download.rs:126-130`, then reads via
  `read_file_bounded` or `stream_file_payload`, both of which open the pathname
  at `crates/agent/src/file_download.rs:172-195` and
  `crates/agent/src/file_download.rs:309-358`. Resumable file-transfer
  download start follows symlinks with `tokio::fs::metadata` at
  `crates/agent/src/file_download.rs:767-781`, hashes by opening the pathname
  at `crates/agent/src/file_download.rs:782-788`, stores the pathname in temp
  session metadata at `crates/agent/src/file_download.rs:803-813`, and later
  chunk reads reopen `metadata.path` at
  `crates/agent/src/file_download.rs:841-849` and
  `crates/agent/src/file_download.rs:921-929`. The affected command payloads
  are `FileReadText`, `FileDownload`, and `FileTransferDownloadStart` at
  `crates/common/src/protocol.rs:2431-2543`.
- Notes: The fixed model should open the reviewed file with no-follow semantics
  by default, retain a stable descriptor or file identity for the operation,
  and re-check identity before each chunk in resumable downloads if descriptors
  cannot be retained across jobs. Literal symlink following should require an
  explicit operator choice and must be represented in status/audit output.
- Resolution: Fixed by adding a shared agent no-follow regular-file opener for
  read/hash/chunk paths, using it for text reads, direct file downloads, file
  pull, and resumable download start/chunk reads. Resumable downloads now store
  the started file identity and reject chunks if the source path changes.
  `file_pull` and resumable download payloads now require explicit
  `follow_symlinks`, default false in CLI/VTY/frontend, with the choice shown in
  review/status surfaces. Regression tests cover default symlink rejection,
  explicit follow opt-in, and resumable source replacement rejection.

### AUD-132: Precompleted Skipped Targets Are Not Atomic With Job Creation

- Severity: High
- Status: Fixed
- Area: API/Jobs/State Machine
- Context: Job creation now pre-completes targets that should not dispatch,
  including never-connected targets, capability-degraded targets, and busy
  update targets. Operators expect those targets to appear immediately as
  skipped, not as ordinary queued work.
- Root Cause: The API first commits the job and every target as `queued`, then
  runs the skip-output and target-terminalization helpers afterward in separate
  durability steps. There is no repair path that finds queued targets that were
  supposed to be precompleted but were left behind after an API crash, deploy,
  database error, or side-effect error between those steps.
- Impact: A never-connected target can again remain queued forever if the API
  stops after creating the job but before `precomplete_never_connected_skips`
  runs. A busy update target can dispatch later after the busy condition clears
  instead of remaining skipped for that run, which violates the update-family
  policy operators reviewed. Capability-degraded targets can also be left as
  ordinary queued targets rather than visible skipped results. This matters for
  20+ VPS operation because bulk jobs and scheduled runs commonly include some
  unavailable, degraded, or busy clients, and those states should not depend on
  a post-commit best-effort cleanup.
- Evidence: `create_job_inner` computes `never_connected_skips`,
  `capability_skips`, and `busy_update_skips`, but then calls
  `record_dispatching_job` / `record_dispatching_job_from_schedule` first at
  `crates/api/src/routes_jobs.rs:337-362`. The Postgres insert path writes all
  targets as `queued` at `crates/api/src/repository_jobs.rs:1157-1213`.
  Only after that transaction commits does `create_job_inner` call
  `precomplete_never_connected_skips`, `precomplete_capability_skips`, and
  `precomplete_busy_update_skips` at `crates/api/src/routes_jobs.rs:363-365`.
  Each precomplete helper writes the synthetic output and then calls
  `update_job_target_result` in separate repository calls, for example
  `precomplete_never_connected_skips` at
  `crates/api/src/routes_jobs.rs:616-649` and
  `precomplete_busy_update_skips` at
  `crates/api/src/routes_jobs.rs:652-686`.
- Notes: The clean fix should materialize skipped targets and their final
  evidence in the same transaction that creates the job, or add an idempotent
  reconciler with durable skip intent that repairs queued precomplete targets
  before dispatch can claim them. Busy-update skip intent must be frozen at job
  creation time; it should not be re-evaluated later after active work changes.
- Resolution: Fixed by passing frozen precompleted target outcomes into job
  creation and inserting skipped target state, synthetic final output, and
  target-result audit evidence inside the same memory/Postgres job-creation
  operation. The create-job route no longer runs post-commit precompletion
  helpers, and focused tests assert skipped targets are terminal, have done
  output, and are not dispatch-claimable immediately after creation.

### AUD-133: Upload Staging Pathnames Can Be Swapped Into Symlinks Before Chmod, Chown, Chunk Writes, Or Commit

- Severity: High
- Status: Fixed
- Area: Agent/File Upload/Safety
- Context: Operators can push inline/chunked files and resumable file-transfer
  uploads into arbitrary VPS paths. These workflows are commonly used for
  service configs, deployment artifacts, scripts, restore material, and other
  privileged writes into directories that may be writable by an application
  user or colocated workload.
- Root Cause: The agent creates upload staging files as visible pathnames under
  the destination parent, then later reopens or mutates those pathnames. The
  normal path operations follow symlinks and do not hold a stable file
  descriptor or re-check file identity before chmod, chown, chunk writes, hash,
  or commit. A local process with write access to the destination directory can
  replace the staging pathname after creation, causing later agent operations
  to act on a different file. The resumable metadata file is also written and
  reread by predictable pathname under the system temp directory rather than a
  protected per-agent state directory.
- Impact: A local user or compromised process under a writable target directory
  can race a privileged upload into chmod/chowning an unintended file, writing
  uploaded chunk bytes through a substituted symlink, or making the final job
  evidence refer to a file different from the one the operator reviewed. For
  example, an app user that can modify the destination directory can replace
  `.vpsman-upload-*` or `.vpsman-transfer-*.part` between creation and the
  agent's later chmod/chown/write/open calls. This is production-relevant
  because operators often upload into application-owned directories while the
  agent runs with higher privilege.
- Evidence: Non-resumable file push builds a destination-adjacent
  `.vpsman-upload-*` path at `crates/agent/src/file_push.rs:380-387`, writes it
  with `tokio::fs::write`, then calls `tokio::fs::set_permissions` and
  optional ownership changes on the pathname at
  `crates/agent/src/file_push.rs:410-418` before renaming it at
  `crates/agent/src/file_push.rs:420-465`. The ownership helper uses Unix
  `libc::chown`, which follows symlinks, at
  `crates/agent/src/platform_accounts.rs:239-252`. Resumable upload creates
  `.vpsman-transfer-<name>-<session>.part` with `tokio::fs::File::create` at
  `crates/agent/src/file_push.rs:165-198`, writes session metadata under
  `std::env::temp_dir()` at `crates/agent/src/file_push.rs:765-810`, later
  stats and opens the temp path with normal following operations at
  `crates/agent/src/file_push.rs:848-885`, and commits by hashing,
  `set_permissions`, and renaming the same pathname at
  `crates/agent/src/file_push.rs:279-318`.
- Notes: This is distinct from AUD-083. AUD-083 covers staging-file readability
  before final mode is applied; this issue covers path-substitution safety and
  wrong-file mutation/write hazards. Upload staging should use owner-only
  create-new files, descriptor-anchored chmod/chown/hash/write where possible,
  no-follow opens and identity checks before each phase, and protected
  per-agent metadata storage. Writable destination directories need the same
  race-safe treatment as other privileged filesystem operations.
- Resolution: Fixed by creating upload staging files with owner-only no-follow
  create-new opens, applying final mode and ownership through the open
  descriptor, identity-checking resumable temp files, and moving resumable
  metadata to a private agent-owned directory.

### AUD-134: Restore Staging Pathnames Can Be Precreated Or Swapped Into Symlinks

- Severity: High
- Status: Fixed
- Area: Agent/Restore/Safety
- Context: Restore and restore-rollback jobs write recovered file contents or
  rollback snapshots into operator-selected VPS paths. These paths can be live
  application directories, restore rehearsal roots, or incident-recovery
  staging trees where an application user or colocated process may be able to
  create files in the destination parent.
- Root Cause: Restore staging uses predictable destination-adjacent pathnames
  such as `.vpsman-restore-<file>-<job>.tmp`,
  `.vpsman-restore-<file>-<job>.bak`, and
  `.vpsman-restore-rollback-<file>-<job>.tmp`. The agent then writes, copies,
  chmods, and renames those paths with ordinary pathname operations. These
  operations do not use create-new no-follow opens, stable file descriptors, or
  identity checks, so a local process can precreate the staging path as a
  symlink or swap it between write/copy and chmod/rename phases.
- Impact: A writable destination parent can turn a privileged restore into an
  unintended write or mode change on a symlink target outside the reviewed
  restore scope. For example, if an app user predicts the restore staging name
  or races it after creation, `tokio::fs::write`, `tokio::fs::copy`, or
  `tokio::fs::set_permissions` can follow the symlink and overwrite or chmod a
  different file. This is stronger than temporary readability: it can corrupt
  live files or weaken permissions on files the operator did not intend to
  restore.
- Evidence: `write_restored_file` creates rollback and restore staging paths
  from the destination filename and job ID at `crates/agent/src/restore.rs:601-622`,
  copies an existing destination to `.bak` with `tokio::fs::copy` at
  `crates/agent/src/restore.rs:606-608`, writes restored bytes to `.tmp` with
  `tokio::fs::write` at `crates/agent/src/restore.rs:621-624`, then chmods
  that path and renames it at `crates/agent/src/restore.rs:625-628`.
  Failed-restore rollback copies a snapshot back to the live destination and
  chmods the destination by pathname at `crates/agent/src/restore.rs:649-670`.
  Explicit restore rollback builds `.vpsman-restore-rollback-*` under the
  destination parent, copies the rollback snapshot to it, chmods it, and
  renames it at `crates/agent/src/restore_rollback.rs:174-205`.
- Notes: This is distinct from AUD-086 and AUD-087. AUD-086 covers default
  readability of restore staging files before archive modes are applied, and
  AUD-087 covers symlinked parent components escaping a restore root. This
  issue covers staging-path substitution itself. Restore staging should use
  owner-only create-new no-follow files, descriptor-based permission changes,
  and identity checks before final rename. Rollback snapshots should use the
  same race-safe staging model.
- Resolution: Fixed by creating restore and rollback staging files with
  no-follow create-new opens, keeping descriptors through chmod/copy/sync, and
  committing with fd-relative rename.

### AUD-135: Text-Write And Copy Staging Pathnames Can Be Swapped Before Chmod Or Commit

- Severity: High
- Status: Fixed
- Area: Agent/File Browser/Safety
- Context: Operators can edit text files and copy files through the frontend,
  CLI, or bulk job dispatch. These workflows are used for production
  configuration, scripts, and service-owned trees, often with the agent running
  at higher privilege than the directory owner.
- Root Cause: The agent stages text writes and copied files under the
  destination parent using visible `.vpsman-edit-*` and `.vpsman-copy-*`
  pathnames, then performs chmod and final rename by pathname. The text-write
  path writes and closes the temp file before chmod and rename. The copy path
  uses `create_new`, but later chmods the temp pathname rather than the open
  file descriptor. Neither path re-checks that the staging pathname still
  refers to the file the agent created.
- Impact: A local process with write access to the destination directory can
  watch for the staging pathname, unlink it, and replace it with a symlink
  before chmod or final rename. The agent can then chmod the symlink target and
  rename the symlink into the reviewed destination, replacing a regular file
  with a link to an unintended path. This is practical for application-owned
  deployment/config directories and can corrupt files, weaken permissions, or
  make job evidence claim a reviewed edit/copy succeeded while the committed
  filesystem object is not the staged payload.
- Evidence: `execute_file_write_text` calls `atomic_write` at
  `crates/agent/src/file_browser.rs:397-496`. `atomic_write` builds a
  destination-adjacent `.vpsman-edit-*` temp path, writes it with
  `tokio::fs::write`, then calls `tokio::fs::set_permissions` and renames the
  same pathname at `crates/agent/src/file_browser.rs:937-967`. File copy builds
  a `.vpsman-copy-*` temp path at `crates/agent/src/file_browser.rs:1384-1409`,
  opens it with `OpenOptions::create_new` at
  `crates/agent/src/file_browser.rs:1441-1447`, then chmods the temp pathname
  and renames it at `crates/agent/src/file_browser.rs:1470-1478` and
  `crates/agent/src/file_browser.rs:1415-1428`.
- Notes: This is distinct from AUD-089, which covers temporary readability of
  text-edit staging, and from AUD-130, which covers source/chmod/chown
  symlink races after operand validation. This issue is specifically about the
  staging pathname itself being substituted before chmod or commit. The fix
  should apply descriptor-based permissions, no-follow/identity checks, and
  final commit validation for text-write and copy staging files.
- Resolution: Fixed by creating text-write and copy staging files owner-only
  with no-follow create-new opens, applying permissions through descriptors,
  syncing, and committing with fd-relative rename.

### AUD-136: Directory Creation Can Chmod A Swapped Symlink Target After Mkdir

- Severity: Medium/High
- Status: Fixed
- Area: Agent/File Browser/Safety
- Context: Operators can create directories from the file browser, CLI, and
  bulk file workflows to prepare deployment paths, restore roots, service data
  directories, and application-owned trees. The agent may run with elevated
  filesystem privileges while the destination parent is writable by an
  application user or colocated workload.
- Root Cause: `file_mkdir` creates the directory by pathname and then applies
  the requested mode by calling `tokio::fs::set_permissions` on the same
  pathname. It does not hold or re-open the created directory with no-follow
  semantics, and it does not verify that the path still refers to the directory
  it just created. The recursive path also uses normal `create_dir_all`, which
  follows existing symlinked parent components.
- Impact: A local process with write access to the destination parent can race
  after the agent creates the directory, remove the empty directory, replace it
  with a symlink, and make the privileged agent chmod the symlink target. This
  can weaken permissions on files or directories outside the reviewed mkdir
  target. Recursive mkdir can also create reviewed child directories under a
  symlinked parent, making the actual filesystem mutation differ from what the
  operator expected when preparing restore or deployment trees.
- Evidence: `execute_file_mkdir` validates only `validate_browser_path`, not
  `validate_mutable_path`, at `crates/agent/src/file_browser.rs:511-519`.
  It checks existence with `tokio::fs::metadata`, creates the directory with
  `tokio::fs::create_dir_all` or `tokio::fs::create_dir`, then calls
  `tokio::fs::set_permissions(target, ...)` by pathname at
  `crates/agent/src/file_browser.rs:520-541`. The command is exposed as
  `JobCommand::FileMkdir` at `crates/common/src/protocol.rs:2476-2483`.
- Notes: This is distinct from AUD-130, which covers explicit chmod/chown/copy
  operations after validation. The mkdir workflow performs its own chmod after
  creation and needs the same descriptor-anchored or no-follow identity model.
  Recursive mkdir should either reject symlinked parent components by default
  or make symlink following an explicit, audited operator choice.
- Resolution: Fixed by resolving mkdir parents with no-follow directory
  traversal, creating directories with `mkdirat`, and applying modes through
  the opened directory descriptor.

### AUD-137: Command-Template Delete Route Lacks Backend Confirmation

- Severity: Medium/High
- Status: Confirmed
- Area: API/Command Templates/Confirmation
- Context: Command templates are durable reusable job definitions. Deleting a
  user-defined template removes saved operation payloads and defaults that
  operators may rely on for repeatable dispatch, updates, file operations,
  process supervision, backups, restores, or config jobs.
- Root Cause: The API upsert route requires `confirmed`, but the delete route
  accepts only a template ID and `jobs:write`. The frontend has a local
  deletion prompt, but the backend contract does not require a reviewed delete
  request body, template name, or equivalent target fingerprint.
- Impact: A direct API caller or future CLI/automation path with write access
  can delete the wrong reusable command template without server-enforced
  review. This is practical in production because templates are shared
  operator configuration: deleting the wrong entry can break runbooks or cause
  operators to recreate privileged job payloads under pressure.
- Evidence: `upsert_command_template` rejects unconfirmed requests in
  `crates/api/src/routes_command_templates.rs:45-55`, while
  `delete_command_template` deletes by path ID without a confirmation request
  at `crates/api/src/routes_command_templates.rs:73-88`.
  `Repository::delete_command_template` performs the delete and only then
  records `command_template.deleted` audit at
  `crates/api/src/repository_command_templates.rs:267-316`. The dashboard
  prompt exists at `frontend/src/panels/JobDispatchPanel.tsx:1206-1223`,
  proving the product treats deletion as review-worthy.
- Notes: This is separate from AUD-045, which covers unreviewed template saves,
  and AUD-059, which covers the write-scope boundary. The clean fix should add
  a typed delete request with `confirmed` and reviewed template identity.

### AUD-138: Data-Source Preset Updates Can Bypass Confirmation For One Assigned VPS

- Severity: Medium/High
- Status: Confirmed
- Area: API/CLI/Data Sources/Confirmation
- Context: Data-source presets generate agent hot-config sections for
  telemetry, process inventory, execution policy, tunnel adapters, backup and
  restore behavior, and autonomous updater settings. Updating an assigned
  preset changes future rendered config for the VPSs that inherit it.
- Root Cause: The repository requires confirmation only when a changed preset
  has `affected_client_count > 1`. A preset assigned to exactly one production
  VPS is updated immediately even when `confirmed = false`. The CLI and VTY
  expose `data-source-preset-update` with an optional `--confirmed` flag, but
  they do not require it before sending the update request.
- Impact: A CLI or direct API operation can change a reusable config source for
  one production VPS without the same explicit review used by the frontend and
  by other config-affecting operations. This can silently change the next
  rendered hot-config patch for backup, update, telemetry, process, or network
  runtime behavior on that VPS.
- Evidence: `UpdateDataSourcePresetRequest` has a default-false `confirmed`
  field in `crates/api/src/model_data_sources.rs:67-73`.
  `Repository::update_data_source_preset` only returns
  `confirmation_required` when `diff.affected_client_count > 1 &&
  !request.confirmed` at
  `crates/api/src/repository_data_source_presets.rs:414-435`; otherwise it
  proceeds to update the preset at
  `crates/api/src/repository_data_source_presets.rs:437-491`. The CLI forwards
  `options.confirmed` without an `ensure!` at
  `crates/vpsctl/src/commands_inventory.rs:699-720`, and VTY parsing accepts
  the update without requiring `--confirmed` at
  `crates/vpsctl/src/vty_inventory.rs:1105-1114`.
- Notes: This is not the same as AUD-052's scope issue or AUD-053's create-path
  upsert issue. The fix should make all changed preset updates require
  confirmation regardless of affected-client count, while unchanged updates can
  remain idempotent no-ops.

### AUD-139: CLI Tag Create And Single-VPS Assignment Auto-Confirm Tag Mutations

- Severity: Medium/High
- Status: Confirmed
- Area: CLI/VTY/Fleet Tags
- Context: Fleet tags drive selectors, saved schedules, bulk job targeting,
  dashboard grouping, and future operator workflows. Tag mutations can change
  which VPSs a saved selector or recurring job affects.
- Root Cause: The backend supports an unconfirmed preview path for tag
  mutations that returns affected VPSs and schedule impact notices, but the
  normal `vpsctl` and VTY commands for tag creation and single-VPS tag
  assignment build a privilege assertion with `confirmed = true` and send
  `confirmed: true` directly. These commands have no `--confirmed` argument
  and no preview step.
- Impact: Operators using CLI/VTY can create tags or assign a tag to a VPS
  without seeing the affected target/schedule-impact review that the product
  exposes elsewhere. On a long-running 20+ VPS fleet, a single wrong tag can
  change future schedule materialization or bulk dispatch membership.
- Evidence: `assign_agent_tag_mutation` returns a preview with
  `schedule_impacts` when `confirmed` is false at
  `crates/api/src/repository_inventory.rs:627-653`. The CLI `tag_create` and
  `agent_tag` commands hard-code `confirmed: true` and build confirmed
  privilege intents at `crates/vpsctl/src/commands_inventory.rs:459-523`.
  VTY does the same at `crates/vpsctl/src/vty_inventory.rs:317-370`, while
  the parser accepts `tag-create <name>` and `agent-tag <client_id> <tag>`
  without any confirmation flag at
  `crates/vpsctl/src/vty_inventory.rs:774-790`.
- Notes: This complements AUD-041, which is the frontend inline-tag variant.
  The clean CLI fix should require explicit `--confirmed` and preferably expose
  a dry-run/preview command that displays target and schedule-impact data.

### AUD-140: Single-File Browser Confirmations Remain Armed After Operation Edits

- Severity: Medium/High
- Status: Fixed
- Area: Frontend/File Browser
- Context: The single-VPS file browser lets operators create, upload, rename,
  delete, chmod, chown, copy, move, and save files on one selected VPS. These
  workflows use a confirmation prompt before dispatching privileged file jobs.
- Root Cause: `confirmOperation` stores a frozen operation in
  `pendingConfirmation`, and target changes clear that prompt, but most other
  operation-affecting controls remain editable without clearing it. Path input,
  selected file/folder, new name, upload file/destination/mode/ownership,
  recursive flags, chmod mode, chown owner/group, rename destination, create
  content, and clipboard source changes can leave the old prompt armed.
- Impact: An operator can review one file operation, edit the visible form, and
  still submit the previously reviewed operation. The job payload is frozen, so
  this is not a mutable-payload backend bug, but it is a practical production
  safety problem: a stale confirmation can delete, chmod, chown, rename,
  overwrite, upload, or save an older path/byte set while the visible composer
  now shows a different intended operation.
- Evidence: `frontend/src/panels/jobs/FileBrowserPanel.tsx:330-339` creates the
  pending confirmation snapshot. Target changes clear it at
  `frontend/src/panels/jobs/FileBrowserPanel.tsx:171-181`, but form handlers
  such as path input, editor mode/content, upload file/destination/options,
  create fields, rename destination, chmod/chown fields, and recursive toggles
  update state without clearing `pendingConfirmation` at
  `frontend/src/panels/jobs/FileBrowserPanel.tsx:635-939`. The confirm handler
  then executes the old frozen operation at
  `frontend/src/panels/jobs/FileBrowserPanel.tsx:972-984`.
- Notes: This is distinct from AUD-055, which covers the saved-content marker
  after editor edits. The clean fix should match the multi-file panel behavior:
  any edit that changes the pending operation must close the confirmation and
  require a fresh review.
- Fix: The file browser now invalidates pending single-file confirmations when
  any operation-affecting draft field changes, including path, selected item,
  editor content/mode, upload/create/rename/chmod/chown fields, recursive
  flags, and clipboard source. Covered by a Playwright regression in
  `frontend/tests/console-file-browser.spec.ts`.

### AUD-141: Supervisor PID Records Can Target Reused Host Processes After Agent Restart

- Severity: High
- Status: Confirmed
- Area: Agent/Process Supervisor/Safety
- Context: Process-supervisor jobs start long-running commands on an agent and
  later stop, restart, reconcile, or report those managed processes after agent
  restart. The agent persists supervisor records under its local state root so
  the process supervisor can recover across restarts.
- Root Cause: A persisted supervisor record stores only numeric `pid` and
  `process_group_id` as process identity. Later lifecycle paths treat
  `kill(pid, 0)` success as proof that the persisted process is still the
  original supervised process, without checking a stable process identity such
  as `/proc/<pid>/stat` start time, process-group identity, executable/argv
  fingerprint, or a supervisor-owned pidfd equivalent.
- Impact: If a supervised process exits while the agent is down and Linux later
  reuses its PID or process-group ID, `process-start` can falsely reject a new
  start because an unrelated PID exists, `process-status` can report an
  unrelated host process as managed, and `process-stop` or `process-restart`
  can send SIGTERM/SIGKILL to an unrelated PID or process group. On production
  VPSs this can stop arbitrary workloads outside the supervisor's ownership.
- Evidence: `ProcessRecord` stores `pid` and `process_group_id` at
  `crates/agent/src/supervisor.rs:41-54`, and `start_process` records them
  from `child.id()` at `crates/agent/src/supervisor.rs:334-343`.
  `process-start` rejects an existing record solely when
  `process_is_running(record.pid)` is true at
  `crates/agent/src/supervisor.rs:182-184`. `stop_record` signals the stored
  process group and then falls back to the stored PID at
  `crates/agent/src/supervisor.rs:363-388`. `reconcile_record` marks a record
  running solely when `process_is_running(record.pid)` is true at
  `crates/agent/src/supervisor.rs:420-429`. `process_is_running` is implemented
  as `kill(pid, 0)` at `crates/agent/src/process_cleanup.rs:57-58`, and process
  group signaling uses `kill(-process_group_id, signal)` at
  `crates/agent/src/process_cleanup.rs:70-75`.
- Notes: This is practical, not lab-only: the supervisor explicitly supports
  persisted long-running process records and startup reconciliation. PID reuse
  after an agent crash/restart or machine churn is normal on Linux over long
  uptime. The clean fix should persist and verify a stable process identity
  before treating a stored PID/PGID as owned, and should fail closed rather
  than signaling a mismatched process.

### AUD-142: Supervisor Records And Logs Are Written With Default-Readable Permissions

- Severity: High
- Status: Confirmed
- Area: Agent/Process Supervisor/Security
- Context: Process-supervisor jobs start long-running commands on a VPS agent,
  persist restart state under the supervisor root, and append stdout/stderr to
  local supervisor log files. Operators can supply argv, cwd, and environment
  values from the frontend and CLI, and those values are needed for restart
  policy recovery.
- Root Cause: Supervisor records serialize the full `argv`, `cwd`, and `env`
  into JSON and write them with plain `OpenOptions::create_new(true).write(true)`
  under directories created by `create_dir_all`. Supervisor stdout/stderr logs
  are also opened with `OpenOptions::create(true).append(true)`. No code sets
  owner-only modes on the state directories, temp record files, final record
  files, or log files before writing secret-bearing material.
- Impact: On typical Linux umasks such as `022`, the persisted supervisor JSON
  and logs can become readable by non-root local users on the VPS. That exposes
  service tokens, database URLs, API keys, command arguments, cwd paths, and
  application stdout/stderr that operators commonly provide to supervised
  daemons. This bypasses the API `jobs:read` boundary and is practical on
  production VPSs that host application users, deploy users, or compromised
  low-privilege accounts.
- Evidence: `ProcessRecord` stores `argv`, `cwd`, and `env` at
  `crates/agent/src/supervisor.rs:41-47`, and `start_process` copies operator
  inputs into the record at `crates/agent/src/supervisor.rs:334-340`.
  Environment values are allowed up to 32 entries of 4096 bytes each at
  `crates/agent/src/supervisor_validation.rs:46-63`. The frontend exposes
  command argv, cwd, and env fields at
  `frontend/src/panels/jobs/JobOperationControls.tsx:891-918`, and vpsctl
  builds `ProcessStart` with `env` from CLI options at
  `crates/vpsctl/src/commands_process.rs:116-134`. Record save creates the
  parent directory and temp JSON file without explicit permissions at
  `crates/agent/src/supervisor.rs:790-806`, then renames it into place at
  `crates/agent/src/supervisor.rs:812-817`. Supervisor logs are opened without
  explicit permissions at `crates/agent/src/supervisor.rs:303-314`.
- Notes: This is separate from AUD-123, which covers API read scope for derived
  process inventory, and from AUD-141, which covers PID identity. The clean fix
  should make the supervisor state root, records directory, logs directory,
  record temp files, final record files, and log files owner-only before
  writing, and should repair permissions on existing supervisor state during
  startup reconciliation.

### AUD-143: Headless CLI Tutorial Presents The Public Panel URL As The Operator API Endpoint

- Severity: Medium/High
- Status: Confirmed
- Area: Docs/Deployment/API Boundary
- Context: The project policy and current deployment shape keep the API as a
  private operator/control-plane service. Public URLs, when needed, are for
  static frontend hosting, release artifacts, or explicitly exposed agent
  gateway TCP, not for the API itself.
- Root Cause: The headless CLI/VTY tutorial still teaches operators to set
  `VPSMAN_API_URL=https://panel.example.com`, which implies the CLI should use
  a public dashboard/panel origin as the API endpoint. That contradicts the
  current private-API guidance elsewhere.
- Impact: An operator following the headless tutorial can recreate the exact
  exposure class the current deployment defaults avoid: publishing dashboard
  routes together with `/api` and `/ws`, or configuring scripts to depend on a
  public panel URL for private API access. This increases the blast radius of
  bearer tokens, auth throttling, WebSocket streams, scope bugs, and any future
  operator endpoint weakness.
- Evidence: `tutorials/09-headless-cli-vty.md:8-12` says "Set API access" and
  exports `VPSMAN_API_URL=https://panel.example.com`. In contrast,
  `docs/operator-access-scopes.md:3-5` says the API and gateway are private and
  must not be exposed publicly, and `README.md:124-129` says compose does not
  publish the API host port while only agent TCP should be exposed through an
  operator-chosen proxy/firewall/tunnel when needed.
- Notes: This is distinct from AUD-067. AUD-067 fixed the default compose
  public frontend/API exposure path; this is stale operator-facing guidance that
  can lead a production deployment back into that unsafe topology.

### AUD-144: Strict Registered-Update Policy Only Gates Direct Staging Jobs

- Severity: High
- Status: Confirmed
- Area: API/Worker/Agent Updates
- Context: Operators can enable `require_registered_agent_updates` to make
  production agent updates depend on the API release registry before replacing
  binaries across the fleet.
- Root Cause: The API and worker enforcement functions return success for every
  update-family command except `agent_update`. They check registered release
  metadata only for `JobCommand::UpdateAgent { sha256_hex, .. }` and allow
  `agent_update_check`, `agent_update_activate`, and `agent_update_rollback`
  without registry verification.
- Impact: With strict registered updates enabled, an operator or compromised
  jobs-write path can still run a manifest-check update against an arbitrary
  version URL and activate the staged binary, or activate/rollback to a local
  staged hash, without that artifact being present in the release registry.
  That defeats the integrity policy operators would rely on for controlled
  fleet updates, especially in 20+ VPS rollouts where the registry is meant to
  be the approved source of update artifacts.
- Evidence: `crates/api/src/routes_jobs.rs:559-574` checks
  `require_registered_agent_updates` but returns `Ok(true)` unless the command
  is `UpdateAgent`. The scheduled-worker mirror has the same shape at
  `crates/worker/src/main.rs:2218-2242`. The protocol defines other
  binary-changing update commands with hashes and manifest URLs at
  `crates/common/src/protocol.rs:2366-2379`. The agent update-check path fetches
  the supplied manifest, stages the artifact, and activates it when requested at
  `crates/agent/src/update.rs:73-113` and `crates/agent/src/update.rs:205-320`.
- Notes: This is separate from AUD-040 and AUD-060, which cover release-registry
  mutation review/scope, from AUD-084, which covered redirect support, and from
  AUD-119/AUD-120, which covered activation heartbeat ordering and hash
  matching. This issue is specifically the server-side policy bypass when strict
  registered updates are enabled.

### AUD-145: Key Rotation, Revoke, And Delete Disconnect Before DB Invalidation, Leaving A Reconnect Race

- Severity: High
- Status: Confirmed
- Area: API/Gateway/Key Lifecycle
- Context: Key rotation, current-key revocation, and client deletion are
  operator access-deactivation workflows. They are used during reinstall,
  retirement, and compromise response, where operators expect the old client
  key or deleted client to stop being usable immediately.
- Root Cause: The API asks the gateway to disconnect the current live session
  before committing the database state that makes the old key/client invalid.
  During that window, the old agent can reconnect. The gateway validates the
  handshake against the still-valid database row, posts hello, and registers a
  new in-memory session. The later DB transaction replaces/revokes/clears the
  key and ends recorded `gateway_sessions`, but it does not issue a second
  post-commit disconnect for a session that arrived during the window.
- Impact: A compromised or retired agent key can keep a live gateway transport
  after an operator believes rotation, revocation, or deletion cut access. The
  post-commit state prevents normal new dispatch because the client is hidden
  or lacks a current process incarnation, and late command output is guarded by
  terminal-state checks. However, the stale transport can still remain in the
  gateway's in-memory session map, continue sending frames until naturally
  closed, and create misleading live-session/telemetry effects. In incident
  response or fleet retirement, that weakens the operational meaning of
  "revoke", "delete", and "replace key".
- Evidence: Key rotation verifies privilege and then calls
  `disconnect_gateway_session_for_lifecycle` before repository mutation at
  `crates/api/src/routes_key_lifecycle.rs:39-49`. Current-key revocation has
  the same order at `crates/api/src/routes_key_lifecycle.rs:91-100`. Client
  deletion has the same order at `crates/api/src/routes_inventory.rs:96-104`.
  The Postgres rotation mutation only later locks the client row, ends
  gateway-session rows, clears `process_incarnation_id`, and replaces the key
  at `crates/api/src/repository_key_lifecycle.rs:299-368`. Current-key
  revocation only later inserts the revocation, marks active targets lost, and
  hides/revokes the client at
  `crates/api/src/repository_key_lifecycle.rs:582-688`. Deletion only later
  hides the client, clears `public_key`, and clears `process_incarnation_id` at
  `crates/api/src/repository_inventory.rs:885-914`. Gateway identity
  validation accepts a handshake when `clients.public_key` still matches and
  `hidden_at IS NULL` at `crates/api/src/repository_ingest.rs:342-370`, and
  the gateway registers that accepted session in memory at
  `crates/gateway/src/main.rs:766-795`. The hello upsert can internally set
  `accepted_hello = false` when the row is hidden, but
  `ingest_agent_hello` still returns an accepted response at
  `crates/api/src/repository_ingest.rs:528-590` and
  `crates/api/src/routes_ingest.rs:45-60`, so the gateway has no protocol-level
  rejection to honor if lifecycle state changes between validation and hello
  ingest.
- Notes: This is distinct from AUD-113 and AUD-114. Those fixed missing
  disconnect calls for already-open sessions; this issue is a time-of-check /
  time-of-use ordering gap around new handshakes during the lifecycle
  operation. A clean fix should make the DB invalidation and lifecycle target
  updates win before any old-key reconnect can be accepted, then disconnect any
  live gateway session for that client after commit. One practical model is:
  lock and invalidate the client row in a transaction, commit, call gateway
  disconnect, and make the operation fail closed or explicitly surface a
  pending-disconnect state if the post-commit disconnect cannot be delivered.
  Another model is a database-backed lifecycle fence that
  `validate_agent_public_key` rejects while key rotation/revoke/delete is in
  progress.

### AUD-146: Publishing The Dashboard Frontend Still Publishes API And WebSocket Routes

- Severity: High
- Status: Confirmed
- Area: Deploy/Nginx/API Boundary
- Context: The API and gateway are private operator control-plane services, and
  public URLs for static frontend hosting or release artifacts are supposed to
  be supplied separately. The compose deployment now keeps the API service off
  host ports and binds the frontend to loopback by default, but operators can
  still widen `VPSMAN_FRONTEND_BIND` or put the frontend container behind a
  public reverse proxy when they want browser access.
- Root Cause: The shipped frontend Nginx configuration is not a static-only
  frontend. It also proxies `/api/`, `/health`, and `/ws` to the private API
  service. Since the frontend container is the host-published service in the
  compose template, publishing the dashboard endpoint also publishes API and
  WebSocket routes on that same origin.
- Impact: An operator trying to expose only a dashboard/static frontend can
  accidentally expose the private operator API and live WebSocket endpoint. That
  turns a public dashboard URL into a control-plane URL, contradicting the
  documented security model and increasing the blast radius of any token,
  session, scope, browser, or reverse-proxy mistake. This is practical because
  using a host bind or external proxy for browser access is a normal deployment
  step, even though API access should remain private.
- Evidence: `deploy/compose.yml:64-72` publishes only the `frontend` service
  host port through `${VPSMAN_FRONTEND_BIND:-127.0.0.1:5173}:80`. The API
  service intentionally has no host port, and `README.md:124-129` explains that
  Nginx reaches it over the private Docker network. However,
  `deploy/nginx.conf:8-14` proxies `/api/`, `deploy/nginx.conf:16-22` proxies
  `/health`, and `deploy/nginx.conf:24-32` proxies `/ws` to `http://api:8080`.
  The control-plane policy is explicit in `docs/operator-access-scopes.md:3-5`:
  the API and gateway are private and public URLs are separate.
- Notes: This is not the same as the already-fixed default exposure issue. The
  default compose bind is loopback, but the remaining defect is that frontend
  publication and API publication are still the same Nginx surface. A clean fix
  should split static frontend hosting from private operator API proxying. A
  public/static frontend profile should not proxy API or WebSocket routes at
  all. If a same-origin dashboard-to-API proxy remains useful, it should be a
  clearly private operator profile/config with explicit naming and docs.

### AUD-147: Custom Agent Binary URL Installs Without A Required SHA-256 Pin

- Severity: Medium/High
- Status: Confirmed
- Area: Deploy/Agent Install/Supply Chain
- Context: The documented one-line agent installer supports the default
  official GitHub release flow and also supports a custom
  `VPSMAN_AGENT_BINARY_URL` for operators who host agent binaries elsewhere.
  Custom hosting is practical for private fleets, air-gapped mirrors, regional
  mirrors, or pre-release rollout testing.
- Root Cause: The custom URL install path downloads and installs the binary even
  when `VPSMAN_AGENT_BINARY_SHA256` is absent. The default GitHub path is
  hash-verified through `SHA256SUMS`, but the custom URL path treats the hash as
  optional instead of requiring the operator to pin the exact bytes.
- Impact: An operator can roll out agents from a mutable or compromised URL with
  no immutable byte binding. DNS/proxy mistakes, mirror drift, CDN cache
  poisoning, or an accidentally replaced artifact can install a different agent
  binary than the operator intended. Because agent installation provisions the
  long-running fleet control component, this is a production supply-chain and
  rollback-forensics issue, not a cosmetic installer detail.
- Evidence: The operator-facing docs advertise `VPSMAN_AGENT_BINARY_URL` and
  `VPSMAN_AGENT_BINARY_SHA256` as optional values at
  `deploy/AGENT_GATEWAY_INSTALL.md:19-24`. The documented one-line install uses
  `deploy/install-agent.sh` at `README.md:174-188` and
  `deploy/AGENT_GATEWAY_INSTALL.md:50-70`. In that script, the default release
  path downloads `version.json`, the tag-pinned agent asset, and `SHA256SUMS`,
  then verifies the selected checksum at `deploy/install-agent.sh:103-127`. The
  custom URL path downloads `VPSMAN_AGENT_BINARY_URL` and only verifies a hash
  if `VPSMAN_AGENT_BINARY_SHA256` is non-empty at
  `deploy/install-agent.sh:180-189`. By contrast, the older smoke-only
  `scripts/install-agent.sh` requires `VPSMAN_AGENT_SHA256_HEX` when downloading
  from a URL at `scripts/install-agent.sh:392-401`, and
  `scripts/smoke-agent-install-assets.sh:265-273` asserts that missing-hash URL
  downloads are rejected. That safety invariant is not applied to the current
  documented installer.
- Notes: Keep the default GitHub manifest flow as-is. The clean fix is to
  require `VPSMAN_AGENT_BINARY_SHA256` whenever `VPSMAN_AGENT_BINARY_URL` is
  used, validate it as a 64-character hex digest, and add smoke coverage for
  `deploy/install-agent.sh` so the documented installer and the test installer
  enforce the same byte-pinning rule. If operators want no-hash local installs,
  they can use an explicit local file path such as `VPSMAN_AGENT_BINARY_PATH`,
  where the trusted filesystem boundary is different from a mutable URL.

### AUD-148: Backup Policy Prune Reselects Live Artifacts Instead Of The Reviewed Candidate Set

- Severity: High
- Status: Confirmed
- Area: API/Frontend/CLI/Backups/Retention
- Context: Backup policy pruning is a destructive retention workflow. Operators
  can dry-run policy prune, review matched rows/object keys, then run a
  confirmed prune to remove backup artifact metadata and, when configured,
  object-store bytes.
- Root Cause: The confirmed prune request carries only `schedule_id`,
  `dry_run`, `metadata_only`, and `confirmed`. It does not carry a preview
  hash, preview token, or the concrete candidate identities from the reviewed
  dry-run. The API therefore calls the live candidate selector again during the
  confirmed request and prunes whatever rows match at that later moment.
- Impact: A confirmed prune can delete backup artifacts that were not in the
  operator-reviewed dry-run. This is practical in a 20+ VPS fleet because
  scheduled backups, manual handoffs, artifact metadata imports, policy edits,
  or delayed artifact records can change the retention candidate set between
  dry-run and confirmation. The normal UI confirmation only says scope and mode,
  so an operator may believe they are applying the reviewed set while the API
  is deleting a different set of backup evidence.
- Evidence: `BackupPolicyPruneRequest` exposes only `schedule_id`, `dry_run`,
  `metadata_only`, and `confirmed` at
  `crates/api/src/model_backups.rs:123-131`. The route re-lists candidates on
  every request at `crates/api/src/routes_backups.rs:177-190`, then deletes the
  reselected metadata/objects at `crates/api/src/routes_backups.rs:203-230`.
  The selector returns concrete identities including `request_id`,
  `artifact_id`, and `object_key` at
  `crates/api/src/repository_backup_policies.rs:490-544`, but those identities
  are not persisted or submitted by the confirmation. The dashboard dry-run and
  confirm flow builds a fresh request from live controls and confirms only
  scope/mode at `frontend/src/panels/BackupsPanel.tsx:544-569` and
  `frontend/src/panels/BackupsPanel.tsx:1189-1211`. The CLI similarly posts
  only the same request fields at `crates/vpsctl/src/commands_backups.rs:145-178`.
- Notes: This is distinct from AUD-056, which covers object-delete ordering
  after a row is selected, and from AUD-050, which covered server artifact
  cleanup re-evaluating an expression. The clean fix should freeze candidate
  identities during preview/dry-run, require a server-issued preview token or
  preview hash for confirmed prune, and have the confirmed prune consume exactly
  those reviewed `backup_request_id`/`artifact_id`/`object_key` identities. If
  any identity no longer matches, skip it with visible evidence or reject and
  require a fresh review.

### AUD-149: Compose Update And Rollback Swap Release Directories Without Forcing Container Recreation

- Severity: High
- Status: Confirmed
- Area: Deploy/Update/Rollback
- Context: The official compose deployment runs released server binaries and
  frontend assets from `deploy/runtime/server/current` and
  `deploy/runtime/frontend/current`. Operators use `deploy/update.sh latest`,
  `deploy/update.sh <tag>`, and `deploy/update.sh rollback` for production
  server/frontend upgrades and rollback.
- Root Cause: `deploy/update.sh` swaps the host `current` and `previous`
  release directories, then runs plain `docker compose up -d --remove-orphans`.
  The compose service definitions and images are unchanged, so Compose is not
  forced to recreate already-running containers. Existing containers also keep
  their bind mounts to the directory inode mounted when the container started;
  replacing the host pathname does not move a running container onto the new
  directory.
- Impact: An update or rollback can report success while API, gateway, worker,
  and frontend containers keep running the previous release. This is practical
  and production-impacting because operators may believe a security/reliability
  fix or rollback is live when the fleet control plane is still executing the
  old binaries/assets. It also makes post-release verification confusing: the
  filesystem under `deploy/runtime/*/current` shows the new release, while live
  processes can still be the old release until containers are manually recreated
  or restarted.
- Evidence: `deploy/compose.yml:14-24`, `deploy/compose.yml:31-41`,
  `deploy/compose.yml:48-56`, and `deploy/compose.yml:64-70` run fixed images
  with bind mounts from `./runtime/server/current` and
  `./runtime/frontend/current/dist`. `swap_release_dir` renames the `current`
  directory and moves the staged directory into its place at
  `deploy/update.sh:99-114`. Normal update calls only
  `compose up -d --remove-orphans` after swapping at `deploy/update.sh:207-214`.
  Rollback performs the same directory swap and also calls only
  `compose up -d --remove-orphans` at `deploy/update.sh:116-137`. README states
  the script recreates containers at `README.md:147-149`, but the script does
  not pass `--force-recreate`, run `compose restart`, or otherwise replace the
  running containers after the bind mount source path is swapped.
- Notes: The clean fix should make update and rollback explicitly restart or
  recreate the affected services after the verified directory swap. Use
  `--force-recreate` for API, gateway, worker, and frontend, or an equivalent
  controlled stop/up sequence. The script should verify the live
  server/frontend versions after recreation and fail loudly if the running
  services still report the old release.

### AUD-150: Displaced Gateway Sessions Can Keep Forwarding Telemetry After Replacement

- Severity: High
- Status: Confirmed
- Area: Gateway/API/Telemetry/Lifecycle
- Context: The gateway accepts long-lived agent TCP/Noise sessions. In
  production, duplicate client identity use, VM image cloning, reconnect races,
  half-open TCP sessions, or a restarted process can cause a new session for a
  client while an older transport is still alive.
- Root Cause: On a new hello, the gateway overwrites
  `GatewayState.sessions[client_id]` with the new session but does not signal
  the displaced session loop to close. The API marks older active
  `gateway_sessions` rows as `expired`, but telemetry ingests do not carry or
  verify gateway `session_id` or `process_incarnation_id`; the gateway only
  checks that the telemetry `client_id` matches the session-local client ID,
  and the API only checks that the client is not hidden.
- Impact: A displaced or duplicate agent process can continue sending telemetry
  after the API and dashboard consider its session replaced. That stale
  transport can refresh `clients.last_seen_at`, keep status online, replace
  telemetry rollups/network rates/tunnel telemetry, and emit telemetry webhook
  events for the wrong live process. In a 20+ VPS fleet this can mask a real
  agent replacement, pollute topology/alert state, and make operators trust
  metrics from a stale or cloned host.
- Evidence: `crates/gateway/src/main.rs:780-796` posts hello and inserts the
  new session into the in-memory map without disconnecting any previous sender
  for that client. `crates/gateway/src/main.rs:830-848` accepts telemetry from
  any still-running session loop after only checking the local client ID.
  `crates/api/src/repository_gateway_sessions.rs:69-85` marks older DB session
  rows `expired`, but does not close the transport. `crates/api/src/repository_ingest.rs:706-748`
  records telemetry and updates `clients.last_seen_at` based only on
  `event.telemetry.client_id` and hidden-client state.
- Notes: This is distinct from AUD-145, which covers key-lifecycle reconnect
  races. A clean fix should actively disconnect displaced gateway sessions when
  a new session is accepted and bind telemetry ingestion to the current
  gateway session/incarnation, rejecting or dropping telemetry from stale
  transports.

### AUD-151: Operator Management Mutations Lack Request-Bound Privilege Verification

- Severity: High
- Status: Fixed
- Area: API/Frontend/CLI/Auth/Privilege
- Context: System > Users and the CLI manage operator accounts, roles, scopes,
  passwords, TOTP state, status, and sessions. These actions decide who can
  access the private control plane and can directly affect every fleet
  operation.
- Root Cause: Operator management request models have `confirmed` and
  `admin_risk_acknowledged` fields, but no `privilege_assertion`. The API
  routes require only an admin bearer session and, for most mutations, a
  boolean confirmation. The frontend and CLI build confirmation prompts or
  hardcode `confirmed: true`, but they do not build a super-password-derived
  request-bound privilege assertion for the exact account-management payload.
- Impact: A stolen or replayed admin access token, compromised admin browser
  session, or CSRF-like private-dashboard bug can create another admin, widen
  scopes, reset passwords, clear TOTP, disable/delete operators, or revoke
  sessions without proving possession of the local privilege secret. This
  weakens the project-wide privilege model exactly on account recovery and
  account takeover paths.
- Evidence: `crates/api/src/auth_model.rs:286-324` defines
  `CreateOperatorRequest`, `UpdateOperatorRequest`,
  `OperatorLifecycleRequest`, and `OperatorPasswordResetRequest` without a
  privilege assertion field. `crates/api/src/routes_auth.rs:309-465` performs
  create, update, status, password reset, and TOTP-clear mutations after admin
  auth and simple confirmation/admin-risk checks only. Session revocation at
  `crates/api/src/routes_auth.rs:495-506` has no confirmation or privilege
  assertion. The dashboard freezes user action state at
  `frontend/src/panels/SystemPanel.tsx:430-575`, then calls the mutation
  handlers without any privilege assertion. The CLI similarly posts operator
  mutations with no privilege assertion and often hardcoded confirmation at
  `crates/vpsctl/src/commands_auth.rs:80-183`.
- Notes: This is distinct from AUD-046, which tracks missing or auto-confirmed
  confirmation semantics. The clean fix should add an account-management
  privilege intent bound to action, target operator IDs, role, scopes, session
  TTL, password-reset marker or password hash, and admin-risk acknowledgement.
  First bootstrap remains the special unauthenticated setup path.
- Resolution: Fixed by adding request-bound privilege assertions to operator
  create/update/lifecycle/password-reset/TOTP-clear/session-revoke contracts.
  The API recomputes a non-secret canonical payload hash from the actual
  request shape and verifies the DB privilege intent through the gateway.

### AUD-152: Migration Restore Runs Can Use Stale Hidden Restore Options

- Severity: High
- Status: Confirmed
- Area: Frontend/Backups/Migrations
- Context: The dashboard migration assistant is presented as a migration-specific
  workflow: select a restore plan, link it, and optionally dispatch the restore
  run for the target VPS. Operators expect the restore run to be derived from
  the selected restore plan plus visible migration controls.
- Root Cause: The migration-run submit path builds the restore job from shared
  restore-run state owned by the restore subpage: agent-local archive path,
  archive SHA-256, uploaded artifact file, dry-run flag, private key, post-restore
  command, timeout, and force-unprivileged flag. The migration assistant only
  displays checklist summaries for some of these values and does not provide
  controls for editing or clearing them. The confirmation prompt for migration
  restore shows only restore plan, route, mode, and privilege, omitting artifact
  source, archive path/hash, uploaded artifact, post-restore command, timeout,
  and force-unprivileged policy.
- Impact: An operator can prepare a restore run for one context, switch to the
  migration assistant, select a different restore plan, and dispatch a migration
  restore that silently uses stale hidden restore options. In production this can
  restore from an unintended agent-local archive, use a stale uploaded artifact
  or private key, run an old post-restore command, or use the wrong privilege
  policy/timeout. Because migration restores are destructive recovery/cutover
  workflows, hidden stale state can mutate the wrong files or run unexpected
  commands on the target VPS.
- Evidence: `buildRestoreRunJobSnapshot` includes `archive_path`,
  `archive_sha256_hex`, `artifactFile`, `dry_run`, `privateKeyHex`,
  `postRestoreArgv`, timeout, and privilege policy in the dispatched restore
  operation at `frontend/src/panels/BackupsPanel.tsx:828-895`.
  `submitMigrationRun` populates those fields from shared `restore*` state
  rather than migration-local visible controls at
  `frontend/src/panels/BackupsPanel.tsx:1094-1108`. The migration confirmation
  items show only plan, route, mode, and privilege at
  `frontend/src/panels/BackupsPanel.tsx:1384-1414`. The migration assistant
  component receives `archivePath`, `forceUnprivileged`, `postRestoreArgv`,
  `privateKeyReady`, and `restoreDryRun`, but only renders checklist text plus
  restore-plan and note inputs, with no editable controls for the hidden restore
  options at `frontend/src/panels/backups/MigrationLinkForm.tsx:6-105` and
  `frontend/src/panels/backups/MigrationLinkForm.tsx:108-200`.
- Notes: This is distinct from AUD-029. The migration-run snapshot is frozen
  once reviewed, but the snapshot can be built from stale, hidden restore-run
  state before review. A clean fix should make migration-run options explicitly
  visible and editable in the migration assistant, or reset/derive them
  migration-locally and require the confirmation prompt to include the complete
  restore-run evidence.

### AUD-153: Per-Interface Network-Rate Telemetry Has No Retention Path

- Severity: Medium/High
- Status: Confirmed
- Area: API/Telemetry/Retention
- Context: Agents send network interface counters as part of normal telemetry.
  The API stores per-client, per-interface, per-bucket network-rate rows and
  uses them for dashboard and telemetry history queries.
- Root Cause: `telemetry_network_rates` is a separate rollup table, but the
  canonical history-retention domain list and prune implementation only include
  `telemetry_rollups`, `system_metric_rollups`, job outputs, backups, audit
  logs, and network observation/topology history. No retention domain or prune
  branch selects or deletes old `telemetry_network_rates` rows.
- Impact: In a 20+ VPS fleet, every connected client can write one row per
  network interface per telemetry bucket indefinitely. Operators can configure
  and run history retention for telemetry rollups while this adjacent
  per-interface telemetry table keeps growing, increasing database size,
  index bloat, backup/restore cost, and dashboard query cost over long-running
  deployments.
- Evidence: The schema creates `telemetry_network_rates` with
  `(client_id, interface, bucket_secs, bucket_start)` as the primary key at
  `migrations/0003_telemetry_alerts_history.sql:24-37`. Normal telemetry ingest
  upserts rows into that table for each valid interface at
  `crates/api/src/repository_ingest.rs:1181-1227`. Dashboard and API telemetry
  reads query the table at
  `crates/api/src/repository_telemetry_rollups.rs:232-246` and
  `crates/api/src/repository_telemetry_rollups.rs:333-386`. The history
  retention domain enum has no network-rate domain at
  `crates/api/src/model_history.rs:4-23`, the schema CHECK for retention
  domains excludes it at `migrations/0003_telemetry_alerts_history.sql:370-388`,
  and `prune_telemetry_rollups` deletes only from `telemetry_rollups` at
  `crates/api/src/repository_history.rs:1019-1058`.
- Notes: This is not the same as `telemetry_tunnels`: tunnel telemetry is
  deleted and replaced per client during ingest, so it is current state rather
  than unbounded history. A clean fix should either add a first-class
  `telemetry_network_rates` retention domain or make the existing telemetry
  rollup retention domain prune both `telemetry_rollups` and
  `telemetry_network_rates` with clear operator-facing counts.

### AUD-154: History Retention Prune Reselects Live Rows Instead Of Deleting The Reviewed Dry-Run Set

- Severity: High
- Status: Confirmed
- Area: API/Frontend/CLI/History Retention
- Context: Operators can dry-run history retention prune for domains such as
  `job_outputs`, `backup_artifacts`, audit logs, telemetry rollups, and network
  history, review the matched row/object counts, and then run a confirmed prune.
  For object-backed domains this can remove both database metadata and retained
  object-store payloads.
- Root Cause: The prune request contains only `domain`, `dry_run`,
  `metadata_only`, and `confirmed`. A confirmed prune recomputes live
  candidates from the current retention policies and current table contents
  instead of consuming a server-issued preview token, preview hash, or concrete
  row/object identities from the reviewed dry-run.
- Impact: A confirmed prune can delete history rows or object-store payloads
  that were not present in the operator-reviewed dry-run. This is practical in
  a 20+ VPS deployment because job outputs, file-transfer output artifacts,
  backup artifacts, audit records, telemetry rows, and network observations can
  arrive or age past the cutoff between review and confirmation. Operators can
  believe they are applying a reviewed retention set while the API deletes a
  later live set of forensic or restore evidence.
- Evidence: `HistoryRetentionPruneRequest` exposes only `domain`, `dry_run`,
  `metadata_only`, and `confirmed` at `crates/api/src/model_history.rs:105-112`.
  `prune_history_retention` re-lists policies and recomputes each domain's
  cutoff on every request at `crates/api/src/routes_history.rs:62-82`.
  For object-backed domains it calls
  `list_history_retention_object_candidates` during the confirmed request at
  `crates/api/src/routes_history.rs:91-94`, then deletes those live candidates
  at `crates/api/src/routes_history.rs:109-133`. Non-object domains similarly
  call `prune_history_domain` from the current cutoff and policy at
  `crates/api/src/routes_history.rs:151-154`. The frontend confirmation calls
  `prune(false)` and submits only the live domain/mode fields at
  `frontend/src/panels/AuditLogPanel.tsx:140-154`, and the CLI/VTY paths post
  the same four request fields at `crates/vpsctl/src/commands_jobs.rs:546-568`
  and `crates/vpsctl/src/vty_direct.rs:457-497`.
- Notes: This is distinct from AUD-037, which covers frontend mutable prune
  controls after a prompt opens, and from AUD-051, which covers object-delete
  ordering after a candidate has been selected. The clean fix should freeze the
  dry-run candidates on the server with a preview token or require the confirmed
  request to carry the reviewed row/object identities plus a preview hash. If
  any reviewed identity no longer matches, the API should skip it with visible
  evidence or reject and require a fresh review.

### AUD-155: Failed Artifact Cleanup Jobs Can Hide Already-Deleted Artifacts

- Severity: High
- Status: Confirmed
- Area: Worker/Artifact Cleanup/Observability
- Context: Artifact cleanup is a destructive server-side maintenance job that
  deletes or tombstones retained job-output, file-transfer, and backup artifact
  evidence. Operators rely on the resulting server-job row to understand what
  was actually removed, especially after cleanup errors.
- Root Cause: The worker accumulates deleted/tombstoned/skipped counts in local
  memory while processing candidates, but if any later candidate returns an
  error, the error path updates only `status`, `error`, and `completed_at`.
  Counts and metadata are persisted only on the all-success path.
- Impact: A cleanup job can delete some artifacts or metadata, then fail on a
  later object-store/delete operation and be shown as `failed` with
  `deleted_count = 0`, `deleted_bytes = 0`, and no tombstone/skip metadata. In
  production this makes destructive maintenance evidence misleading: operators
  may believe no artifacts were removed, rerun cleanup unnecessarily, or miss
  that forensic job output, file-transfer artifacts, or backup artifact rows
  were already changed before the failure.
- Evidence: `server_jobs` stores aggregate deletion fields with defaults at
  `migrations/0002_jobs_schedules_commands.sql:201-220`. The worker persists
  `deleted_count`, `deleted_bytes`, and tombstone/skip metadata only in the
  success branch at `crates/worker/src/main.rs:1101-1130`. On error it writes
  only `status = failed`, `error`, and `completed_at` at
  `crates/worker/src/main.rs:1132-1147`. `run_artifact_cleanup_job` increments
  the local counts after each successful candidate, but uses `?` on
  `apply_artifact_cleanup_candidate`, so a later failure discards those counts
  at `crates/worker/src/main.rs:1189-1210`. Candidate handlers mutate durable
  metadata and delete objects before returning success, for example job-output
  cleanup at `crates/worker/src/main.rs:1261-1281`,
  file-transfer source cleanup at `crates/worker/src/main.rs:1323-1342`, and
  backup artifact cleanup at `crates/worker/src/main.rs:1358-1389`.
- Notes: This is distinct from AUD-006, which covers metadata/object ordering,
  AUD-049, which covers domain authorization, and AUD-050, which covers
  reviewed-set freezing. A clean fix should persist per-candidate terminal
  status or update aggregate counts incrementally in the same transaction as
  each candidate mutation. Failed jobs should visibly report partial deletion,
  tombstone, and skipped counts.

### AUD-156: Process Status And Log Reads Can Restart Supervised Processes

- Severity: High
- Status: Confirmed
- Area: Agent/Process Supervisor/Command Semantics
- Context: Operators use process-supervisor status and log jobs as inspection
  commands while incident work, scheduled checks, or explicit start/stop/restart
  operations are running across production VPSs.
- Root Cause: The shared command-safety table classifies `process_status` and
  `process_logs` as read-only, but both handlers call
  `reconcile_and_save_record`. Reconciliation is mutating: it can mark records
  exited, apply restart policy, start a replacement process, save the new
  record, and ensure the restart monitor is running.
- Impact: A supposedly read-only status or log request can change runtime state
  by restarting a managed daemon and rewriting supervisor records. Because the
  agent only blocks a new command when the incoming command itself is
  `exclusive`, these read jobs can run while explicit exclusive supervisor work
  is active or while another operator/schedule is inspecting the same process.
  In production, routine status/log checks can therefore trigger surprising
  daemon restarts, PID/log/cgroup changes, and races with start/stop/restart
  jobs, violating the frontend/CLI/API expectation that reads are observational.
- Evidence: `process_status` maps loaded records through
  `reconcile_and_save_record` at `crates/agent/src/supervisor.rs:231-244`.
  `process_logs` tails files, then calls `reconcile_and_save_record` at
  `crates/agent/src/supervisor.rs:255-286`. That helper calls
  `reconcile_record`, `save_record`, and `ensure_restart_monitor` at
  `crates/agent/src/supervisor.rs:413-417`. `reconcile_record` delegates to
  `maybe_restart_record` at `crates/agent/src/supervisor.rs:420-435`, and
  `maybe_restart_record` can call `start_process` and save the restarted record
  at `crates/agent/src/supervisor.rs:450-498`. The restart monitor also uses the
  same load/reconcile/save path at `crates/agent/src/supervisor.rs:542-557`.
  The protocol marks `process_status` and `process_logs` read-only while
  `process_start`, `process_stop`, and `process_restart` are exclusive at
  `crates/common/src/protocol.rs:1304-1308`. The agent's active-command gate
  rejects only incoming exclusive commands when another exclusive command is
  active at `crates/agent/src/runtime.rs:1049-1064`.
- Notes: This is distinct from AUD-011/AUD-012, which cover supervisor log
  bounds and record durability, AUD-123, which covers process inventory scope,
  AUD-141, which covers PID reuse, and AUD-142, which covers local record/log
  permissions. A clean fix should make status/log reads observational, move
  restart reconciliation into the background monitor or an explicit mutating
  command, and serialize supervisor mutations per process.

### AUD-157: Client And Gateway Lifecycle Histories Have No Retention Path

- Severity: Medium/High
- Status: Confirmed
- Area: API/Gateway/Client Lifecycle/Retention
- Context: Normal 20+ VPS operation records agent status transitions and
  gateway session starts/ends whenever agents reconnect, go offline, are
  replaced, or lifecycle actions revoke/delete identities.
- Root Cause: `client_status_history` and `gateway_sessions` are append-style
  lifecycle/history tables, but the history-retention domain model has no
  domain for either table and repository pruning never deletes them.
- Impact: Reconnect churn, agent restarts, gateway deploys, offline sweeps, key
  rotations, and client lifecycle operations can grow these tables indefinitely.
  In production this can bloat the control-plane database and indexes, slow
  gateway-session and lifecycle diagnostics, and leave stale lifecycle evidence
  outside the operator-visible retention controls that cover adjacent audit,
  telemetry, job-output, system-metric, backup, and network histories.
- Evidence: The schema creates `client_status_history` at
  `migrations/0001_identity_access.sql:71-87` and `gateway_sessions` at
  `migrations/0001_identity_access.sql:143-161`. Status transition writes are
  appended by `record_client_status_transition_in_tx` at
  `crates/api/src/repository_ingest.rs:1517-1541`, and gateway session
  start/end writes are appended at
  `crates/api/src/repository_gateway_sessions.rs:69-107` and
  `crates/api/src/repository_gateway_sessions.rs:186-205`. `HistoryDomain`
  enumerates only audit logs, system metrics, telemetry rollups, job outputs,
  backup artifacts, network observations, and topology history at
  `crates/api/src/model_history.rs:3-23`. The prune implementation only matches
  those domains at `crates/api/src/repository_history.rs:844-930` and has no
  branch for lifecycle status history or gateway sessions.
- Notes: This is distinct from AUD-150, which covers stale displaced sessions
  still forwarding telemetry, and from AUD-113/AUD-114/AUD-145, which cover
  key-lifecycle invalidation and disconnect ordering. A clean fix should add
  explicit retention domains for client lifecycle history and ended gateway
  sessions, or another bounded purge policy with visible counts and audit
  evidence. Active gateway sessions must not be pruned.

### AUD-158: Webhook Events In The Default Partition Bypass Event Retention

- Severity: Medium/High
- Status: Confirmed
- Area: API/Worker/Webhooks/Retention
- Context: Webhook events are created from normal agent status changes,
  schedule events, alert reads, job lifecycle changes, and manual dry runs. The
  webhook worker creates date-named event partitions and drops old partitions as
  the event-retention mechanism.
- Root Cause: The schema includes a `webhook_events_default` partition, but
  retention only drops date-named partitions matching `webhook_events_YYYYMMDD`.
  Some normal insert paths write webhook events inside existing transactions
  without first creating the date partition, so those rows can land in the
  default partition and are never selected by the partition-drop retention path.
- Impact: During normal operations, especially after a fresh deployment, worker
  downtime, or a day boundary before the worker has created the new partition,
  status-change webhook events can accumulate in the default partition
  indefinitely. In a 20+ VPS fleet with reconnects, offline sweeps, updates, and
  schedule activity, this can grow the control-plane database outside the
  operator-visible webhook retention setting and make webhook event processing
  and database maintenance increasingly expensive.
- Evidence: The schema partitions `webhook_events` by `occurred_at` and creates
  `webhook_events_default` at
  `migrations/0003_telemetry_alerts_history.sql:277-295`. The worker creates
  today's and tomorrow's date partitions at
  `crates/worker/src/webhook_rules.rs:846-853`, and retention only drops tables
  whose names match `^webhook_events_[0-9]{8}$` at
  `crates/worker/src/webhook_rules.rs:871-902`. API status-transition webhook
  events are inserted directly into `webhook_events` inside the existing
  transaction at `crates/api/src/repository_ingest.rs:1568-1628`, and gateway
  session status transitions call that path at
  `crates/api/src/repository_gateway_sessions.rs:124` and
  `crates/api/src/repository_gateway_sessions.rs:234`. The repository helper
  used by other event sources explicitly creates the matching partition before
  insert at `crates/api/src/repository_webhook_rules.rs:422-424` and
  `crates/api/src/repository_webhook_rules.rs:1058-1075`, showing that direct
  in-transaction status events are the inconsistent path.
- Notes: This is distinct from AUD-157 because named webhook-event partitions do
  have a retention path; the leak is specifically rows routed to the default
  partition. A clean fix should ensure partitions exist before transactional
  inserts that can emit webhook events, or add a safe default-partition drain
  that moves rows into date partitions or prunes old processed default rows with
  visible audit evidence.

### AUD-159: Webhook Permanent-Failure Deliveries Bypass Delivery Retention And Create Unbounded Alerts

- Severity: Medium/High
- Status: Confirmed
- Area: Worker/Webhooks/Retention/Alerts
- Context: Webhook-rule delivery is an integration path for fleet events. A
  misconfigured, retired, or temporarily broken receiver can make many event
  deliveries exhaust the retry budget and become permanently failed.
- Root Cause: `webhook_rule_deliveries` has a terminal
  `permanently_failed` status, and the worker creates a fleet alert for each
  permanently failed delivery. The webhook delivery retention job deletes only
  `delivered` and retryable `failed` rows, so terminal permanent failures are
  excluded from the retention setting.
- Impact: A single bad webhook endpoint can create an unbounded number of
  permanently failed delivery rows and matching open fleet-alert rows, even
  while operators believe webhook retention is bounding old integration
  history. In production this can flood the alert view, inflate database and
  index size, and make integration health harder to triage because every event
  produces a separate durable failure artifact.
- Evidence: The delivery schema allows `permanently_failed` at
  `migrations/0003_telemetry_alerts_history.sql:303-325`. The worker marks a
  delivery `permanently_failed` after the retry budget at
  `crates/worker/src/webhook_rules.rs:741-755`, updates the row at
  `crates/worker/src/webhook_rules.rs:756-784`, and creates a per-delivery
  fleet alert when that status is recorded at
  `crates/worker/src/webhook_rules.rs:789-792` and
  `crates/worker/src/webhook_rules.rs:989-1035`. Retention deletes only
  `status IN ('delivered', 'failed')` at
  `crates/worker/src/webhook_rules.rs:906-950`.
- Notes: This is distinct from AUD-117, which covers alert-notification webhook
  deliveries not retrying automatically, and from AUD-158, which covers event
  partition retention. A clean fix should decide whether permanent-failure
  evidence is retained until operator acknowledgement, aggregated per rule, or
  pruned by the same webhook retention policy after sufficient visibility. The
  chosen behavior should also clean up or resolve the associated
  `webhook_delivery:<id>` fleet-alert states consistently.

### AUD-160: Webhook-Rule Retention Silently Clamps The Shipped 90-Day Setting To 7 Days

- Severity: Medium/High
- Status: Confirmed
- Area: Worker/Webhooks/Retention/Config
- Context: Operators configure webhook-rule retention through the suite config
  and shipped deployment template. Those retention settings govern webhook event
  partitions and delivered/failed webhook-rule delivery evidence.
- Root Cause: The worker command-line default and shipped suite config set
  `webhook_rule_retention_days` to 90, but `WebhookRuleWorkerConfig::new`
  clamps the effective value to a maximum of 7 without surfacing a validation
  error or operator-visible warning.
- Impact: A production deployment that appears configured to retain webhook
  integration evidence for 90 days actually drops date-named webhook event
  partitions and delivered/failed delivery rows after 7 days. Operators can lose
  forensic and integration troubleshooting evidence much earlier than the
  deployed config says, while audits and UI/config review still show a 90-day
  intent. This is practical in 20+ VPS operation where webhook events and
  deliveries are part of incident reconstruction and external notification
  debugging.
- Evidence: The shipped suite config sets `webhook_rule_retention_days = 90` at
  `deploy/config/vpsman.toml:40-43`, and the worker CLI default is also 90 at
  `crates/worker/src/main.rs:111-116`. Suite config applies that value into the
  worker runtime args at `crates/worker/src/main.rs:325-329`, but the effective
  runtime config clamps it with `retention_days.clamp(1, 7)` at
  `crates/worker/src/webhook_rules.rs:37-50`. That clamped value is then used
  for partition drops at `crates/worker/src/webhook_rules.rs:871-902` and
  delivery pruning at `crates/worker/src/webhook_rules.rs:906-950`.
- Notes: This is distinct from AUD-158 and AUD-159. Those cover retention rows
  that are missed entirely. This issue covers rows that are eligible for
  retention but are pruned according to a silently shortened retention window.
  A clean fix should align the shipped default, suite-config validation, UI/help
  text, and worker clamp, preferably by rejecting out-of-range retention rather
  than silently changing it.

### AUD-161: Artifact Cleanup Server Jobs Can Remain Running Forever After Worker Loss

- Severity: High
- Status: Confirmed
- Area: Worker/Server Jobs/Artifact Cleanup
- Context: Artifact cleanup is a destructive server-side maintenance workflow
  that can delete retained job-output, file-transfer, and backup artifact
  evidence. Operators track it through `server_jobs` and may cancel queued
  cleanup before it starts.
- Root Cause: The worker claims one queued artifact-cleanup server job by
  setting `status = 'running'` and `started_at = now()`, but `server_jobs` has
  no lease owner, lease deadline, heartbeat, business timeout, or reclaim path.
  The cancel route only cancels queued server jobs, not running ones.
- Impact: If the worker process, container, host, or database connection is lost
  after the job is marked running, the server job can remain active forever.
  Operators cannot cancel it through the API, later workers will not reclaim it,
  and the job list can permanently report destructive maintenance as still
  running. If the crash happened after some candidates were marked `deleting`,
  this also obscures whether object cleanup is incomplete or merely stuck.
- Evidence: The schema stores `server_jobs.status`, `started_at`, and
  `completed_at`, but no lease or timeout fields at
  `migrations/0002_jobs_schedules_commands.sql:201-220`. The worker claims only
  queued artifact cleanup jobs and updates them to running at
  `crates/worker/src/main.rs:1153-1176`, then processes the job outside any
  durable server-job lease at `crates/worker/src/main.rs:1094-1150` and
  `crates/worker/src/main.rs:1185-1210`. Later worker ticks call the same claim
  path, which filters only `status = 'queued'`. Server-job cancellation updates
  only `status = 'queued'` rows at
  `crates/api/src/repository_server_jobs.rs:266-313`.
- Notes: This is distinct from AUD-155, which covers misleading counts when a
  cleanup job returns an error after partial deletion. This issue covers worker
  loss before the success/error finalization path runs at all. A clean fix
  should add a visible in-progress lease/attempt model for server jobs or a
  stale-running recovery path that marks the job failed/abandoned with partial
  evidence and allows an operator to retry or cancel safely.

### AUD-162: Update-Check Activation Can Downgrade Agents From An Older Release Manifest

- Severity: High
- Status: Confirmed
- Area: Agent/Updates/Safety
- Context: Operators can run manual `agent_update_check` jobs from the dashboard
  or CLI, and can enable the autonomous updater. These workflows normally point
  to the official GitHub `releases/latest/download/version.json`, but the
  manifest URL is configurable for channels, staged rollouts, mirrors, and
  operator-controlled release hosts.
- Root Cause: The agent update-check path treats any manifest version that is
  not byte-for-byte equal to the embedded agent release version as an update
  candidate. It does not parse versions, compare ordering, or require an
  explicit downgrade/rollback operation before staging and optional activation.
- Impact: If an operator accidentally supplies a stale pinned manifest URL, a
  mirror lags behind, a channel URL points at an older release, or `latest` is
  temporarily inconsistent, the autonomous updater or a dashboard/CLI
  "activate if newer" job can replace agents with an older binary. That bypasses
  the explicit rollback workflow and is practical in fleet operation because
  the same manifest URL can be reused across many VPSs.
- Evidence: `crates/agent/src/update.rs:224-240` reads the embedded current
  version and returns `current` only when the manifest version string is exactly
  equal. Any different version continues into asset selection, checksum
  verification, staging, and optional activation at
  `crates/agent/src/update.rs:256-318`. The dashboard labels this operation
  "Activate if newer" at
  `frontend/src/panels/jobs/JobOperationControls.tsx:739-745`, and the agent
  config example says the autonomous updater stages newer artifacts at
  `docs/agent-config.example.toml:29-35`. Existing updater tests cover equal
  version current detection and a newer-looking candidate, but not older
  manifest rejection.
- Notes: This is distinct from AUD-144, which covers strict release-registry
  bypass. A clean fix should make update-check/autonomous update compare the
  candidate release version against the embedded current release version and
  report a visible `older` or `downgrade_blocked` status unless an explicit
  rollback/downgrade command is used. If non-semver release identifiers remain
  valid, define conservative behavior instead of treating every unequal string
  as newer.

### AUD-163: Custom JSON Command Timeouts Can Be Bypassed After Stdout Closes

- Severity: High
- Status: Confirmed
- Area: Agent/Custom Runtime Commands/Reliability
- Context: Operators can configure custom JSON-producing commands for process
  inventory, custom telemetry metrics, and runtime tunnel traffic telemetry.
  The config model gives those commands explicit timeout and output-size
  budgets, and docs describe them as bounded commands for nonstandard images,
  providers, and runtime adapters.
- Root Cause: Several direct-spawn custom JSON command runners apply the
  timeout only while reading stdout, then await child process exit without a
  timeout or command-cancel path. If the command emits valid JSON, closes or
  redirects stdout, and then keeps running, the bounded stdout read completes
  before the timeout and the later `child.wait().await` can wait forever.
- Impact: A malformed, daemonizing, or provider-supplied wrapper can stall the
  agent beyond the configured timeout. For process-list jobs, the target can
  remain active on the agent even after the API eventually records a control
  timeout, and operator cancellation cannot interrupt the process-list wait.
  For custom telemetry or runtime traffic telemetry, the main agent loop awaits
  metrics collection before reading more frames or sending telemetry, so one
  hung custom telemetry command can stop that agent from processing commands
  or heartbeats until the process is restarted. In a 20+ VPS fleet, one bad
  custom source should produce a failed sample/job, not wedge the agent event
  loop.
- Evidence: `RuntimeTunnelCommand` exposes `timeout_secs` and
  `max_output_bytes` at `crates/common/src/network/models.rs:132-139`, and
  config validation enforces those budgets at
  `crates/common/src/config/validation.rs:439-448`. The agent config example
  presents process inventory and custom metrics commands as bounded at
  `docs/agent-config.example.toml:57-66` and
  `docs/agent-config.example.toml:80-82`. Process-list dispatch receives a
  cancel token at `crates/agent/src/executor.rs:98-106`, but the
  `ProcessList` branch calls `execute_process_list` without passing that token
  at `crates/agent/src/executor.rs:345-350`; cancel frames only set the active
  command token at `crates/agent/src/runtime.rs:1226-1235`. The process
  inventory runner bounds `read_limited_stdout` but then waits unbounded at
  `crates/agent/src/process.rs:247-278`. The custom telemetry runner has the
  same stdout-timeout-then-unbounded-wait shape at
  `crates/agent/src/telemetry_custom.rs:77-87`, and runtime traffic telemetry
  does the same at `crates/agent/src/telemetry_traffic.rs:193-199`. The main
  agent loop awaits `collect_metrics_for_config` before returning to frame
  processing at `crates/agent/src/runtime.rs:162-170`, and metrics collection
  awaits both custom metrics and runtime status telemetry at
  `crates/agent/src/telemetry.rs:72-83`.
- Notes: This is distinct from ordinary timeout expiry where stdout remains
  open; that path is bounded. The practical failure is a command that closes
  stdout early but does not exit, which is common for buggy shell wrappers,
  daemonizing helpers, or scripts that redirect output after printing JSON. A
  clean fix should run the whole child lifecycle under the same timeout and
  cancellation budget, kill the child on timeout/cancel/output-limit, and
  return a visible failed telemetry sample or command output instead of
  blocking the agent task.

### AUD-164: Process Supervisor Stop And Restart Can Mutate Host State After Command Timeout

- Severity: High
- Status: Confirmed
- Area: Agent/Process Supervisor/Timeouts
- Context: Operators can start supervised processes from the dashboard or CLI
  with a stored stop policy, then later issue `process_stop` or
  `process_restart` jobs. The API accepts `graceful_stop_secs` up to 300
  seconds, so this is practical for production daemons that need longer
  graceful shutdown windows.
- Root Cause: The agent wraps the whole process-supervisor command in a
  60-second async timeout, but the actual supervisor work runs inside
  `tokio::task::spawn_blocking`. When the async timeout expires, Rust drops the
  join handle but cannot cancel the blocking thread. That thread can continue
  sending signals, waiting through the stored graceful-stop window, writing
  supervisor records, and in the restart case starting a new process after the
  command has already reported a timeout.
- Impact: A stop or restart job can appear failed or timed out while the agent
  still mutates the host afterward. Operators may dispatch follow-up work,
  retry, or investigate based on a terminal timeout/failure, only for the old
  blocking worker to later kill or restart the service and update local
  supervisor state. At 20+ VPS scale this creates a real operational safety
  problem: service state, job output, and audit timing no longer explain why a
  process stopped or restarted.
- Evidence: `ProcessRunPolicy.graceful_stop_secs` defaults to 5 but is part of
  the persisted process policy at `crates/common/src/protocol.rs:771-789`.
  API validation accepts values from 1 to 300 seconds at
  `crates/api/src/job_request.rs:782-797`, and CLI/VTY parsing exposes the same
  range in `crates/vpsctl/src/vty_process.rs:235-244`.
  `execute_process_supervisor_command` applies
  `time::timeout(Duration::from_secs(timeout_secs.clamp(1, 60)),
  execute_at_root(...))` at `crates/agent/src/supervisor.rs:76-84`, while
  `execute_at_root` runs `execute_blocking` through `spawn_blocking` at
  `crates/agent/src/supervisor.rs:147-154`. `ProcessStop` and
  `ProcessRestart` both call `stop_record` at
  `crates/agent/src/supervisor.rs:190-215`. `stop_record` waits for the stored
  `graceful_stop_secs` and can then run a second fallback wait at
  `crates/agent/src/supervisor.rs:360-387`; the blocking cleanup loop sleeps
  until the deadline in `crates/agent/src/process_cleanup.rs:38-118`. For
  `ProcessRestart`, the same blocking command calls `start_process` and saves a
  new record after the stop phase.
- Notes: This is distinct from AUD-141, which covers stale PID reuse after an
  agent restart; AUD-156, which covers read-side status/log commands restarting
  processes; and AUD-163, which covers custom JSON commands waiting forever
  after stdout closes. A clean fix should make supervisor stop/restart
  cancellation and timeout authoritative: the blocking cleanup budget must fit
  inside the command deadline, or the operation must run through a cancellable
  worker that cannot keep mutating process state after the target has timed
  out.

### AUD-165: Managed Network Rollback Rewrites Files Non-Atomically And Drops Original Modes

- Severity: High
- Status: Confirmed
- Area: Agent/Network Apply/Rollback
- Context: Network apply, OSPF cost update, and operator rollback write
  managed ifupdown, netplan, systemd-networkd, and Bird2 snippets, then run
  validation and reload hooks. If validation, reload, or deadline checks fail,
  the agent attempts to roll managed files back before returning an error.
- Root Cause: The successful apply path uses a temp file and rename, but the
  rollback path restores previous contents with direct `tokio::fs::write`.
  The rollback record stores only bytes, not original metadata, and the direct
  write truncates/recreates the live managed file in place without restoring
  the original mode or using the same atomic replace helper.
- Impact: A network apply failure can leave production network configuration
  files partially written, truncated, or with different filesystem modes during
  the exact failure path where operators rely on rollback to recover
  connectivity. This is practical at fleet scale because validation and reload
  hooks are normal production controls and can fail due to syntax errors,
  provider state, daemon failures, package differences, or deadline expiry.
  The job can report failure while the host is left with a corrupted or
  mode-changed managed config file that requires manual repair before another
  reload or reboot.
- Evidence: `apply_updates_with_rollback` calls `rollback_updates` after write,
  deadline, validation, or reload failures at
  `crates/agent/src/network_apply.rs:682-705`. The apply write path uses
  temp-file plus rename and sets `0o644` at
  `crates/agent/src/network_apply.rs:981-1005`, while `rollback_updates`
  restores prior contents with direct `tokio::fs::write` and removes absent
  files best-effort at `crates/agent/src/network_apply.rs:1007-1014`. The
  backup path records content only and writes a separate `0o600` backup file at
  `crates/agent/src/network_apply.rs:908-921`; `PlannedFileUpdate` has no
  previous mode/owner metadata. Existing tests cover content rollback after a
  validation hook failure at `crates/agent/src/network_apply/tests.rs:370-427`
  but do not assert atomic rollback or mode preservation.
- Notes: This is distinct from frontend network confirmation issues
  AUD-031/AUD-032, canonical OSPF state AUD-013, and network read-scope issues
  AUD-078/AUD-079. A clean fix should preserve previous metadata for managed
  files and use an atomic restore/remove path for rollback, with tests that
  cover validation-hook failure, reload-hook failure, and deadline failure.

### AUD-166: Duplicate Resumable Download Chunks Can Poison Server-Side Handoff

- Severity: Medium/High
- Status: Confirmed
- Area: API/File Transfers/Reliability
- Context: Operators can use resumable file-transfer downloads to pull larger
  files from a VPS in chunks, then create a server-side handoff artifact from
  the retained chunk outputs. Retrying a chunk at the same offset is a normal
  resumable-transfer behavior during browser refreshes, CLI retry, transient
  gateway/API failures, or uncertain operator recovery after a partially
  completed download.
- Root Cause: Handoff assembly treats every retained
  `file_transfer_download_chunk` job for the session as a required sequential
  chunk. It sorts chunks by offset and job ID, then rejects the first chunk
  whose offset is not exactly the next expected offset. It does not collapse
  duplicate offset chunks, compare duplicate bytes/hashes, or choose one
  valid representative for each offset.
- Impact: A valid retry of an already downloaded chunk can make the whole
  server-side handoff fail with `file_transfer_handoff_chunk_gap`, even when
  all byte ranges needed to reconstruct the file are present and hash-valid.
  The session view can still show the transfer as completed and handoff
  available, but the operator cannot create the retained artifact without
  manually cleaning history or starting a new session. This is practical in
  production because resumable downloads are explicitly designed for partial
  progress and retry, and fleet operators commonly retry after browser,
  gateway, or network uncertainty.
- Evidence: The API lists all retained download chunk outputs for the client
  and session in `list_file_transfer_download_handoff_chunks` at
  `crates/api/src/repository_file_transfers.rs:208-218`, then
  `build_file_transfer_download_handoff_chunks` groups by job, pushes one
  chunk record per chunk job, and sorts only by offset and job ID at
  `crates/api/src/repository_file_transfers.rs:678-717`. The handoff writer
  then requires strict sequential offsets at
  `crates/api/src/routes_file_transfers.rs:492-497`. The frontend download
  loop issues separate `file_transfer_download_chunk` jobs with the same
  session ID and current offset at
  `frontend/src/resumableFileTransfer.ts:422-431`; retrying after an uncertain
  result can therefore leave multiple valid chunk jobs for the same offset.
- Notes: This is distinct from AUD-105, which covers derived session records
  outliving the job-output evidence they depend on, and from AUD-131, which
  covers path/symlink safety while reading. A clean fix should make handoff
  assembly idempotent by grouping chunks by offset, accepting identical
  duplicate chunks, rejecting conflicting duplicates explicitly, and assembling
  exactly one contiguous byte range for each offset before marking a handoff
  available or creating the object-store artifact.

### AUD-167: Migration-Link Creation Bypasses Request-Bound Privilege Verification

- Severity: Medium/High
- Status: Confirmed
- Area: API/Backups/Migrations/Privilege
- Context: Operators create migration links to bind a metadata-only restore
  plan into a later migration workflow. The link records source backup ID,
  source and target VPS IDs, restore paths, config inclusion, destination root,
  notes, and the restore plan identity.
- Root Cause: Creating a migration link requires only an authenticated
  operator with `backups:write` plus `confirmed = true`. The route does not
  carry or verify a request-bound privilege assertion for the exact restore
  plan being linked, even though creating the adjacent restore plan does verify
  a restore `JobPrivilegeIntent`.
- Impact: A token with backup-write authority but without the local privilege
  secret can create durable migration state for an already planned restore.
  This can produce false migration evidence, make audit trails imply an
  approved migration step, and consume the unique migration-link slot for that
  restore plan so the normal bundled migration workflow cannot be retried
  cleanly. In production migrations, that is enough to block or confuse a
  cutover even if it does not directly execute host mutation.
- Evidence: `create_migration_link` authorizes with role `operator` and
  `backups:write`, validates only `confirmed` and note length, loads the
  restore plan, and records the link at
  `crates/api/src/routes_migrations.rs:27-57`. The validator only checks
  `request.confirmed` at `crates/api/src/routes_migrations.rs:62-75`. In
  contrast, restore-plan creation computes a restore command payload hash and
  calls `verify_privilege_intent` at
  `crates/api/src/routes_restores.rs:51-83`. The schema allows only one link
  per restore plan with `restore_plan_id UUID NOT NULL UNIQUE` in
  `migrations/0004_backups_restores.sql:66-79`.
- Notes: This is distinct from AUD-047, which covers read-scope leakage for
  migration-link listings, and AUD-110, which covers bundled migration-run
  ordering. A clean fix should require a DB privilege assertion bound to the
  migration link action, restore plan ID, source backup ID, source/target VPSs,
  destination root, paths, and config-restore flag.

### AUD-168: Chunked Backup Artifact Commit Rehydrates The Whole Artifact In API Memory

- Severity: Medium/High
- Status: Confirmed
- Area: API/Backups/Resource Bounds
- Context: Operators use chunked backup artifact upload when an encrypted
  backup artifact is too large for the inline upload route. This is the
  practical path for retained backup artifacts and restore material in real
  fleet operation.
- Root Cause: The chunked upload path limits individual chunks and stages
  bytes on disk, but the commit path reads the entire staged artifact back into
  a `Vec<u8>` for validation. The validation then parses the whole artifact
  JSON and decodes the full `ciphertext_base64` field into another in-memory
  byte buffer before committing the object.
- Impact: A feature intended to avoid large request bodies can still allocate
  the full artifact, plus decoded ciphertext, in API memory at commit time.
  With the default 128 MiB chunked-artifact limit, one or a few concurrent
  commits can create large memory spikes; if operators raise
  `VPSMAN_BACKUP_HANDOFF_MAX_BYTES`, the spike scales with that setting. At
  20+ VPS scale, backup artifacts are expected operational data, so this can
  turn normal artifact ingestion into API memory pressure or OOM risk instead
  of a bounded streaming workflow.
- Evidence: Chunked upload sessions allow staged artifacts up to
  `backup_artifact_streaming_max_bytes()` at
  `crates/api/src/backup_upload_sessions.rs:344-365`, with a default of
  `MAX_BACKUP_ARTIFACT_CHUNKED_UPLOAD_BYTES = 128 * 1024 * 1024` at
  `crates/api/src/backup_handoff.rs:10-12` and environment override handling
  at `crates/api/src/backup_handoff.rs:121-127`. The commit preparation reads
  the whole staged file with `tokio::fs::read` and passes the full buffer to
  validation at `crates/api/src/backup_upload_sessions.rs:208-219`. The
  validation parses the full JSON and decodes the full ciphertext base64 at
  `crates/api/src/routes_backups.rs:1102-1145`.
- Notes: This is distinct from AUD-082, which covers temporary file
  permissions, and AUD-062, which covers artifact metadata/cleanup registry
  consistency. A clean fix should validate staged artifacts with a bounded
  streaming or incremental parser/hash path, or keep the chunked-artifact
  maximum low enough that the memory cost is explicitly within the API's
  resource budget and concurrency controls.

### AUD-169: Agent Backups Can Be Valid Above The API Restore-Preparation Inline Limit

- Severity: Medium/High
- Status: Fixed
- Area: API/Backups/Restore Workflow
- Context: Operators can configure agents to back up more than 1 MiB of
  plaintext, then later use the normal dashboard or CLI restore-preparation
  workflow to decrypt the stored artifact and build a restore job.
- Root Cause: The agent backup configuration accepts
  `backup.max_plaintext_bytes` up to 16 MiB, and the backup command enforces
  that configured limit while producing a valid encrypted backup artifact. The
  API restore-preparation endpoint decrypts the artifact and then requires the
  restored archive to fit the shared inline file-push limit of 1 MiB. The
  restore command validator also requires inline restore archives to pass that
  same 1 MiB inline payload limit unless the operator manually supplies an
  agent-local archive path.
- Impact: An operator can create a fully valid backup using supported agent
  configuration and then be unable to restore it through the normal API,
  dashboard, or CLI preparation path. This is practical for production because
  backing up application configs, service data, or multiple files can exceed
  1 MiB while still being well within the documented and validated agent backup
  limit. The failure appears only during restore preparation, which is the
  worst point operationally because the operator may discover it during a
  recovery or migration window.
- Evidence: Agent config validation accepts `max_plaintext_bytes` in
  `1..=16 * 1024 * 1024` at
  `crates/common/src/config/validation.rs:129-136`, while the default is
  1 MiB at `crates/common/src/config/models.rs:331-336`. The agent backup
  command collects and encodes the tar archive up to
  `config.backup.max_plaintext_bytes` at
  `crates/agent/src/backup.rs:185-214` and
  `crates/agent/src/backup.rs:372-402`. API restore preparation decrypts and
  decompresses the artifact, then rejects archives larger than
  `MAX_INLINE_FILE_PUSH_BYTES` at
  `crates/api/src/backup_artifact_crypto.rs:83-89`; that constant is 1 MiB at
  `crates/common/src/file_transfer.rs:7-8`. The restore command validation
  calls `validate_inline_file_payload` for `archive_base64` at
  `crates/api/src/job_request.rs:516-524`, and the inline validator rejects
  payloads above the same 1 MiB cap at
  `crates/common/src/file_transfer.rs:109-124`.
- Notes: This is distinct from AUD-092, which covers agent-local restore
  archive memory behavior, and AUD-168, which covers chunked artifact commit
  memory pressure. A clean fix should make the backup and restore size model
  consistent: either cap agent backups to the supported API restore size, or
  add a supported retained/object-backed restore artifact path so valid larger
  backups can be restored without forcing operators into manual agent-local
  staging.
- Fix Notes: Removed the API/dashboard restore-preparation path and inline
  restore archive transport. Restore now requires one operator-staged
  agent-local archive file with size and SHA-256 metadata, so the former API
  inline restore-preparation size ceiling no longer blocks valid larger backup
  archives.

### AUD-170: Dashboard Restore Preparation Sends The Backup Private Key To The API

- Severity: High
- Status: Fixed
- Area: Frontend/API/Backups/Key Custody
- Context: Operators restore encrypted backup artifacts from the dashboard.
  The dashboard presents the workflow as browser-held key material and
  browser-decrypted restore preparation, while the CLI keeps the private key
  local by reading it from an environment variable and decrypting before job
  creation.
- Root Cause: The dashboard posts `private_key_hex` to the API
  `/artifact/prepare-restore` route. The API accepts that key, loads or accepts
  the encrypted artifact, decrypts it server-side, and returns an inline
  plaintext restore archive to the browser. This is inconsistent with the CLI
  restore path, which decrypts locally before building the restore command.
- Impact: Backup private keys cross the operator browser/API boundary and are
  exposed to API request handling, middleware, crash dumps, reverse proxies,
  request captures, and any future logging or tracing around request bodies.
  Backups can include private service config, credentials, and identity
  material, so the decryption key should remain operator-local unless the
  product explicitly documents and designs for server-side key custody. In
  production this can surprise operators who choose browser restore expecting
  the key to stay in browser memory, especially because the CLI path already
  demonstrates the safer local-decrypt model.
- Evidence: The dashboard builds the restore snapshot by calling
  `onPrepareBackupArtifactRestore` with `private_key_hex` at
  `frontend/src/panels/BackupsPanel.tsx:866-875`. The API hook posts that body
  to `/api/v1/backups/{id}/artifact/prepare-restore` at
  `frontend/src/hooks/useBackupsData.ts:220-227`. The route requires
  `backups:write`, checks only that the key is non-empty, and passes the key
  to `prepare_backup_archive_for_restore` at
  `crates/api/src/routes_backups.rs:806-839`. The form text says the workflow
  runs "browser-decrypted" restores at
  `frontend/src/panels/backups/RestoreRunForm.tsx:58-61`. The CLI restore
  path reads `private_key_env`, decrypts locally, and then builds the restore
  command at `crates/vpsctl/src/commands_backups.rs:967-982`.
- Notes: This is distinct from AUD-027, which covers backup read-scope
  boundaries, and AUD-169, which covers restore size mismatch. A clean fix
  should make browser restore decrypt locally or explicitly remove the
  server-side prepare route for private-key material.
- Fix Notes: Removed the API prepare-restore route and dashboard hook/form path
  that accepted `private_key_hex`; restore runs now require an agent-local
  archive path plus size and SHA-256 metadata.

### AUD-171: Inline Restore Archives Persist Decrypted Backup Content In Jobs And Webhooks

- Severity: Critical
- Status: Fixed
- Area: API/Backups/Restore Payloads/Webhooks
- Context: Normal restore runs created by the dashboard or CLI send a
  `restore` job to the API after decrypting a backup artifact. Restored
  archives can contain private configuration, credentials, service keys,
  application state, and agent configuration.
- Root Cause: The restore command model carries `archive_base64` inline.
  Both dashboard and CLI restore paths populate that field with the decrypted
  archive bytes. The API then persists the full `JobCommand` JSON in
  `jobs.operation` and includes the same operation in the `job.created`
  webhook payload.
- Impact: A successful restore request can copy decrypted backup contents into
  durable API database state and queued webhook event payloads. If webhook
  rules are enabled, the decrypted archive can also be delivered to external
  integration endpoints as part of a normal job-created event. Even without
  webhooks, DB backups, history exports, diagnostic dumps, retention gaps, and
  operator read paths can now contain plaintext restore archives that operators
  expected to exist only transiently for dispatch to one target. This is a
  practical production secrecy issue, not only storage overhead, because the
  affected data is exactly backup restore material.
- Evidence: `JobCommand::Restore` includes `archive_base64` at
  `crates/common/src/protocol.rs:2605-2614`. The dashboard sets
  `operation.archive_base64 = artifact.archive_base64` after restore
  preparation at `frontend/src/panels/BackupsPanel.tsx:884-896`. The CLI sets
  `archive_base64: Some(archive_base64)` after local decryption at
  `crates/vpsctl/src/commands_backups.rs:967-982`. The API persists
  `operation.clone()` into `jobs.operation` when recording dispatching jobs at
  `crates/api/src/repository_jobs.rs:1403-1423`. The same repository passes
  `operation: Some(&operation)` into job-created webhook event construction at
  `crates/api/src/repository_jobs.rs:1460-1470`, and the webhook payload
  serializes it under `job.operation` at
  `crates/api/src/repository_jobs.rs:3482-3514`.
- Notes: This is distinct from AUD-019 and AUD-017, which cover read-scope
  exposure of stored operations and webhook payloads. The defect here is that
  decrypted backup payload bytes are stored and emitted at all. A clean fix
  should use a short-lived object-backed restore material reference, redact or
  omit payload-bearing fields from stored job operations and webhook payloads,
  and keep only hashes, sizes, and audit-safe metadata in durable records.
- Fix Notes: Removed `archive_base64` from the shared restore command model,
  API validation, agent execution input, CLI/VTY, dashboard forms, privilege
  canonicalization, and tests. Restore job operations persist only path, size,
  hash, scope, and execution metadata.

### AUD-172: Password Reset Preserves Old-Password-Encrypted TOTP Secrets

- Severity: High
- Status: Fixed
- Area: API/Auth/User Management
- Context: Admin operators can reset another operator's password from
  System > Users, or reset their own password. Existing sessions for the
  target operator are revoked as part of the reset. Operators can also have
  TOTP enabled, and TOTP login verification decrypts the stored TOTP secret
  with the submitted account password.
- Root Cause: Password reset changes only `operators.password_hash` and
  revokes sessions. It does not clear, rewrap, or otherwise invalidate
  `totp_secret_ciphertext_hex`, `totp_secret_nonce_hex`, or
  `totp_secret_salt_hex`. Those fields remain encrypted under the previous
  password, while login after reset attempts to decrypt them with the new
  password.
- Impact: A TOTP-enabled operator whose password is reset can become unable
  to log in with the new password and current TOTP code, because TOTP secret
  decryption fails before code verification. This is production-practical for
  user recovery and access management: resetting a locked-out user's password
  does not actually recover access unless an admin also clears TOTP. If the
  last admin resets their own password while TOTP is enabled, the reset also
  revokes the current session and can leave the deployment without an
  accessible admin account.
- Evidence: `reset_operator_password` validates the new password and calls
  `Repository::reset_operator_password` at
  `crates/api/src/routes_auth.rs:420-438`. The repository method updates only
  `password_hash`, revokes sessions, and returns the operator row at
  `crates/api/src/repository_auth.rs:1378-1454`; it does not modify the TOTP
  secret fields. Login for TOTP-enabled users decrypts
  `operator.encrypted_totp_secret()` with the submitted password and treats a
  decrypt failure as invalid credentials at
  `crates/api/src/repository_auth.rs:259-323`. The dashboard reset flow
  describes only password replacement and session revocation at
  `frontend/src/panels/SystemPanel.tsx:151-158` and
  `frontend/src/panels/SystemPanel.tsx:1059-1089`; TOTP clearing is a
  separate action.
- Notes: A clean fix should make password reset and TOTP state explicit:
  either clear TOTP as part of reset, require an explicit
  reset-and-clear-TOTP option for TOTP-enabled users, or re-encrypt the TOTP
  secret only when the current password is available. The current behavior is
  unsafe because the UI/API present password reset as account recovery while
  preserving unusable TOTP material.
- Resolution: Fixed by clearing TOTP enabled state and encrypted TOTP secret
  material as part of password reset while continuing to revoke target
  sessions; frontend confirmation copy now reflects that behavior.

### AUD-173: Bulk Tag Preview Races Can Apply A Stale Target Set To A Newer Tag Form

- Severity: High
- Status: Fixed
- Area: Frontend/Fleet Tags
- Context: System operators use the Tags bulk mutation panel to preview target
  VPSs and schedule impacts before adding, removing, or deleting tags. Tags are
  part of the normal target-selection model for jobs, schedules, alert scopes,
  and fleet organization, so wrong tag mutations can affect later production
  work.
- Root Cause: The bulk tag panel stores only the latest `preview` response.
  `previewTargets()` reads live `action`, `tag`, and `selectorExpression`,
  starts asynchronous target resolution and preview requests, then writes the
  response back with `setPreview(...)` without checking that the form still
  matches the request that produced the response. Input edits clear the preview
  immediately, but an older in-flight preview can resolve after the edit and
  repopulate `preview` for the new visible form. The confirmed submit path then
  mixes `preview.affected` from the stale response with the current live
  `action`, `tag`, and `selectorExpression`.
- Impact: A real operator can preview `tag=A` on selector `id:vps-a`, edit the
  form to `tag=B` or selector `id:vps-b` while the preview is still in flight,
  and then see the old preview repopulate the panel. Confirming can apply the
  current tag/action to the stale target IDs, or send a selector string that no
  longer corresponds to the fixed IDs. This can mutate tags on the wrong VPSs,
  corrupt schedule-impact review, and cause future jobs or schedules to target
  the wrong machines.
- Evidence: The reviewed bulk tag workflow is implemented in
  `frontend/src/panels/TagsPanel.tsx:596-625`: `clearMutationPreview()` clears
  visible state on edits, but `previewTargets()` has no request generation or
  form-snapshot guard before `setPreview(...)`. Confirmation submission at
  `frontend/src/panels/TagsPanel.tsx:628-669` reads
  `preview?.affected.map(...)` for fixed target IDs while reading live
  `action`, `tag.trim()`, and `selectorExpression.trim()` for the mutation and
  privilege assertion. The backend trusts the fixed target IDs after verifying
  they exist at `crates/api/src/routes_inventory.rs:503-535`, and
  `Repository::bulk_mutate_tags` applies the requested tag/action to exactly
  those target IDs at `crates/api/src/repository_inventory.rs:402-548`.
- Notes: This is distinct from AUD-041, which covers inline fleet tag controls
  bypassing the reviewed bulk workflow entirely. This issue is inside the
  reviewed bulk workflow and is practical whenever the operator edits while a
  preview request is still in flight. A clean fix should store a frozen preview
  snapshot or request generation containing action, tag, selector, target IDs,
  and schedule impacts; ignore stale async preview responses; and submit only
  the reviewed snapshot.
- Fix: Bulk tag preview now uses a review generation guard, discards stale
  async preview completions, and opens confirmation from a frozen mutation
  snapshot containing action, tag, selector, preview targets, and schedule
  impacts. Covered by `dispatch-target-consistency.spec.ts`.

### AUD-174: Artifact Cleanup Preview Races Can Queue A Stale Cleanup Set After Expression Edits

- Severity: High
- Status: Fixed
- Area: Frontend/Server Jobs/Artifact Cleanup
- Context: Server artifact cleanup is a destructive maintenance workflow. It
  deletes retained artifact objects and metadata for domains such as job
  output, backups, update artifacts, and file-transfer artifacts. The backend
  now snapshots the reviewed candidate set into
  `server_job_artifact_cleanup_targets`, so the frontend review flow is the
  operator's main guard against deleting the wrong retained evidence.
- Root Cause: The server-jobs panel stores a single `preview` object and does
  not bind it to the expression that was current when the async preview request
  started. Editing the expression clears the preview immediately, but an older
  in-flight preview can resolve afterward and repopulate `preview` for the old
  expression. The panel then enables `Queue cleanup` and submits
  `preview.expression` plus `preview.preview_hash`, even though the visible
  textarea may now contain a different cleanup expression.
- Impact: An operator can preview one cleanup expression, edit the expression
  while the preview request is still in flight, and then have the old preview
  reappear as a queueable cleanup. Because cleanup jobs persist the reviewed
  candidate set and workers delete those objects later, this can queue deletion
  of a stale artifact set that no longer matches the operator's visible intent.
  In production this can remove retained job-output or backup evidence from
  the wrong domain or time window.
- Evidence: `frontend/src/panels/jobs/ServerJobsPanel.tsx:47-63` starts
  `onPreviewCleanup(expression)` and unconditionally writes the result with
  `setPreview(...)`. Expression edits at
  `frontend/src/panels/jobs/ServerJobsPanel.tsx:129-136` clear `preview` and
  `confirmOpen`, but they do not cancel or generation-guard the in-flight
  preview. `queueCleanup()` at
  `frontend/src/panels/jobs/ServerJobsPanel.tsx:65-84` submits
  `preview.expression` and `preview.preview_hash`; the hook forwards those
  fields to `/api/v1/server-jobs/artifact-cleanup` at
  `frontend/src/hooks/useJobsData.ts:503-512`. The backend verifies the hash
  and stores snapshot rows for that supplied expression at
  `crates/api/src/repository_server_jobs.rs:105-213`.
- Notes: This is distinct from AUD-050. AUD-050 covered backend cleanup jobs
  re-evaluating expressions at execution time instead of deleting the reviewed
  set. This issue is a frontend review race that can queue the wrong reviewed
  set before the backend snapshot is created. A clean fix should tie preview
  responses to a frozen expression or request generation, ignore stale async
  preview responses, and submit only a confirmation snapshot whose expression,
  preview hash, matched count, bytes, and reviewed artifacts match the visible
  confirmation.
- Fix: Artifact cleanup preview now freezes the expression for the preview
  request, ignores stale preview completions after expression edits, and queues
  cleanup only from the current reviewed preview hash/expression. Covered by
  `dispatch-target-consistency.spec.ts`.

### AUD-175: Dispatch Review Can Open A Stale Confirmation After Operation Or Selector Edits

- Severity: High
- Status: Fixed
- Area: Frontend/Job Dispatch
- Context: The Jobs dispatch composer is the primary operator path for shell,
  file, backup, config, process-supervisor, terminal, and agent-update jobs.
  It intentionally freezes a confirmation snapshot before dispatching so that
  reviewed selectors, target IDs, payload hashes, timeouts, and privilege
  assertions match the submitted job.
- Root Cause: `submitJob()` starts async backend target resolution and builds
  a dispatch confirmation after the await. Form edits close an already-open
  prompt, but there is no request generation or form-change guard for an
  in-flight review request. Because React closures preserve the form values
  from the moment `Review dispatch` was clicked, the response opens a frozen
  confirmation for that older selector and operation even if the operator has
  already edited the visible composer to a different selector or operation.
- Impact: A practical failure is: review dispatch for `id:vps-a`, change the
  selector or operation to a destructive file/config/update job for another
  VPS while the resolve request is pending, and then have the old confirmation
  appear over the changed composer. Confirming submits the old frozen target
  IDs and operation while the surrounding form now shows a newer intent. This
  can execute a stale production job after the operator has already moved on to
  a different dispatch draft, and it undermines the rule that edits close the
  confirmation and require fresh review.
- Evidence: `frontend/src/panels/JobDispatchPanel.tsx:628-656` resolves
  targets asynchronously and then opens a confirmation. Form changes close
  existing prompts at
  `frontend/src/panels/JobDispatchPanel.tsx:391-449`, but they do not cancel
  or invalidate the in-flight submit request because no prompt exists yet.
  `buildDispatchConfirmationSnapshot()` at
  `frontend/src/panels/JobDispatchPanel.tsx:659-779` freezes selector,
  timeout, force flag, command mode, file-transfer fields, update fields,
  backup fields, operation payload, and privilege assertion from the review
  request's render snapshot. Those frozen values are correct for that old
  review, but stale if the operator has edited the composer while the request
  was pending.
  `dispatchJobNow()` submits the frozen snapshot's `target_client_ids`,
  selector, operation, timeout, and privilege assertion at
  `frontend/src/panels/JobDispatchPanel.tsx:959-1047`. The backend verifies
  fixed target IDs against the submitted selector and then uses those IDs at
  `crates/api/src/routes_jobs.rs:193-217`.
- Notes: This is distinct from AUD-028. AUD-028 covered editing after a
  confirmation prompt was already open. This issue occurs before the prompt
  opens, while the confirmation snapshot is being assembled asynchronously. A
  clean fix should freeze all dispatch review inputs before the first await and
  pair them with a form generation. If the generation changes before the async
  review completes, the response should be ignored and no prompt should open.
  Valid prompts should remain bound only to their frozen snapshot.
- Fix: Job dispatch target preview and dispatch review now use the shared
  review-generation guard with a visible preparing state. Operation-affecting
  edits invalidate pending review work, and stale completions cannot open a
  dispatch confirmation. Covered by `dispatch-target-consistency.spec.ts`.

### AUD-176: Config And Data-Source Review Requests Can Open Stale Confirmations After Edits

- Severity: High
- Status: Fixed
- Area: Frontend/Config/Data Sources
- Context: Operators use Config and Data-source panels to apply incremental
  runtime config patches and assign data-source presets across selector
  resolved VPS sets. These workflows can affect monitoring, telemetry, hot
  config rendering, and agent runtime behavior across many production VPSs.
- Root Cause: Several review handlers build a frozen confirmation after one
  or more async calls but do not bind the response to a form generation. Input
  edits close already-open confirmations, but they do not cancel or invalidate
  an in-flight review request that started before the edit. When the old
  request completes, it can reopen a valid confirmation for the earlier
  selector, preset, rendered TOML, timeout, or target set while the visible
  editor now shows a newer draft.
- Impact: A practical operator sequence is: click `Review apply` or submit a
  data-source assignment, immediately adjust the selector, preset, rule values,
  target VPS, or timeout while backend rendering/target resolution/privilege
  assertion work is pending, then have the old confirmation open over the
  changed form. Confirming executes the older frozen config patch or assignment
  after the operator has moved on to a different draft. In 20+ VPS operation,
  this can apply stale runtime config or data-source assignments to the wrong
  target set and weaken the guarantee that edits require a fresh review.
- Evidence: Bulk config apply resolves targets, renders a hot-config template,
  builds a privilege assertion, and then opens confirmation at
  `frontend/src/panels/ConfigPanel.tsx:517-563`. The rule-values textarea,
  selector input, and timeout input clear current confirmation state at
  `frontend/src/panels/ConfigPanel.tsx:627-681`, but no generation guard
  prevents the pending review from later setting `applySnapshot` and
  `confirmOpen`. Data-source assignment has the same shape at
  `frontend/src/panels/DataSourcePresetPanel.tsx:252-288`: it resolves
  targets, previews the assignment, and opens `pendingConfirmation` after the
  awaits, while preset and selector edits only clear current state at
  `frontend/src/panels/DataSourcePresetPanel.tsx:661-725`. Single-VPS
  data-source config apply renders/builds a privileged patch and opens
  confirmation at `frontend/src/panels/DataSourcePresetPanel.tsx:340-380`;
  target, timeout, and privilege edits clear current confirmation state around
  `frontend/src/panels/DataSourcePresetPanel.tsx:785-810` but do not
  invalidate the in-flight request.
- Notes: This is distinct from AUD-030 and AUD-035, which covered reading
  mutable state after an already-open confirmation. The current issue occurs
  before the prompt opens. A clean fix should snapshot review inputs before the
  first await, assign a review generation to every async review request, ignore
  responses from stale generations, and submit only the frozen snapshot shown
  in the prompt.
- Fix: Bulk config apply, single-VPS config apply, data-source assignment, and
  data-source rendered apply now invalidate pending async review work on
  selector, preset, target, TOML, timeout, values, or privilege edits. Stale
  render/resolve/assertion completions are ignored. Covered by
  `dispatch-target-consistency.spec.ts`.

### AUD-177: Network Mutation Review Requests Can Open Stale Confirmations After Topology Edits

- Severity: Critical
- Status: Fixed
- Area: Frontend/Topology/Network
- Context: The topology panels can apply, rollback, inspect, probe, speed-test,
  and update OSPF cost for tunnel plans. Apply, rollback, and OSPF cost update
  are privileged network mutations that can change routing and interface state
  on production VPSs.
- Root Cause: Network review handlers now build frozen snapshots, but they do
  not bind those async snapshot-building requests to a form generation. Editing
  plan, endpoint side, backend, timeout, probe/speed options, or privilege mode
  clears any currently open snapshot, but it does not invalidate a review
  request already waiting on operation construction or privilege assertion
  building. When the old request completes, it can reopen a confirmation for
  the previous topology draft over a changed visible form.
- Impact: An operator can click review for one tunnel plan/side/backend or
  OSPF update, then adjust the selected plan, endpoint side, backend, timeout,
  or privilege mode while async review work is pending. The old confirmation
  can appear afterward and still submit a root network mutation for the stale
  endpoint. In a real 20+ VPS fleet this can apply or roll back tunnel config,
  run network lifecycle commands, or change OSPF cost on an endpoint the
  operator no longer intends to mutate.
- Evidence: Network apply review starts async work in
  `frontend/src/panels/topology/TopologyApplyControls.tsx:122-204`,
  including `buildNetworkApplyOperation(...)` and
  `buildPrivilegeForJobOperation(...)`, then sets `networkSnapshot`. Plan,
  side, backend, timeout, probe/speed, and privilege edits clear only the
  current snapshot at
  `frontend/src/panels/topology/TopologyApplyControls.tsx:271-468`; no
  generation guard prevents the pending request from later setting a stale
  snapshot. OSPF cost update has the same pattern at
  `frontend/src/panels/topology/TopologyOspfUpdateControls.tsx:100-152` and
  clears only current snapshots on edits at
  `frontend/src/panels/topology/TopologyOspfUpdateControls.tsx:223-299`.
  Confirm submission uses the frozen snapshots at
  `frontend/src/panels/topology/TopologyApplyControls.tsx:213-235` and
  `frontend/src/panels/topology/TopologyOspfUpdateControls.tsx:161-180`.
- Notes: This is distinct from AUD-031 and AUD-032, which covered reading
  mutable topology state after an already-open confirmation. The residual
  problem is stale async confirmation creation before the prompt opens. A clean
  fix should snapshot the review inputs and a form generation before the first
  await, ignore review completions from stale generations, and keep the
  confirmation bound to that exact frozen network mutation.
- Fix: Added a shared frontend review-generation guard and wired topology
  apply/rollback/probe/speed and OSPF cost review preparation through it.
  Operation-affecting edits now invalidate in-flight review work before a stale
  privilege assertion can reopen a prompt. Covered by desktop Playwright
  stale-edit tests in `frontend/tests/dispatch-target-consistency.spec.ts`.

### AUD-178: Backup And Restore Review Requests Can Open Stale Confirmations After Edits

- Severity: Critical
- Status: Fixed
- Area: Frontend/Backups/Restore
- Context: Operators use the backup workspace to create backup policies,
  request backups, upload/promote artifacts, create restore plans, run live or
  dry-run restores, roll back restores, and run migration restores. These
  workflows affect backup evidence, recurring schedules, restore target paths,
  decrypted restore payloads, and production filesystem state.
- Root Cause: Backup and restore review handlers build frozen confirmation
  snapshots after asynchronous work such as selector resolution, privilege
  assertion generation, retained-output loading, restore artifact preparation,
  and restore rollback operation construction. Form edits close any currently
  open confirmation, but there is no request generation or form-change guard
  for an in-flight review request. A pending old review can therefore complete
  after the operator has edited the form and reopen a confirmation for the
  older backup/restore draft.
- Impact: An operator can click review for one backup policy, one-time backup,
  restore plan, live restore, migration restore, or restore rollback, then edit
  the target VPS, paths, include-config flag, destination root, archive source,
  private key, dry-run mode, timeout, or privilege mode while async review work
  is pending. The old confirmation can appear over the changed form and submit
  a stale snapshot if confirmed. For restore and rollback this can mutate the
  wrong target or path set; for backup policy creation it can save a recurring
  policy for an older fixed target set and path scope.
- Fix: Backup/restore review preparation now uses the shared review-generation
  guard so stale async completions are ignored after any relevant edit. Restore
  execution no longer accepts manually entered archive path, size, or SHA-256
  in frontend/CLI/VTY; it selects a completed upload transfer record whose size
  and SHA-256 match the selected backup artifact. Restore scope and destination
  root are derived from backup/target records in frontend and CLI/VTY. Covered
  by desktop Playwright restore stale-edit tests, restore executable dispatch
  tests, Rust VTY tests, CLI help smoke, and restore visual audit screenshots.
- Evidence: Backup policy review resolves targets and builds a schedule
  privilege assertion before setting `pendingPolicySnapshot` and
  `pendingConfirmation` at `frontend/src/panels/BackupsPanel.tsx:428-511`.
  One-time backup and restore-plan review build privileged snapshots at
  `frontend/src/panels/BackupsPanel.tsx:583-627` and
  `frontend/src/panels/BackupsPanel.tsx:738-795`. Live restore and migration
  restore prepare/decrypt artifacts and build privileged job snapshots at
  `frontend/src/panels/BackupsPanel.tsx:807-997` and
  `frontend/src/panels/BackupsPanel.tsx:1084-1123`. Restore rollback loads
  prior job outputs and builds a rollback operation at
  `frontend/src/panels/BackupsPanel.tsx:1000-1055`. The relevant edit
  callbacks clear only existing confirmation state, for example backup policy
  edits at `frontend/src/panels/BackupsPanel.tsx:1671-1733`, backup request
  edits at `frontend/src/panels/BackupsPanel.tsx:1758-1791`, restore-plan
  edits at `frontend/src/panels/BackupsPanel.tsx:1827-1893`, restore-run
  edits at `frontend/src/panels/BackupsPanel.tsx:1894-1942`, and rollback
  edits at `frontend/src/panels/BackupsPanel.tsx:1943-1970`. None of these
  edit paths invalidate a review request that is already awaiting async work.
- Notes: This is distinct from AUD-029, which covered mutable form reads after
  an already-open backup/restore confirmation. The current issue occurs before
  the prompt opens. A clean fix should snapshot all reviewed inputs and a form
  generation before async review work starts, ignore stale completions, and
  keep confirmation submission bound only to the exact frozen snapshot shown.

### AUD-179: Multiple Backup Artifacts Can Reference The Same Object Key

- Severity: Medium/High
- Status: Confirmed
- Area: API/Backups/Object Storage
- Context: Operators can record backup artifact metadata for objects that were
  uploaded outside the inline/chunked API path. Backup retention, history
  retention, and server artifact cleanup later delete metadata and object-store
  payloads by object key.
- Root Cause: `backup_artifacts.object_key` is not unique in the canonical
  schema, and the direct metadata-record route validates only object-key
  syntax, hash format, encryption flag, size, and confirmation. It does not
  reject an object key already referenced by another backup artifact row. Some
  upload and handoff paths check `backup_artifact_object_key_exists`, but that
  protection is not enforced by the table or by
  `record_backup_artifact_metadata`.
- Impact: A confirmed metadata import can accidentally link two backup requests
  to the same object-store key. Later retention or artifact cleanup can delete
  that object while another visible backup artifact still points at it, making
  restore/download fail and corrupting operator trust in backup history. This
  is practical because `backup-artifact-record` is a shipped CLI/VTY workflow
  for externally staged artifacts, and long-lived backup policies routinely
  prune old artifacts.
- Evidence: `migrations/0004_backups_restores.sql` defines
  `backup_artifacts.object_key TEXT NOT NULL` without a unique constraint.
  `crates/api/src/routes_backups.rs::record_backup_artifact_metadata` calls
  `validate_backup_artifact_metadata_request` and then
  `record_backup_artifact_metadata` without checking existing object-key use.
  `crates/api/src/repository_backup_artifacts.rs::record_backup_artifact_metadata`
  inserts a new `backup_artifacts` row and links the backup request without a
  uniqueness predicate. Cleanup and retention paths delete by object key, for
  example `crates/worker/src/backup_policy_retention.rs` calls
  `delete_confirmed(&candidate.object_key)` after pruning rows, and
  `crates/worker/src/main.rs::delete_backup_artifact` deletes the object key
  after removing backup artifact metadata.
- Notes: This is distinct from AUD-106. AUD-106 is about recording metadata
  without verifying that object-store bytes match the submitted hash and size.
  This issue remains even when the object exists and the submitted hash is
  correct: the missing invariant is one durable backup artifact ownership row
  per object key, or explicit reference counting before deletion.

### AUD-180: Reuploaded File-Transfer Source Artifacts Can Inherit Stale Cleanup Age

- Severity: Medium/High
- Status: Confirmed
- Area: API/File Transfers/Artifact Cleanup
- Context: Operators can upload reusable source artifacts for resumable
  file-transfer jobs. The source object key is content-addressed by SHA-256, so
  uploading the same script, package, archive, or config payload again is a
  normal production workflow. Server artifact cleanup then lets operators prune
  retained source artifacts by expressions such as domain and age.
- Root Cause: `file_transfer_source_artifacts.object_key` is not unique, but
  the shared `server_artifacts` cleanup registry is unique by object key.
  Reuploading the same source bytes inserts a new source-artifact row, then
  calls `register_server_artifact`, whose `ON CONFLICT (object_key)` branch
  updates metadata and status but does not refresh `created_at` or model
  multiple source-artifact owners for the same object key. Cleanup preview
  filters by `server_artifacts.created_at`, not by the newest
  `file_transfer_source_artifacts.created_at`.
- Impact: A recently reuploaded source artifact can still match an old cleanup
  expression because the registry row keeps the first upload's timestamp.
  Confirmed cleanup for `artifact.domain == "file_transfer_source"` and an age
  window can delete the object and remove all source-artifact rows for that
  object key, including the newer visible source artifact the operator just
  uploaded. Subsequent file-transfer jobs that intended to reuse that new
  artifact fail or lose their reviewed source payload.
- Evidence: `crates/api/src/routes_file_transfers.rs` derives source artifact
  object keys from the SHA-256 hash and allows identical-object reuploads after
  verifying the existing object bytes. `migrations/0007_data_sources_file_transfer.sql`
  defines `file_transfer_source_artifacts.object_key TEXT NOT NULL` without a
  unique constraint. `crates/api/src/repository_file_transfer_sources.rs`
  inserts a fresh source-artifact row and then registers a shared
  `server_artifacts` entry. `crates/api/src/repository_server_jobs.rs`
  upserts that registry row on object-key conflict without updating
  `created_at`, and cleanup expressions evaluate `artifact.created_at` from
  `server_artifacts`. `crates/worker/src/main.rs::delete_file_transfer_source_artifact`
  deletes all source-artifact rows with the object key before deleting the
  object bytes.
- Notes: This is distinct from AUD-062. AUD-062 is about non-atomic
  domain-metadata and cleanup-registry commits. This issue occurs even when
  every insert succeeds: the retained registry timestamp no longer represents
  the currently visible source artifact's lifecycle.

### AUD-181: Key Lifecycle Review Can Open Stale Confirmations After Key-Field Edits

- Severity: High
- Status: Fixed
- Area: Frontend/Access/Keys
- Context: The Access panel imports gateway-issued agent identities, rotates
  client public keys, and revokes current VPS keys. These actions can register
  a new visible client, replace the trust root for an existing VPS, disconnect
  live sessions, and mark active work lost.
- Root Cause: The fixed Access panel now submits frozen snapshots after a
  confirmation opens, but the review request itself still awaits local
  privilege assertion construction before setting the snapshot. Editing the
  client ID, public key, display name, tags, mode, revoke target, or revoke
  reason clears only an already-open confirmation. It does not invalidate a
  review request that is still awaiting `buildPrivilegeAssertion`.
- Impact: An operator can start review for one key import/rotation/revoke,
  edit the visible key lifecycle form while browser crypto or local privilege
  assertion work is still pending, and then see a confirmation open for the
  older client/key/reason after the visible form has changed. Confirming that
  prompt can rotate or revoke the old VPS key after the operator has moved on
  to a newer draft. This is practical on slower operator devices or under
  normal UI multitasking because privilege assertions are asynchronous and key
  lifecycle mutations are high-impact.
- Evidence: `frontend/src/panels/AccessPanel.tsx::requestIdentityImport`
  awaits `buildPrivilegeAssertion` before calling `setIdentitySnapshot` and
  `setPendingConfirmation("agent-identity")`. `requestClientKeyRevoke` has the
  same shape before `setRevokeSnapshot` and `setPendingConfirmation("key-revoke")`.
  The edit handlers call `clearIdentityReview` or `clearRevokeReview`, which
  clear current snapshots and open prompts only; no generation or review-token
  guard prevents an older pending review from opening a stale confirmation.
- Notes: This is distinct from AUD-034. AUD-034 covered mutable form reads
  after an already-open confirmation. The current issue occurs before the
  confirmation opens and should be fixed by snapshotting reviewed inputs plus a
  form generation before the first await, then ignoring stale async completions.
- Fix: Access identity import/rotation and key revoke reviews now snapshot the
  reviewed fields before building local privilege assertions, invalidate on
  key-field edits, and ignore stale assertion completions. Covered by
  `dispatch-target-consistency.spec.ts`.

### AUD-182: Terminal Stream Output Can Append After The Terminal-Open Target Is Terminal

- Severity: Medium/High
- Status: Confirmed
- Area: API/Gateway/Terminal/Lifecycle
- Context: Terminal-open jobs return an immediate command result, but the agent
  keeps a background PTY stream for the same session and forwards PTY chunks
  and stream-status events through the gateway. Operators then use terminal
  replay/session views and normal job-output history to inspect that stream.
- Root Cause: Command-output ingest now checks that the target is still active
  before accepting new output, but terminal-output ingest does not. It only
  verifies that a job target exists, then appends a new `job_outputs` row at
  the next sequence. The terminal-open command itself returns a `done: true`
  status output, so the associated target can be completed before later
  background terminal stream chunks arrive.
- Impact: A completed, canceled, timed-out, or otherwise terminal terminal job
  can keep accumulating new output rows after the terminal state was supposed
  to be immutable. This makes terminal replay and job-output downloads change
  after completion, creates confusing audit and support evidence, and bypasses
  the late-output protection added for normal command output. In production,
  this is practical because long-running terminal sessions intentionally keep
  streaming after the open command has returned and because gateway/API retries
  can deliver buffered terminal events late.
- Evidence: `crates/agent/src/terminal.rs::status_output` marks terminal-open
  status outputs as `done: true`, while background tasks
  `read_terminal_output`, the child wait handler, and the idle reaper keep
  emitting `TerminalStreamOutput` events for the same `open_job_id`.
  `crates/gateway/src/main.rs` forwards `TerminalStreamOutput` frames directly
  to `/internal/v1/gateway/terminal-output`. `crates/api/src/routes_ingest.rs::ingest_terminal_output`
  lists the target and checks only that the target client exists, then calls
  `append_job_output_chunk_with_config`. That append helper allocates
  `MAX(seq) + 1` under an advisory lock and inserts a new output row without
  checking `job_targets.status` or `completed_at`. In contrast,
  `ingest_command_output` rejects late non-identical output for inactive
  targets before storage.
- Notes: This is distinct from AUD-073, AUD-076, and AUD-077. Those cover
  terminal storage ceilings, idempotency, and final-event retention. This issue
  is about accepting new terminal evidence after the parent target is already
  terminal. A clean fix should either keep terminal stream output under a
  separate session stream model or enforce an active-target/terminal-session
  lease predicate before appending to normal job-output history.

### AUD-183: VPS Deletion Confirmation Can Remain Armed After Fleet Selection Changes

- Severity: High
- Status: Fixed
- Area: Frontend/Fleet/Delete
- Context: Fleet > Instances lets operators delete a VPS from inventory. This
  deactivates access, removes the VPS from future targeting, disconnects live
  gateway sessions, soft-deletes related topology plans, and marks active work
  lost. The dashboard presents this as a destructive reviewed action.
- Root Cause: The Fleet delete workflow stores a frozen `deleteSnapshot`, but
  ordinary fleet row selection/opening and selection-panel navigation do not
  clear it. The review request also awaits local privilege assertion
  construction before setting the snapshot, without a generation guard. That
  means a confirmation can remain open after the operator changes the visible
  VPS context, or can open for an older selected VPS after the operator has
  already selected or opened another row.
- Impact: An operator can review deletion for one VPS, change the selected or
  opened VPS while the prompt is still armed, and then confirm deletion from a
  screen that now appears focused on another machine. On slower operator
  devices, the same can happen before the prompt opens because
  `buildPrivilegeAssertion` is asynchronous. This can delete or deactivate a
  production VPS that is no longer the operator's visible focus, which is
  severe for 20+ VPS fleets because deletion also affects selectors, schedules,
  topology visibility, live sessions, and active job targets.
- Evidence: `frontend/src/panels/FleetWorkspace.tsx::requestDeleteAgent`
  captures `rows[0]`, awaits `buildPrivilegeAssertion`, and then sets
  `deleteSnapshot`. `confirmDeleteAgent` submits only that snapshot to
  `onDeleteAgent`. The `ConsoleDataGrid` row open handler calls
  `onSelectAgent(agent.id)`, workflow shortcuts call `onSelectAgent` or
  navigate to other panels, and the selection panel can change visible context,
  but none of those paths clear `deleteSnapshot` or invalidate an in-flight
  delete review. The `ConfirmationPrompt` remains open whenever
  `Boolean(deleteSnapshot)` is true.
- Notes: This is distinct from backend client-deletion lifecycle fixes
  AUD-112, AUD-113, AUD-114, AUD-121, and AUD-145. The backend may execute the
  exact frozen deletion request correctly; the frontend issue is that the
  frozen destructive prompt can survive or appear after the operator's visible
  Fleet context has changed. A clean fix should clear `deleteSnapshot` on row
  open, selection/action context changes, and navigation, and should ignore
  stale async privilege-assertion completions using a review generation.
- Fix: Fleet deletion review now clears on row open, grid selection changes,
  navigation workflow shortcuts, and route/context changes. The delete review
  privilege assertion is generation-guarded so stale completions cannot open a
  prompt. Covered by `dispatch-target-consistency.spec.ts`.

### AUD-184: Bulk File Review Can Open Stale Confirmations After Selector Or Operation Edits

- Severity: Critical
- Status: Fixed
- Area: Frontend/Jobs/Multi-File
- Context: Jobs > Multi files dispatches file downloads, uploads, copies,
  renames, deletes, chmod/chown, mkdir, and text writes across selector-resolved
  VPS sets. The panel intentionally resolves targets and opens a confirmation
  before submitting fixed target IDs and a privileged file-operation payload.
- Root Cause: `prepareBulkOperation` starts asynchronous backend target
  resolution, then builds the operation and opens `pendingConfirmation` after
  the await. Input edits clear an already-open confirmation, and the panel no
  longer reuses a cached preview for execution, but there is no request
  generation or form-snapshot guard for an in-flight review request. An older
  review can therefore complete after the operator edits the selector, action,
  path, destination, mode, owner/group, recursive flag, overwrite flag, upload
  file/options, or write-text content.
- Impact: A practical failure is: click review for a destructive multi-file
  operation on selector A, immediately change the selector or operation to
  selector B or a different path/action while resolution is pending, and then
  have the old confirmation open over the changed composer. Confirming submits
  the old fixed targets and old file payload even though the visible panel now
  shows a newer intent. This can delete, overwrite, chmod, chown, copy, move,
  or upload files on the wrong VPS set or path, which is release-blocking for
  operator workflows across 20+ VPSs.
- Evidence: `frontend/src/panels/jobs/MultiFileActionsPanel.tsx::prepareBulkOperation`
  awaits `onResolveTargets`, stores the returned preview, calls
  `buildOperation`, and then sets `pendingConfirmation`. Selector and
  operation-affecting controls call `clearPendingConfirmation` only for the
  current open prompt; they do not invalidate the async review that is already
  awaiting target resolution. The confirm handler submits
  `confirmation.selectorExpression`, `confirmation.targets`, and
  `confirmation.operation` to `onCreateJob` as fixed target IDs. The existing
  dispatch target consistency test covers resolving targets again instead of
  executing a cached preview, and it covers closing an already-open prompt on
  edits, but it does not cover stale in-flight review completion.
- Fix: Multi-file review preparation now captures a review generation before
  async target resolution and ignores completions after selector or operation
  edits. Added a desktop Playwright regression that edits the path while review
  preparation is pending and verifies only the fresh operation can open a
  confirmation.
- Notes: This is distinct from the earlier cached-preview bug and from
  AUD-175. AUD-175 covers the main job dispatch composer. This issue is in the
  dedicated multi-file workflow with its own target resolution and destructive
  file-operation controls. A clean fix should snapshot all reviewed inputs and
  a form generation before the first await, ignore stale review completions,
  and keep confirmation submission bound to the exact frozen snapshot shown.

### AUD-185: Terminal Input Sequencing Can Drop Out-Of-Order Or Conflicting Input

- Severity: High
- Status: Confirmed
- Area: Agent/API/Terminal
- Context: Operators can open interactive terminal sessions and then send
  terminal-input jobs from the dashboard, CLI, or VTY. The UI derives the next
  input sequence from the session view, and CLI/VTY users provide
  `--input-seq` directly.
- Root Cause: The agent accepts any `input_seq` greater than the last seen
  sequence and treats any later `input_seq <= last_input_seq` as an idempotent
  duplicate. It does not require `input_seq == last_input_seq + 1`, and it does
  not store or compare the input payload for a duplicate sequence. The API
  validates only that `input_seq >= 1`, and terminal session summaries persist
  only `last_input_seq`, not per-sequence input hashes.
- Impact: A stale browser tab, two operators on the same terminal, a CLI/VTY
  mistake, or retry/reorder around the terminal input workflow can permanently
  drop a real input. Example: two clients both see `last_input_seq = 0`; one
  sends `input_seq = 1` with `systemctl restart app`, another sends
  `input_seq = 1` with `systemctl status app`. Whichever reaches the agent
  second is reported as `duplicate_ignored` and is never written to the PTY,
  even though it is not the same input. Similarly, if sequence 2 is accepted
  before sequence 1, the later sequence-1 input is ignored. For production
  terminal workflows this can skip commands, reorder operator intent, or leave
  the shell in a different state than the operator expects.
- Evidence: `crates/agent/src/terminal.rs::TerminalRegistry::accept_input`
  sets `input_already_seen = input_seq <= entry.last_input_seq`, advances
  `last_input_seq` for any higher value, and never stores input bytes or an
  input hash. `crates/agent/src/terminal.rs::input_terminal_session` writes the
  PTY data only when `input_already_seen` is false and returns
  `duplicate_ignored` otherwise. `crates/api/src/job_terminal.rs::validate_terminal_input`
  rejects only zero sequence numbers. `frontend/src/panels/JobDispatchPanel.tsx`
  and `frontend/src/panels/jobDispatchModel.ts` compute or submit the sequence
  from session state, while `crates/vpsctl/src/commands_terminal.rs` and
  `crates/vpsctl/src/vty_terminal.rs` expose user-provided input sequences.
  `crates/api/src/repository_terminal_sessions.rs` stores only
  `last_input_seq`.
- Notes: This is distinct from terminal output storage, replay, and
  late-output issues already tracked by AUD-073, AUD-076, AUD-077, AUD-129, and
  AUD-182. A clean fix should make terminal input idempotency
  sequence-and-payload aware: accept exactly the next sequence, accept
  byte-identical duplicate replays as no-ops, reject conflicting duplicates,
  and reject future gaps as out-of-order/retryable without writing to the PTY.

### AUD-186: Terminal PTYs Can Survive Disconnect Or Access Revocation Without Reconciliation

- Severity: Medium/High
- Status: Confirmed
- Area: Agent/Gateway/Terminal/Lifecycle
- Context: Operators can open interactive terminal sessions that intentionally
  keep a shell or command running after the `terminal_open` job target has
  completed. The API and gateway can later disconnect the agent because of
  network failure, controlled gateway restart, key revocation, key rotation, or
  client deletion.
- Root Cause: Agent terminal sessions live in a process-global
  `TERMINAL_REGISTRY`, independent of the gateway connection loop. When
  `connect_and_stream` returns after a gateway disconnect, `run_agent` retries
  the connection with the same process and command runtime, but it does not
  reconcile, close, or report existing terminal sessions. Terminal lifecycle
  output is emitted through a bounded `terminal_stream_tx` with `try_send`; if
  the agent is disconnected or the channel is full, final `exited`,
  `idle_timeout`, or `closed` evidence can be dropped before it ever reaches
  the gateway/API.
- Impact: A terminal shell or long-running foreground command can continue on
  the VPS after the control plane intentionally disconnects the client during
  delete/revoke/rotation, or after a network/API outage. The only automatic
  local cleanup is the requested idle timeout, which can be as high as 24
  hours. Operators may believe access has been deactivated or a terminal
  session has ended while the agent process still owns a live PTY process group.
  If final lifecycle events were dropped while disconnected, the API terminal
  session view can also remain stale until an operator manually probes the
  session, which is unsafe during incident response across a 20+ VPS fleet.
- Evidence: `run_agent` creates one `AgentCommandRuntime` and one
  `process_incarnation_id` for the process, then repeatedly calls
  `connect_and_stream` after failures without clearing terminal state at
  `crates/agent/src/runtime.rs:52-96`. Terminal sessions are held in the static
  `TERMINAL_REGISTRY` at `crates/agent/src/terminal.rs:37` and inserted during
  `open_terminal_session`; cleanup happens only through explicit
  `close_terminal_session`, child exit, or the idle reaper at
  `crates/agent/src/terminal.rs:485-526` and
  `crates/agent/src/terminal.rs:930-960`. Stream lifecycle events are sent via
  `TerminalSessionHandle::try_emit_stream_output`, which clones the optional
  sender and ignores `stream_tx.try_send(event)` failure at
  `crates/agent/src/terminal.rs:594-617`. The agent connection loop forwards
  `terminal_stream_rx` only while connected at `crates/agent/src/runtime.rs:236-247`.
  Terminal idle timeout is operator-provided with a protocol default and max at
  `crates/common/src/protocol.rs:2321-2326`; frontend construction clamps it up
  to 86,400 seconds in `frontend/src/panels/jobDispatchModel.ts`.
- Notes: This is distinct from AUD-077 and AUD-182. Those cover API/gateway
  handling of terminal stream output after it is emitted. This issue is
  agent-local lifecycle authority: terminal process groups and final session
  evidence are not reconciled when the control-plane connection is lost or
  access is revoked. A clean fix should define the intended policy explicitly,
  such as closing or quarantining terminal sessions on authoritative
  disconnect/revoke, replaying terminal lifecycle summaries after reconnect,
  and making final terminal lifecycle events reliable rather than best-effort.

### AUD-187: History Retention Policy Saves Ignore The Confirmation Contract

- Severity: Medium/High
- Status: Confirmed
- Area: API/Frontend/History Retention
- Context: History retention policies control how long audit logs, telemetry,
  network history, job-output history, and backup-artifact history are
  retained/exported. A policy change can later enable destructive prune
  behavior for forensic job output or backup metadata/object payloads.
- Root Cause: `UpsertHistoryRetentionPolicyRequest` includes `confirmed`, but
  `upsert_history_retention_policy` never checks it. The dashboard saves policy
  changes by sending `confirmed: true` directly with no reviewed confirmation
  prompt.
- Impact: Direct API callers can change retention policy without the
  backend-enforced confirmation contract used for other mutating admin
  workflows. The dashboard also turns the save button into an auto-confirmed
  mutation, so an accidental edit to retention days, prune limit, metadata-only
  mode, or export-enabled state can immediately alter future cleanup/export
  behavior. In production this can shorten evidence retention or flip object
  deletion mode before a later worker/API prune removes rows or object
  payloads.
- Evidence: The request model exposes `confirmed` at
  `crates/api/src/model_history.rs:90-103`, but the route only authorizes
  `inventory:write` and calls `repo.upsert_history_retention_policy` at
  `crates/api/src/routes_history.rs:32-49`. The dashboard `submitPolicy` sends
  `confirmed: true` immediately from live controls at
  `frontend/src/panels/AuditLogPanel.tsx:129-137`.
- Notes: This is distinct from AUD-048, which covers domain-specific
  authorization; AUD-037, which covers mutable prune confirmation; and AUD-154,
  which covers confirmed prune reselecting live rows. A clean fix should
  require backend confirmation for retention policy upserts and make the
  frontend present a reviewed snapshot of domain, retention days, prune limit,
  metadata mode, export mode, and notes before saving.

### AUD-188: File Rename And Move Can Follow Path Races Outside The Reviewed Source Or Destination

- Severity: High
- Status: Fixed
- Area: Agent/File Browser/Safety
- Context: Operators can move or rename files and directories from the
  single-VPS file browser and the multi-file bulk panel. These are privileged
  file operations and are often used under application directories, upload
  trees, cache directories, and other paths where local service users may have
  write access.
- Root Cause: The agent validates the source and destination path strings with
  `validate_mutable_path`, then checks source and destination existence with
  `symlink_metadata`, but the actual operation is a later
  `tokio::fs::rename(source, destination)` by pathname. The source identity,
  destination parent identity, and destination path are not anchored to open
  directory handles or rechecked immediately before the rename with
  no-follow/openat-style semantics.
- Impact: A local user or compromised workload with write access to a reviewed
  parent directory can race a privileged rename/move job by replacing a parent
  component or destination with a symlink or swapping the source path after the
  precheck. The agent can then move the wrong path, move a reviewed source into
  a different directory than the operator reviewed, or replace a path outside
  the intended destination tree. In production this can corrupt service files,
  move data into privileged locations, or make audit output claim that the
  reviewed source/destination was renamed when the filesystem mutation followed
  a different path.
- Evidence: `execute_file_rename` validates path strings at
  `crates/agent/src/file_browser.rs:555-558`, uses
  `tokio::fs::symlink_metadata` for source and destination prechecks at
  `crates/agent/src/file_browser.rs:559-583`, then performs the mutation with
  `tokio::fs::rename(source, destination)` at
  `crates/agent/src/file_browser.rs:585-587`. The workflow is exposed by the
  single-VPS browser's rename and paste-move actions at
  `frontend/src/panels/jobs/FileBrowserPanel.tsx:393-405` and
  `frontend/src/panels/jobs/FileBrowserPanel.tsx:469-498`, and by the
  multi-file bulk rename operation at
  `frontend/src/panels/jobs/MultiFileActionsPanel.tsx:226-236`.
- Notes: This is distinct from AUD-128, which covers recursive delete;
  AUD-130, which covers copy/chmod/chown validation races; AUD-135, which
  covers temporary staging path substitution for text-write/copy; and frontend
  stale-confirmation issues AUD-140 and AUD-184. A clean fix should make
  rename/move descriptor-anchored, verify source and destination parent
  identities at commit time, use no-follow semantics unless an explicit
  operator policy allows symlink following, and report skipped/conflict status
  if the reviewed identities changed.
- Resolution: Fixed by resolving source and destination parents with no-follow
  directory descriptors, rechecking source identity immediately before commit,
  and performing the move with fd-relative rename/no-replace semantics.

### AUD-189: Official Agent Install Examples Do Not Start The Service They Claim To Start

- Severity: Medium/High
- Status: Confirmed
- Area: Deploy/Agent Install/Docs
- Context: Operators onboard VPSs through the documented direct-gateway
  one-line installer. The README and `deploy/AGENT_GATEWAY_INSTALL.md` examples
  are the production copy-paste path for registering a VPS identity and
  installing the agent on the remote host.
- Root Cause: The documented installer defaults
  `VPSMAN_AGENT_ENABLE_SERVICE`/`VPSMAN_ENABLE_SERVICE` to false, so it writes
  `agent.toml` and a systemd unit but does not link, enable, or start the
  service unless the install command explicitly sets the flag. The official
  root and user install examples omit that flag, while the installation guide
  says the installer starts the agent.
- Impact: Following the official install flow can leave a newly provisioned VPS
  with a valid identity, installed binary, config, and unit file, but no
  running agent. At 20+ VPS scale this turns normal onboarding automation into
  visible never-connected clients, failed health expectations, and jobs or
  schedules that immediately skip the target as unavailable. Operators may
  debug gateway/API state even though the documented installer simply never
  started the service.
- Evidence: `deploy/install-agent.sh:36-38` treats the service-enable flag as
  false by default. The script only runs `systemctl ... enable --now` inside
  the `service_enable_requested` branch at
  `deploy/install-agent.sh:273-278`; otherwise it logs that the unit was
  written and tells the operator to set `VPSMAN_AGENT_ENABLE_SERVICE=1` at
  `deploy/install-agent.sh:280-281`. The official root and user examples omit
  the flag at `deploy/AGENT_GATEWAY_INSTALL.md:50-70`, and the same guide says
  "installs a systemd unit, and starts the agent" at
  `deploy/AGENT_GATEWAY_INSTALL.md:73`. The README's typical direct-gateway
  install flow also omits the flag at `README.md:174-180`.
- Notes: This is distinct from AUD-147, which covers custom binary URL
  checksum pinning in the same installer. A clean fix should make the
  documented examples and prose match the intended service policy: either
  include the explicit service-enable flag in the official copy-paste examples,
  or clearly document that the default install only stages the unit and
  requires a separate start command.

### AUD-190: Secure Compose Password Edits Leave API And Worker Using The Wrong Postgres Credentials

- Severity: Medium/High
- Status: Confirmed
- Area: Deploy/Compose/Database
- Context: Operators bootstrap the released Docker Compose deployment by
  copying `deploy/.env.example` to `deploy/.env`, editing secrets, and running
  `docker compose up -d`. This is the documented path for a production
  control-plane deployment.
- Root Cause: Compose passes `POSTGRES_PASSWORD` from `.env` only to the
  Postgres container, while the API and worker read their database connection
  string from the mounted suite config. The shipped suite config hardcodes
  `postgres://vpsman:vpsman@postgres:5432/vpsman`, and the compose service
  definitions do not set `VPSMAN_POSTGRES_URL` or otherwise derive the suite
  config database URL from `.env`.
- Impact: Following the secure documented flow can initialize Postgres with a
  non-default password while API and worker repeatedly try to connect with the
  stale `vpsman` password. A fresh deployment can therefore fail before the
  operator dashboard, scheduler, migrations, and job dispatcher become usable.
  At fleet scale this is a practical rollout blocker because the docs nudge
  operators to replace the placeholder password but do not mention the second
  credential location that must be kept in sync.
- Evidence: `deploy/.env.example:1-3` defines
  `POSTGRES_PASSWORD=replace-with-random-postgres-password`.
  `deploy/compose.yml:7-12` applies that value to the `postgres` service, but
  `deploy/compose.yml:19-27` and `deploy/compose.yml:53-59` configure API and
  worker only with `VPSMAN_SUITE_CONFIG`. The mounted suite config contains
  `postgres_url = "postgres://vpsman:vpsman@postgres:5432/vpsman"` at
  `deploy/config/vpsman.toml:54-56`. Rendering compose with
  `deploy/.env.example` confirms that API and worker receive no
  `VPSMAN_POSTGRES_URL` override while Postgres receives the replacement
  password. The README tells operators to copy and edit `.env` before
  deployment at `README.md:82-87`.
- Notes: This is distinct from suite-config permission/audit issues and from
  AUD-101's read-only config mount. A clean fix should have a single
  operator-facing source of truth for the deploy database password, such as
  passing `VPSMAN_POSTGRES_URL` to API/worker from `.env`, templating the suite
  config before start, or documenting and validating an explicit database URL
  setting so secure deploys fail fast with a clear message instead of a stale
  credential.

### AUD-191: Backup Gateway Endpoints Cannot Receive API Dispatch, Cancel, Or Lifecycle Disconnect Control

- Severity: High
- Status: Confirmed
- Area: API/Gateway/Dispatch
- Context: Agents support prioritized gateway endpoint lists and the official
  install/UI examples encourage operators to configure a primary and backup
  gateway endpoint. Gateway sessions also carry a `gateway_id`, so operators
  can observe which gateway accepted a client session.
- Root Cause: The API control-plane client has exactly one configured
  `gateway_control_url`. Job dispatch, job cancel, key-rotation disconnect,
  key-revoke disconnect, and VPS-delete disconnect all post to that single
  control URL. There is no registry that maps the active `gateway_sessions`
  row's `gateway_id` to a gateway control URL, and no fan-out to every
  configured gateway. If a client is connected to another gateway, the API can
  only ask the wrong gateway.
- Impact: A backup gateway can accept an agent connection and continue sending
  telemetry/events to the API, while operator jobs for that VPS cannot be
  delivered from the API because dispatch is sent to the configured gateway
  only. Cancellation and lifecycle invalidation have the same problem: the
  configured gateway returns an accepted "agent not online" result when the
  session is on a different gateway, so key rotation, revoke, or delete can
  proceed without disconnecting the actual live transport. At 20+ VPS scale
  this makes gateway failover misleading and can leave operators with online
  agents that cannot reliably receive work or be disconnected.
- Evidence: The official install flow shows multiple endpoint labels at
  `README.md:174-180` and `deploy/AGENT_GATEWAY_INSTALL.md:51-58`, and the
  Preferences UI prompts for newline-separated endpoint defaults at
  `frontend/src/panels/PreferencesPanel.tsx:442-475`. The API accepts only one
  `api.gateway_control_url` / `VPSMAN_GATEWAY_CONTROL_URL` at
  `crates/api/src/main.rs:324-330` and constructs one
  `GatewayDispatchClient` at `crates/api/src/main.rs:552-560`. Dispatch uses
  that single client at `crates/api/src/job_dispatcher.rs:239-248`; timeout
  cancellation uses the same single client at
  `crates/api/src/job_dispatcher.rs:325-335`; lifecycle disconnect uses the
  same single client at `crates/api/src/state.rs:158-180` from key lifecycle
  and delete routes. Gateway session records retain `gateway_id` at
  `crates/api/src/repository_gateway_sessions.rs:88-106` and list it back at
  `crates/api/src/repository_gateway_sessions.rs:274-305`, but dispatch and
  disconnect do not route by it. A gateway that does not host the client returns
  `accepted: true, disconnected: false, message: "agent_not_online"` at
  `crates/gateway/src/control.rs:360-376`, and the API currently treats any
  accepted disconnect as success.
- Notes: This is distinct from AUD-145's disconnect-before-DB-invalidation
  race and AUD-150's stale displaced session forwarding. A clean fix should
  either make the product explicitly single-gateway until routing exists, or
  add a durable gateway-control registry and route/fan-out dispatch,
  cancellation, and lifecycle disconnect to the gateway that owns the current
  active session.

### AUD-192: Gateway Agent TCP Listener Still Defaults To All-Interface Binding

- Severity: Medium/High
- Status: Confirmed
- Area: Gateway/Deploy/Security
- Context: Current deployment guidance says API and gateway host ports should
  stay localhost-bound by default, and operators should expose agent TCP only
  through a deliberate public proxy, firewall, or tunnel choice.
- Root Cause: The gateway binary still defaults `VPSMAN_GATEWAY_BIND` to
  `0.0.0.0:9443`, and the shipped suite config also sets
  `[gateway].bind = "0.0.0.0:9443"`. Compose masks this at the host boundary
  by mapping `127.0.0.1:9443:9443`, but direct binary runs, host-network
  deployments, or adapted container deployments inherit the all-interface
  process bind unless the operator notices and overrides it.
- Impact: A production operator can unintentionally expose the raw gateway
  listener to a reachable network while following shipped defaults outside the
  exact compose port mapping. The gateway has Noise authentication and client
  key validation, so this is not the same class as exposing the operator API,
  but it is still a privileged fleet ingress process that holds gateway secret
  material, forwards agent events, and owns command/control session state. A
  default public listener increases attack surface and contradicts the current
  private-by-default deployment model.
- Evidence: `crates/gateway/src/main.rs:53-54` declares the
  `VPSMAN_GATEWAY_BIND` default as `0.0.0.0:9443`;
  `crates/gateway/src/main.rs:211-217` applies `config.gateway.bind` when the
  environment variable is absent; `crates/gateway/src/main.rs:482-490` binds
  the listener to that value. The shipped suite config sets
  `bind = "0.0.0.0:9443"` at `deploy/config/vpsman.toml:20-22`. Compose maps
  the host port to loopback at `deploy/compose.yml:38-39`, while README says
  gateway TCP stays loopback-bound by default at `README.md:124-129` and the
  repository maintenance notes say API and gateway host ports should stay
  localhost-bound by default at `TO_AGENTS.md:129-135`.
- Notes: This is distinct from AUD-066 and AUD-067, which cover the operator
  API/dashboard exposure boundary. A clean fix should make the gateway process
  and shipped suite config default to loopback, then require explicit operator
  configuration for public agent TCP exposure.

### AUD-193: Gateway Lifecycle Events Can Expire Before API Accepts A New Process Incarnation

- Severity: High
- Status: Confirmed
- Area: Gateway/API/Lifecycle
- Context: Agent hello is the authoritative event that lets the API store the
  agent process incarnation and reconcile active targets from the previous
  incarnation. That path is required after an agent restart, update activation,
  or gateway reconnect with a newly started process.
- Root Cause: The gateway treats `/internal/v1/gateway/agent-hello`,
  `/internal/v1/gateway/session-started`, and
  `/internal/v1/gateway/session-ended` as lifecycle events with a fixed
  five-minute TTL. The gateway enqueues `agent-hello`, then immediately stores
  the live session in gateway memory and sends `ServerHello`; it does not wait
  for the API to durably accept the hello and finish incarnation reconciliation.
  If API forwarding is unavailable or delayed past the lifecycle TTL, the hello
  can expire and be dropped while the gateway still has a live agent session.
- Impact: After a practical API outage, restart, or rolling maintenance window
  longer than five minutes, the API can miss the only event that updates
  `clients.process_incarnation_id` and marks old-incarnation active targets
  `agent_lost` or completes matching update activation heartbeat. Later
  telemetry can still mark the client `online`, but telemetry does not set the
  process incarnation. Operators can then see an apparently online VPS whose
  jobs bind to a stale or missing incarnation and either cannot be claimed or
  are rejected by the gateway with an incarnation mismatch. Update activation
  and long-running job state become misleading until another successful hello
  occurs.
- Evidence: `crates/gateway/src/api_client.rs:430-431` defines the lifecycle
  TTL as `CRITICAL_EVENT_TTL = 300s`; `crates/gateway/src/api_client.rs:1689-1691`
  maps `session-started`, `session-ended`, and `agent-hello` to
  `GatewayForwardEventKind::Lifecycle`; `crates/gateway/src/api_client.rs:1701-1706`
  applies the lifecycle TTL; and
  `crates/gateway/src/api_client.rs:1348-1358` drops expired gateway events.
  During `ClientHello`, `crates/gateway/src/main.rs:780-824` posts
  `agent-hello`, stores the session with `process_incarnation_id`, enqueues
  `session-started`, and sends `ServerHello`. The API only updates
  `clients.process_incarnation_id` and reconciles old-incarnation targets in
  accepted hello handling at `crates/api/src/repository_ingest.rs:494-603`.
  Telemetry updates `status`, `last_ip`, and `last_seen_at` without updating
  incarnation at `crates/api/src/repository_ingest.rs:734-748`. Dispatch claim
  relies on non-null/matching `clients.process_incarnation_id` at
  `crates/api/src/repository_jobs.rs:1576-1602`, and gateway dispatch rejects
  a live session whose incarnation differs from the API expectation at
  `crates/gateway/src/control.rs:272-285`.
- Notes: This is distinct from AUD-077, which covers terminal final stream
  expiration; AUD-127, which covers controlled shutdown loss; AUD-150, which
  covers displaced sessions still forwarding telemetry; and AUD-191, which
  covers multi-gateway control routing. A clean fix should make agent hello
  acceptance and lifecycle reconciliation durable before the session becomes
  dispatchable, or persist/retry lifecycle events with semantics appropriate
  for control-plane state rather than dropping them after a short TTL.

### AUD-194: Manual Release Workflow Can Publish Tag-Named Update Assets From The Wrong Commit

- Severity: High
- Status: Confirmed
- Area: Release/Updates/Supply Chain
- Context: The official agent updater uses the GitHub release `version.json`,
  `SHA256SUMS`, and tag-pinned asset URLs as the default update source. The
  Docker deployment updater uses the same release assets for server and
  frontend deployment updates.
- Root Cause: The release workflow supports `workflow_dispatch` with an
  operator-supplied tag name, stamps `VPSMAN_RELEASE_TAG` and
  `VPSMAN_RELEASE_VERSION` from that input, and builds binaries from the
  workflow checkout commit. The publish job writes `version.json` with that
  same tag and the checkout `GITHUB_SHA`, then uploads or clobbers assets on
  the named GitHub release. It does not verify that the supplied tag resolves
  to the same commit that produced the artifacts before upload.
- Impact: A normal manual rerun from the default branch or wrong ref can
  publish assets named and versioned as an existing release tag while their
  bytes come from a different commit. Agents can then accept the official
  manifest for `vX.Y.Z`, verify `SHA256SUMS`, and install a binary that is not
  actually the code behind tag `vX.Y.Z`. Operators lose a reliable relationship
  between release tag, manifest commit, checksums, and deployed agent/server
  behavior, which is serious for production rollback, incident forensics, and
  fleet-wide update safety.
- Evidence: `.github/workflows/release.yml:7-12` enables manual dispatch with a
  free-form `tag` input. The Linux binary job derives release identity from
  `inputs.tag` when the workflow ref is not a tag at
  `.github/workflows/release.yml:39-57`, then builds and stages binaries at
  `.github/workflows/release.yml:70-92`. The publish job repeats the same tag
  derivation at `.github/workflows/release.yml:169-185`, writes `version.json`
  with `"tag": release_tag` and `"commit": GITHUB_SHA` at
  `.github/workflows/release.yml:219-228`, and uploads/clobbers release assets
  at `.github/workflows/release.yml:252-271`. The build helper embeds
  explicit `VPSMAN_RELEASE_TAG` / `VPSMAN_RELEASE_VERSION` without checking the
  git tag target at `build/build-support/src/lib.rs:15-31` and
  `build/build-support/src/lib.rs:51-68`.
- Notes: This is distinct from AUD-084, which covered HTTP redirects during
  update downloads; AUD-149, which covers local compose update process
  recreation; and AUD-162, which covers downgrade selection from an older
  manifest. A clean fix should either remove manual release dispatch for
  production releases or require the workflow to fetch tags and prove
  `git rev-parse "$release_tag^{commit}" == "$GITHUB_SHA"` before building or
  uploading assets.

### AUD-195: Documented Dev Internal Token Bypasses Placeholder Startup Validation

- Severity: Medium/High
- Status: Confirmed
- Area: API/Gateway/Security/Docs
- Context: The API-to-gateway internal token protects private control-plane
  ingest and gateway-control requests. Repository maintenance rules require
  startup validation to reject missing, short, or placeholder internal tokens.
- Root Cause: The API and gateway reject a small literal placeholder set, but
  the operator quickstart and local control-plane tutorials still document
  `dev-internal-token-change-me-32chars`. That value is long enough and is not
  in the rejected placeholder list, so both services accept it as a valid
  internal token.
- Impact: An operator can copy the shipped tutorial token into a durable local
  or adapted deployment and receive no startup failure, even though the token
  explicitly says `change-me`. If API, gateway control, or internal ingest is
  accidentally reachable, the private control-plane bearer token is predictable
  and shared by every tutorial-following environment. This weakens the same
  boundary used for gateway event ingest, dispatch/cancel control, session
  disconnect, and privilege verification forwarding.
- Evidence: `tutorials/00-operator-quickstart.md:11-17` and
  `tutorials/01-local-control-plane.md:54-60` export
  `VPSMAN_INTERNAL_TOKEN=dev-internal-token-change-me-32chars`. The API
  validator only rejects `change-me`, `change-me-internal-token`, and
  `replace-with-random-token-at-least-32-chars` at
  `crates/api/src/main.rs:655-673`; the gateway validator has the same rejected
  set at `crates/gateway/src/main.rs:423-441`.
- Notes: This is distinct from AUD-066/AUD-067/AUD-146 API exposure issues.
  Those cover reachability defaults; this issue covers startup accepting a
  documented placeholder secret after the project explicitly required
  placeholder rejection.

### AUD-196: Manual Quickstart No Longer Starts A Usable Postgres-Backed API

- Severity: Medium
- Status: Confirmed
- Area: Docs/Local Control Plane
- Context: The operator quickstart and local-control-plane tutorial are the
  shortest documented path for starting API, gateway, worker, frontend, and one
  test VPS outside Docker Compose.
- Root Cause: The tutorials still show manual startup commands that set bind
  addresses, internal token, and object-store paths, then run `cargo run -p
  vpsman-api`, `vpsman-gateway`, and `vpsman-worker`. They do not start
  PostgreSQL or set `VPSMAN_POSTGRES_URL`, while the production API repository
  connector now requires a Postgres URL and no longer falls back to a runnable
  memory store.
- Impact: Following the current quickstart or manual local-control-plane
  tutorial fails before the API can serve operators, and the worker runs without
  processing durable queues if started without Postgres. This creates a
  practical onboarding and smoke-test failure for operators trying to validate
  the release or reproduce production behavior locally, and it can push users
  toward ad hoc environment edits instead of the intended Postgres-backed
  control plane.
- Evidence: `tutorials/00-operator-quickstart.md:11-23` and
  `tutorials/01-local-control-plane.md:54-73` run API/gateway/worker manually
  without `VPSMAN_POSTGRES_URL` or a Postgres startup step. The API CLI arg is
  optional at parse time at `crates/api/src/main.rs:213-214`, but startup calls
  `Repository::connect(args.postgres_url.as_deref(), ...)` at
  `crates/api/src/main.rs:551-552`; `Repository::connect` immediately fails
  when no URL is provided at `crates/api/src/repository.rs:99-105`. The worker
  also warns and skips queue processing indefinitely without Postgres at
  `crates/worker/src/main.rs:586-595`.
- Notes: This is distinct from AUD-190, which covers compose `.env` password
  edits diverging from the suite-config Postgres URL. This issue is the manual
  tutorial path omitting Postgres entirely after the memory repository stopped
  being a production startup mode.

### AUD-197: API And Worker Containers Can Read Gateway-Only Secret Material

- Severity: High
- Status: Confirmed
- Area: Deploy/API/Gateway/Secrets
- Context: The privilege model keeps the super password in the browser/CLI and
  requires the private gateway to verify request-bound privilege assertions.
  The API should recompute canonical intent and forward assertions, but it
  should not hold verifier material that would let it approve privileged work
  by itself. The gateway also owns the Noise private key used for enrolled
  agent transport identity.
- Root Cause: The shipped compose file mounts the entire `./config/secrets`
  directory into the API, gateway, and worker containers. The shipped suite
  config stores the gateway private key at
  `/run/secrets/vpsman_gateway_private_key_hex` and the gateway-only privilege
  verifier key at `/run/secrets/vpsman_privilege_verifier_key_hex`. The API
  startup guard only rejects the `VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX`
  environment variable; it does not prevent gateway-only key files from being
  present in the API container.
- Impact: In the official compose shape, any API-container or worker-container
  compromise can read the same verifier key the gateway uses to validate
  privileged assertions, and can also read the gateway Noise identity key. That
  collapses the intended API/gateway separation: code running in the API
  container already has the internal gateway-control token and, with the
  verifier key, can forge request-bound privilege approval instead of only
  forwarding operator-supplied assertions. Gateway identity exposure also
  enlarges the blast radius of an API/worker compromise into the agent transport
  trust root. This materially weakens the safety model for destructive jobs,
  backups/restores, network changes, agent updates, and user-management
  privilege gates.
- Evidence: `deploy/compose.yml:23-27` mounts `./config/secrets:/run/secrets:ro`
  into the API service, and `deploy/compose.yml:55-59` mounts the same secrets
  directory into the worker service. `deploy/config/vpsman.toml:80-83` defines
  both `gateway_private_key_file =
  "/run/secrets/vpsman_gateway_private_key_hex"` and
  `privilege_verifier_key_file =
  "/run/secrets/vpsman_privilege_verifier_key_hex"`. The API calls
  `reject_api_privilege_verifier_env()` at `crates/api/src/main.rs:551`, but
  that guard only checks the verifier environment variable at
  `crates/api/src/main.rs:676-685`. The gateway legitimately reads the verifier
  key file from suite config at `crates/gateway/src/main.rs:247-251`.
- Notes: This is distinct from AUD-102, AUD-121, and AUD-151, which cover
  request-bound privilege checks around specific mutations. This issue is the
  deployment secret boundary itself. A clean fix should provide service-specific
  secret mounts so only the gateway receives gateway identity and verifier
  material, while API and worker receive only the secrets they actually need.

### AUD-198: S3-Compatible Object Store Accepts Plaintext HTTP Endpoints For Signed Requests

- Severity: Medium/High
- Status: Confirmed
- Area: API/Worker/Object Storage/Security
- Context: The default object store is local filesystem, but production
  operators can explicitly configure the documented S3/MinIO-compatible object
  store for backup artifacts, file-transfer artifacts, and large job-output
  objects.
- Root Cause: The S3 endpoint parser accepts arbitrary `http://` endpoints as
  well as `https://` endpoints. The API and worker pass
  `VPSMAN_OBJECT_ENDPOINT` straight into that parser. The S3 adapter then
  computes SigV4 headers, including `Authorization`, and sends signed
  `HEAD`/`PUT`/`GET`/`DELETE` requests to the configured URL without requiring
  TLS, a localhost-only exception, or an explicit unsafe opt-in for plaintext
  transport.
- Impact: If an operator configures a remote or shared-network S3/MinIO
  endpoint with `http://`, the API and worker send object-store access-key
  signatures and artifact bytes over plaintext. A network observer or
  man-in-the-middle can capture reusable object-store credentials, read or
  tamper with backup/file-transfer/job-output artifacts in transit, forge
  object-existence behavior, or interfere with cleanup and download flows. This
  is practical because the S3/MinIO object-store path is documented and the
  smoke test uses a plaintext MinIO endpoint for local development, but neither
  code nor docs distinguish local-only plaintext from production endpoints.
- Evidence: `crates/object-store/src/lib.rs:372-379` defines the S3 object-store
  settings, and `crates/object-store/src/lib.rs:391-410` constructs the store
  from the configured endpoint. `crates/object-store/src/lib.rs:640-690`
  builds the SigV4 `Authorization` header and sends the request to
  `self.endpoint.url(...)`. `crates/object-store/src/lib.rs:732-740` accepts
  both `https://` and `http://` endpoint schemes. The API and worker pass
  `VPSMAN_OBJECT_ENDPOINT` through unchanged at
  `crates/api/src/main.rs:714-745` and `crates/worker/src/main.rs:457-488`.
  The operator tutorial describes S3/MinIO setup as SigV4 over the configured
  endpoint at `tutorials/07-backup-restore-migration.md:162-166`, while the
  MinIO smoke test uses local plaintext HTTP at
  `scripts/smoke-minio-backup-artifact.sh:30-75`.
- Notes: A clean fix should keep local development possible, such as
  `localhost` or `127.0.0.1` MinIO, while rejecting remote plaintext endpoints
  by default or requiring an explicit unsafe local-only override. This is
  distinct from AUD-081/AUD-082 filesystem and temp-file permission issues,
  AUD-106 backup object verification, and AUD-179/AUD-180 object-key collision
  and cleanup-order issues.

### AUD-199: Job-Output And File-Download Archive Exports Can Exhaust API Temp Disk Across Targets

- Severity: High
- Status: Confirmed
- Area: API/Frontend/Job Outputs/Resource Bounds
- Context: Operators can download a job-output archive or a multi-target
  file-download bundle from normal dashboard workflows after running bulk jobs
  across many VPSs. This is a practical 20+ VPS workflow, especially for
  incident evidence, diagnostics, and bulk file collection.
- Root Cause: The API enforces only per-entry or per-stream limits while
  building these exports. It first spools each selected client/stream payload to
  a temp file, then writes a second full tar archive temp file before streaming
  the response. There is no aggregate selected-client cap, total archive-size
  cap, disk reservation, or streaming tar path that releases per-entry temp
  files as it goes.
- Impact: A single authenticated `jobs:read` operator action can fill the API
  host or container temp filesystem. For example, a file-download bundle across
  20 VPSs can retain up to roughly 20 GiB of per-client payload temps, then
  create another roughly 20 GiB tar temp before the response starts. A
  job-output archive can be larger because stdout and stderr are capped
  separately per client. Disk exhaustion can break unrelated API workflows,
  object-store spooling, backup handoff, history exports, and deployment
  health, turning a normal evidence-export action into a control-plane outage.
- Evidence: `crates/api/src/routes.rs:333-340` exposes
  `/api/v1/jobs/{job_id}/outputs/download-bundle` and
  `/api/v1/jobs/{job_id}/outputs/archive`. The file-download bundle route
  groups every selected client's outputs and spools each payload before writing
  the final archive at `crates/api/src/routes_job_history.rs:158-208`.
  The job-output archive route does the same for per-client stdout/stderr
  entries at `crates/api/src/routes_job_history.rs:235-275` and
  `crates/api/src/routes_job_history.rs:446-475`. The only hard caps are
  `MAX_FILE_DOWNLOAD_BUNDLE_ENTRY_BYTES` and
  `MAX_JOB_OUTPUT_ARCHIVE_STREAM_BYTES`, each 1 GiB, at
  `crates/api/src/routes_job_history.rs:32-34`; the spool helpers enforce
  those caps per entry/stream at `crates/api/src/routes_job_history.rs:479-554`.
  Archive writers then create an additional tar temp file at
  `crates/api/src/routes_job_history.rs:588-682`, and streaming starts only
  after that tar is complete at `crates/api/src/routes_job_history.rs:711-765`.
  The dashboard exposes this as a normal action through
  `frontend/src/hooks/useJobsData.ts:191-223` and
  `frontend/src/panels/jobs/MultiFileActionsPanel.tsx:624-635`.
- Notes: This is distinct from AUD-082, which covers transient spool-file
  permissions. A clean fix should add an aggregate export budget and reject
  oversize selections before materialization, or stream the tar with bounded
  per-entry spooling and backpressure from a private spool directory. The UI
  should surface a clear error and guide operators to narrower target or stream
  selections instead of silently risking temp-disk exhaustion.

### AUD-200: Job-Output Listing And Chunk Downloads Load Entire Output History Without Pagination

- Severity: High
- Status: Confirmed
- Area: API/Frontend/CLI/Job Outputs/Resource Bounds
- Context: Operators inspect job details, follow command output, download
  individual output chunks, and run resumable file-transfer downloads through
  normal dashboard and CLI workflows. Long-running commands, noisy diagnostics,
  file-transfer chunks, and terminal-backed output can produce many retained
  `job_outputs` rows across a 20+ VPS fleet.
- Root Cause: The API's primary job-output read primitive returns every output
  row for a job in one unpaginated response and base64-encodes inline data for
  each row. Multiple targeted paths reuse that full-list primitive and filter
  in memory, including per-client downloads and single chunk download. The CLI
  follow path repeatedly polls the full output list and filters already-seen
  rows client-side instead of asking for output after a cursor.
- Impact: A realistic high-output or many-target job can make normal operator
  inspection and follow workflows slow or unusable. The API can allocate and
  serialize a large response for every dashboard refresh, output event, CLI
  follow poll, or single chunk download. The browser stores the full output
  vector in React state, and `vpsctl` can hit its API-response byte cap while
  following or fetching metadata needed for a file-transfer chunk. This can turn
  one noisy job into repeated API CPU/memory pressure and prevent operators
  from retrieving the exact output evidence they need during an incident.
- Evidence: `Repository::list_job_outputs` selects all rows for a job with no
  `LIMIT`, cursor, client filter, stream filter, or metadata-only mode at
  `crates/api/src/repository_job_outputs.rs:42-99`. The public list route
  returns that full vector at `crates/api/src/routes_job_history.rs:147-155`.
  Targeted download routes still call the full-list primitive before filtering:
  file-download-for-client at `crates/api/src/routes_job_history.rs:302-320`,
  stream download at `crates/api/src/routes_job_history.rs:339-368`, and
  single output chunk download at
  `crates/api/src/routes_job_history.rs:882-896`. The dashboard loads full
  outputs through `frontend/src/hooks/useJobsData.ts:176-179`, stores them in
  job detail state at `frontend/src/panels/JobHistoryPanel.tsx:245-260`, loads
  them whenever a job is opened at
  `frontend/src/panels/JobHistoryPanel.tsx:366-405`, and reloads the full job
  output set on every live output event for the selected job at
  `frontend/src/panels/JobHistoryPanel.tsx:543-547`. The CLI `job outputs`
  command prints the full response at `crates/vpsctl/src/commands_jobs.rs:203-208`,
  and `job follow` polls `/outputs`, parses the complete list, and only then
  de-duplicates locally at `crates/vpsctl/src/commands_jobs.rs:227-291`.
  Resumable file-transfer downloads also fetch and parse the full output list
  for each chunk step at
  `crates/vpsctl/src/commands_file_transfer_download.rs:407-465`.
- Notes: This is distinct from AUD-073, which is about missing storage
  retention for terminal output, and from AUD-199, which is about export temp
  disk usage. A clean fix should add paginated and filtered output APIs
  (`client_id`, stream, `seq_after`, limit, metadata-only/include-data controls)
  plus direct repository lookups for single output rows. Frontend and CLI
  follow views should poll incrementally from a cursor, and chunk/file-transfer
  download paths should fetch only the exact needed row or stream instead of
  materializing the whole job history.

### AUD-201: Server-Side File-Transfer Handoff Scans All Client Chunks And Leaks Temp Files On Failed Assembly

- Severity: High
- Status: Confirmed
- Area: API/Frontend/CLI/File Transfers/Resource Bounds
- Context: Operators use resumable file-transfer downloads to pull large files
  from a VPS, then create a server-side handoff artifact so the completed
  transfer can be downloaded or reused without rerunning the agent-side job.
  The dashboard exposes single and bulk "Download handoffs" actions, and the
  CLI exposes `file-transfer-handoff`.
- Root Cause: Creating one handoff asks the repository for every retained
  `file_transfer_download_chunk` output for the client, regardless of session,
  then filters the requested session in memory. The Postgres query has no
  session predicate, row limit, or cursor and base64-encodes each retained row's
  inline preview. After that broad scan, handoff assembly writes a named temp
  file, but `create_file_transfer_handoff` only removes that temp file after
  object-store upload success or failure. If assembly itself fails because of a
  missing/gapped chunk, duplicate retry conflict, object-store read failure,
  hash mismatch, or size mismatch, the partially written temp file is left on
  the API host.
- Impact: A normal operator handoff for one session can repeatedly scan and
  allocate metadata for old unrelated download sessions from the same VPS.
  In long-lived 20+ VPS operation, one busy client can accumulate many download
  chunk outputs, making handoff creation increasingly expensive. When assembly
  fails, retries can also leave large partial temp files behind, filling the API
  temp filesystem and breaking unrelated control-plane workflows. This is
  practical because resumable downloads are explicitly retryable and AUD-166's
  duplicate-chunk condition can make handoff assembly fail during ordinary
  recovery.
- Evidence: `create_file_transfer_handoff` creates a temp path and calls
  `write_handoff_temp_file` before any cleanup guard at
  `crates/api/src/routes_file_transfers.rs:182-230`.
  `write_handoff_temp_file` loads chunks via
  `list_file_transfer_download_handoff_chunks` at
  `crates/api/src/routes_file_transfers.rs:471-482`, opens the temp file at
  `crates/api/src/routes_file_transfers.rs:486-490`, then can return errors
  after gap, size, hash, or object-load failures at
  `crates/api/src/routes_file_transfers.rs:492-570`. The caller removes the
  temp file only after `store.put_file_idempotent` returns at
  `crates/api/src/routes_file_transfers.rs:218-228`, so errors from
  `write_handoff_temp_file` bypass cleanup. The repository handoff query first
  calls `list_file_transfer_download_chunk_outputs(client_id)` at
  `crates/api/src/repository_file_transfers.rs:208-218`; the Postgres branch
  selects all `file_transfer_download_chunk` outputs for that client with no
  session predicate or limit at
  `crates/api/src/repository_file_transfers.rs:260-285`, and only later filters
  by session while building handoff chunks at
  `crates/api/src/repository_file_transfers.rs:678-717`. The dashboard invokes
  these paths from single and bulk handoff actions at
  `frontend/src/panels/jobs/FileTransferSessionsPanel.tsx:62-109` and
  `frontend/src/panels/jobs/FileTransferSessionsPanel.tsx:271-315`; the CLI
  invokes the same API at `crates/vpsctl/src/commands_file_transfers.rs:102-121`.
- Notes: This is distinct from AUD-166, which is about duplicate chunks making
  otherwise valid handoffs fail, and from AUD-199, which is about job-output
  archive temp-disk fan-out. A clean fix should query only chunk outputs for
  the requested `(client_id, session_id)`, avoid loading unrelated rows, stream
  or bound assembly memory, and wrap the handoff temp file in a cleanup guard so
  every failed assembly removes partial files before returning.

### AUD-202: Retained Backup Handoff Leaks Staging Files When Assembly Fails

- Severity: Medium/High
- Status: Confirmed
- Area: API/Backups/Resource Cleanup
- Context: Operators promote retained backup stdout into a durable backup
  artifact through the backup handoff route. This path is intended for normal
  recovery workflows where the agent already produced encrypted backup bytes
  as job output and the API turns those retained outputs into an object-store
  artifact.
- Root Cause: `stage_retained_backup_artifact_stdout` creates a named
  `{uuid}.part` file in the backup handoff staging directory and streams
  retained stdout into it, but it only removes the file for the empty-output
  case. Errors after the staging file is created, including object-store read
  failure, inline base64 decode failure, per-part hash/size mismatch, max-size
  overflow, or staging write/sync failure, return before the caller receives a
  `StagedRetainedBackupArtifact`. The route therefore has no path to remove
  the partial file.
- Impact: A failed retained-backup handoff can leave encrypted backup artifact
  bytes behind in the API staging directory. Repeated retries after a bad
  retained output, an object-store read problem, or an artifact larger than the
  configured limit can accumulate large partial files and fill `/tmp` or the
  configured `VPSMAN_BACKUP_HANDOFF_STAGING_DIR`. On filesystem-default
  deployments this can break unrelated API workflows that also need temp disk
  for downloads, archives, file transfers, object spooling, or future backup
  handoffs.
- Evidence: The staging file is created at
  `crates/api/src/backup_handoff.rs:25-35`. The function can return errors
  during object-store loading and inline decoding at
  `crates/api/src/backup_handoff.rs:45-102`, during streaming from a
  filesystem-backed object at `crates/api/src/backup_handoff.rs:140-149`, and
  during max-size or write checks at `crates/api/src/backup_handoff.rs:160-170`.
  It removes the staging file only for empty stdout at
  `crates/api/src/backup_handoff.rs:104-107`. The route calls the helper at
  `crates/api/src/routes_backups.rs:639-645`; if that helper returns an error,
  no prepared path exists for route-level cleanup.
- Notes: This is distinct from AUD-082, which covers temporary file
  permissions, AUD-168/AUD-203, which cover memory pressure, and AUD-062, which
  covers durable object metadata/cleanup-registry consistency. The clean fix is
  a cleanup guard around the staging file for every failed assembly path.

### AUD-203: Retained Backup Handoff Rehydrates The Whole Artifact In API Memory After Streaming

- Severity: Medium/High
- Status: Confirmed
- Area: API/Backups/Resource Bounds
- Context: Retained backup handoff streams stdout chunks into a staging file so
  an operator can promote an existing backup job output into a durable backup
  artifact without asking the agent to run the backup again.
- Root Cause: After streaming retained outputs into the staging file, the route
  immediately reads the entire staged artifact back into a `Vec<u8>` for
  validation. The validator then parses the whole JSON document and decodes the
  full `ciphertext_base64` into another in-memory buffer before the object is
  committed.
- Impact: A workflow that appears disk-streamed still allocates the full
  encrypted backup artifact, plus decoded ciphertext, in API memory. With the
  default 128 MiB handoff limit, a few concurrent handoffs can create large
  memory spikes; if operators raise `VPSMAN_BACKUP_HANDOFF_MAX_BYTES`, the
  spike scales with that setting. This is practical for 20+ VPS deployments
  because retained backup promotion is a normal recovery and migration workflow,
  not a synthetic stress path.
- Evidence: The staging helper streams retained outputs into a file at
  `crates/api/src/backup_handoff.rs:21-116`, including chunked file reads at
  `crates/api/src/backup_handoff.rs:129-151`. The handoff route then reads the
  full staged file with `tokio::fs::read` at
  `crates/api/src/routes_backups.rs:645-653` and passes the full buffer to
  `validate_encrypted_backup_artifact_with_limit` at
  `crates/api/src/routes_backups.rs:654-660`. That validator parses the full
  JSON and decodes `ciphertext_base64` at
  `crates/api/src/routes_backups.rs:1102-1145`.
- Notes: This is the retained-backup handoff analogue of AUD-168's chunked
  upload commit memory issue. A clean fix should validate retained backup
  handoff artifacts with a bounded streaming or incremental parser/hash path,
  or keep the handoff limit explicitly low enough that the memory cost is part
  of the supported API capacity model.

### AUD-204: Abandoned Chunked Backup Upload Sessions Can Leave Staging Files Indefinitely

- Severity: Medium/High
- Status: Confirmed
- Area: API/Frontend/CLI/Backups/Resource Cleanup
- Context: Operators use chunked backup artifact upload for artifacts larger
  than the inline request limit. Browser tab closes, network failures, CLI
  interruption, validation failures, and operator cancellation are ordinary
  production conditions during backup and migration work.
- Root Cause: Backup artifact upload sessions write a `.part` staging file and
  a JSON manifest under `VPSMAN_BACKUP_UPLOAD_STAGING_DIR` or the default temp
  directory. The session has a 24-hour expiry, but expired-session cleanup runs
  only opportunistically when a new upload session is created. There is no API
  startup cleanup, periodic worker cleanup, request-end cleanup, or dashboard/
  CLI abort path for interrupted uploads.
- Impact: A failed or abandoned chunked upload can leave encrypted backup
  artifact bytes in the API temp/staging directory indefinitely if no later
  upload session is created. Each abandoned session can be as large as the
  configured backup artifact streaming limit, 128 MiB by default. In a 20+ VPS
  deployment where operators routinely test restores, migrations, and backup
  uploads, repeated abandoned sessions can fill the API temp filesystem and
  break unrelated downloads, archives, object-store spooling, file-transfer
  handoffs, and future backup uploads.
- Evidence: Session creation creates the staging file and manifest at
  `crates/api/src/backup_upload_sessions.rs:78-120`. Chunk writes append to the
  staging file and resave the manifest at
  `crates/api/src/backup_upload_sessions.rs:123-173`. Cleanup is invoked only
  from `create` at `crates/api/src/backup_upload_sessions.rs:85-88`, and
  `cleanup_expired` scans only manifest files at
  `crates/api/src/backup_upload_sessions.rs:291-312`. The dashboard chunked
  upload flow creates a session, loops chunks, and commits without a
  `finally`/abort path at `frontend/src/hooks/useBackupsData.ts:142-189`.
  The CLI chunked upload path has the same create/chunk/commit flow without
  abort-on-error or interrupt cleanup at
  `crates/vpsctl/src/commands_backups.rs:364-447`.
- Notes: This is distinct from AUD-082, which covers temp-file permissions,
  AUD-168, which covers chunked commit memory pressure, AUD-202, which covers
  retained-backup handoff staging leaks, and AUD-062, which covers durable
  object metadata/cleanup-registry consistency. A clean fix should add a
  reliable cleanup path for expired and orphaned upload session files, and make
  dashboard/CLI callers abort sessions on recoverable failures when they still
  know the upload ID.

### AUD-205: Restore Post-Hooks Can Fail Without Making The Restore Target Fail Safely

- Severity: High
- Status: Confirmed
- Area: Agent/Backups/Restore
- Context: Restore and migration-restore jobs can run an operator-supplied
  post-restore command after restored files have already been written. In
  production this hook is a natural place to reload services, run a validation
  script, or verify that restored application state is healthy.
- Root Cause: The restore implementation treats a completed post-restore
  process as a successful restore command regardless of the process exit code.
  It records `"post_restore": {"status":"failed"}` inside the status payload
  when the hook exits nonzero, but the final `CommandOutput` still has
  `exit_code: 0` and `done: true`. Hook timeout/cancellation is handled as a
  command error only after files have been restored, and that error path is
  converted to a generic failed output without the normal `restored_files`
  rollback evidence.
- Impact: A restore can show as completed even though the operator's
  post-restore validation or service-restart hook failed. Conversely, a hook
  timeout can mark the target failed after mutating files but leave operators
  without the durable `restored_files` and rollback paths needed by the normal
  restore-rollback workflow. At 20+ VPS scale this can hide bad restores during
  incident recovery or leave partially restored machines that cannot be rolled
  back through the intended UI/CLI path.
- Evidence: `restore_archive` writes files first, then calls
  `run_post_restore_argv` at `crates/agent/src/restore.rs:215-225`. The final
  restore status is emitted with `exit_code: Some(0)` at
  `crates/agent/src/restore.rs:227-250`. `run_post_restore_argv` returns
  `Ok(post_restore_output_status(...))` for any completed process at
  `crates/agent/src/restore.rs:490-520`, and `post_restore_output_status`
  records nonzero exits only as `"status": "failed"` at
  `crates/agent/src/restore.rs:545-558`. If the hook errors or times out,
  `command_result_outputs` replaces the command result with a generic failed
  final output at `crates/agent/src/runtime.rs:827-864`, which does not include
  restored-file rollback metadata.
- Notes: This is distinct from restore staging/path-race issues and stale
  restore confirmations. A clean fix should make post-restore failure semantics
  explicit: either treat nonzero hooks as target failure with durable rollback
  evidence, or require hooks to be informational and label them so operators do
  not rely on them for restore success. Timeout/error handling should preserve
  enough restored-file evidence for safe rollback after files have already been
  changed.

### AUD-206: Alert Notification Delivery Kinds Can Be Saved But Cannot Be Delivered By The Shipped Worker

- Severity: Medium/High
- Status: Confirmed
- Area: API/Worker/Frontend/Alerts
- Context: Operators can create fleet alert notification channels for critical
  alerts. The default worker then processes queued notification deliveries in
  normal production deployments.
- Root Cause: The alert notification channel repository accepts any token-like
  `delivery_kind` and only checks that `target` is non-empty. It marks every
  non-`audit_log` kind as queued for delivery. The shipped API and worker
  delivery processors only implement `audit_log`, `webhook`, and
  `webhook_json`; every other kind is converted to a failed delivery with
  `notification delivery adapter '<kind>' is not configured`. The frontend
  compounds this by offering `email` and `slack` in the delivery-kind datalist
  even though neither adapter exists.
- Impact: An operator can save an apparently valid alert channel, including UI
  suggested kinds such as `email` or `slack`, and matching critical alerts will
  create queued delivery records that the shipped worker later fails. In a
  20+ VPS fleet this can make production alerting silently nonfunctional until
  someone inspects failed delivery history. A webhook channel with a non-URL
  target has the same late-failure shape because URL validation happens during
  delivery rather than channel save.
- Evidence: `notification_status_for_kind` returns `queued` for all
  non-`audit_log` kinds at
  `crates/api/src/repository_alert_notifications.rs:651-656`.
  `channel_from_request` normalizes arbitrary token delivery kinds and calls
  only `validate_target` at
  `crates/api/src/repository_alert_notifications.rs:679-680`, while
  `validate_target` checks only length/null bytes at
  `crates/api/src/repository_alert_notifications.rs:873-879`.
  Alert dispatch copies that kind into queued candidates at
  `crates/api/src/fleet_alert_notifications.rs:148-160`. The manual API
  processor supports only `webhook`, `webhook_json`, and `audit_log` at
  `crates/api/src/fleet_alert_notifications.rs:298-306`, and the worker claims
  all queued deliveries without filtering by kind at
  `crates/worker/src/alert_notifications.rs:111-126` before failing unknown
  kinds at `crates/worker/src/alert_notifications.rs:220-225`. The frontend
  suggests unsupported `email` and `slack` kinds at
  `frontend/src/panels/FleetWorkspace.tsx:4078-4082`.
- Notes: This is distinct from alert delivery retry behavior. A clean fix
  should either restrict saved delivery kinds to the adapters actually shipped,
  with kind-specific target validation for webhook URLs, or introduce an
  explicit external-adapter ownership model so the default worker does not
  consume and fail custom delivery kinds.

### AUD-207: Schedules Keep Dispatching Privileged Jobs After Owner Disable/Delete Or Scope Loss

- Severity: High
- Status: Fixed
- Area: API/Worker/Schedules/Auth
- Context: Operators can create recurring schedules and backup policies that
  materialize privileged jobs later through the worker. User management can then
  disable, delete, downgrade, or remove scopes from the operator account that
  created or last updated that schedule.
- Root Cause: Operator disable/delete revokes that operator's sessions, while
  role/scope update changes the operator row, but neither path disables,
  transfers, or marks schedules owned by that operator. The schedule worker
  selects due schedules only by schedule state and due time, without joining
  `operators` or checking that `schedules.actor_id` still belongs to an active
  operator with the authority expected for the saved recurring job. It then
  inserts the materialized job and audit row with the stored `actor_id` and
  treats the saved definition as previously privilege-unlocked.
- Impact: An operator account can be disabled, deleted, downgraded, or stripped
  of the relevant scopes after compromise, offboarding, or normal access cleanup
  while its existing recurring privileged schedules continue to dispatch jobs
  across the fleet.
  In a 20+ VPS deployment this can keep backups, restores, file operations,
  scripts, network changes, or update jobs running under a no-longer-active
  operator identity. The audit trail still points at the old operator, but the
  permission model no longer has an active human owner for the automation.
- Evidence: `set_operator_status` revokes sessions for disabled/deleted
  operators at `crates/api/src/repository_auth.rs:1356-1362` but does not
  update schedules. `update_operator` changes role/scopes at
  `crates/api/src/repository_auth.rs:1121-1228` without schedule
  reconciliation. The worker due-schedule selection reads only from
  `schedules` with `enabled = TRUE`, `deleted_at IS NULL`, and `next_run_at <=
  now()` at `crates/worker/src/main.rs:1550-1573`. Materialization reloads the
  schedule from `schedules` without joining `operators` at
  `crates/worker/src/main.rs:1601-1625`, then inserts the job with
  `schedule.actor_id` at `crates/worker/src/main.rs:1844-1857` and records audit
  metadata saying the saved schedule intent was previously privilege-unlocked at
  `crates/worker/src/main.rs:1922-1960`.
- Notes: This is distinct from schedule confirmation and stale-target issues.
  A clean fix should define explicit ownership semantics: disable or require
  transfer/re-approval of schedules when their owner is disabled/deleted or loses
  required scopes, and make the worker visibly skip/disable due schedules whose
  owner is no longer authorized instead of silently dispatching them.
- Fix Notes: The schedule worker now revalidates the saved `actor_id` against
  an active operator with `jobs:write` and `schedules:write` before materializing
  jobs. Unauthorized due schedules are disabled with `actor_authority_revoked`
  and an audit row, without job creation.

### AUD-208: Backup-Policy Retention Prune Can Delete Backups After Policy Owner Loses Authority

- Severity: High
- Status: Fixed
- Area: Worker/Backups/Retention/Auth
- Context: Backup policies are stored as schedules and can be pruned
  automatically by the worker when `backup_policy_prune_enabled` is enabled.
  Operators may later be disabled, deleted, downgraded, or stripped of backup
  scopes after creating those policies.
- Root Cause: The backup-policy retention worker selects backup policies by
  schedule state and operation type only. It does not join `operators`, does not
  validate the schedule's current `actor_id`, and does not require that the
  owner remains active with backup authority before pruning. The audit row is
  written with `actor_id = NULL`, so the destructive automatic prune is not tied
  to a currently authorized operator either.
- Impact: An offboarded, compromised, or downgraded operator's old backup policy
  can continue deleting backup artifact metadata and, when object deletion is
  enabled, object-store bytes. In a 20+ VPS deployment this can remove restore
  evidence and retained encrypted backups after the human owner is no longer
  permitted to manage backups. The default config leaves the worker disabled,
  but the workflow is shipped, documented in config, and covered by smoke tests;
  once enabled, the missing owner check is production-real rather than a lab-only
  edge case.
- Evidence: Policy selection joins `backup_policies` to `schedules` and filters
  only by `schedule.enabled` and `schedule.operation ->> 'type' = 'backup'` at
  `crates/worker/src/backup_policy_retention.rs:107-125`. Prune execution clears
  `backup_requests.artifact_id`, deletes `backup_artifacts`, and marks
  `server_artifacts` deleting at
  `crates/worker/src/backup_policy_retention.rs:237-280`; when object deletion
  is enabled, the worker then deletes object-store keys at
  `crates/worker/src/backup_policy_retention.rs:165-182`. The audit insert uses
  `actor_id = NULL` at `crates/worker/src/backup_policy_retention.rs:290-300`.
  The shipped config exposes this worker at `deploy/config/vpsman.toml:45-50`.
- Notes: This is distinct from AUD-056's delete-before-object-success ordering
  and AUD-207's recurring job dispatch. A clean fix should share the schedule
  ownership model: skip/disable/require reapproval for policies whose owner is
  disabled, deleted, or no longer has backup authority before any retention
  prune mutates metadata or object bytes.
- Fix Notes: Backup-policy retention candidates now carry the schedule actor
  and the worker checks active `backups:write` plus `schedules:write` authority
  before candidate lookup or deletion. Revoked policies are skipped and audited.

### AUD-209: Queued Artifact Cleanup Can Delete Artifacts After Creator Disable/Delete Or Scope Loss

- Severity: High
- Status: Fixed
- Area: API/Worker/Server Jobs/Auth
- Context: Operators can preview and confirm a server-side artifact-cleanup job.
  The API stores the reviewed artifact set and the worker later claims the
  queued server job to tombstone metadata and delete object-store bytes.
- Root Cause: Server jobs store `created_by`, but the worker claim path selects
  queued artifact-cleanup jobs by `server_jobs.job_type` and `server_jobs.status`
  only. It does not join `operators`, does not verify that the creator is still
  active, and does not verify that the creator still has the authority needed
  for the artifact domains being deleted.
- Impact: A compromised, offboarded, deleted, disabled, or downgraded operator
  can leave behind a queued destructive cleanup job that still runs after the
  account loses authority. In a 20+ VPS deployment this can delete job-output,
  file-transfer, backup, or update artifacts from the control plane after access
  has supposedly been revoked. This is especially risky because artifact cleanup
  is intentionally asynchronous and destructive, so operators expect disabling
  an account or stripping scopes to stop that account's pending maintenance
  deletes before the worker executes them.
- Evidence: Creation requires current `jobs:write` and stores `created_by` at
  `crates/api/src/routes_server_jobs.rs:49-65` and
  `crates/api/src/repository_server_jobs.rs:105-180`. The worker claim query at
  `crates/worker/src/main.rs:1153-1176` selects only queued artifact-cleanup
  server jobs and marks them running without checking `server_jobs.created_by`
  against the current `operators` table. The subsequent cleanup path runs the
  deletion/tombstone workflow for the stored target set.
- Notes: This is distinct from AUD-049, which covers insufficient creation
  scope for backup artifacts; AUD-050, which covers reviewed-set consistency;
  and AUD-161, which covers running server jobs with no lease/reclaim model. A
  clean fix should either cancel/disable queued server jobs when their creator
  loses required authority, or make the worker visibly fail/skip the job before
  any artifact mutation when the creator is no longer active and authorized.
- Fix Notes: Artifact-cleanup server-job claims now include `created_by`; the
  worker fails the cleanup job with `actor_authority_revoked` unless the creator
  is still an active operator with `jobs:write`, before any artifact mutation.

### AUD-210: vpsctl Structured-Output Capture Writes Sensitive Stdout To Default-Permission Temp Files

- Severity: Medium/High
- Status: Fixed
- Area: CLI/Output/Security
- Context: Operators use `vpsctl --output json` or `--output pretty-json` from
  terminals, bastion hosts, automation runners, and CI jobs to normalize command
  output for scripting. The mode is enabled for every non-VTY command, including
  auth, user/session, job-output, terminal replay, rendered config, backup, and
  file-transfer commands.
- Root Cause: The output wrapper redirects process stdout into a named file
  under `std::env::temp_dir()` using `OpenOptions::create_new(true)` without
  setting owner-only permissions or using a private/unlinked temp file. On a
  normal umask such as `022`, the capture file is created as group/world
  readable for the lifetime of the command, then read back and removed after
  normalization.
- Impact: A local user or process on the same operator host, bastion, shared
  runner, or container namespace can race-read captured CLI stdout containing
  control-plane secrets and payload metadata. Practical exposed examples include
  login and refresh responses with access and refresh tokens, operator/session
  listings, job-output payloads with `data_base64` and object keys, terminal
  replay output, and rendered data-source hot-config TOML.
- Evidence: `vpsctl` applies output capture around command dispatch at
  `crates/vpsctl/src/commands.rs:20-30`, and enables it for every command except
  VTY at `crates/vpsctl/src/cli.rs:1051-1053`. The capture file is created and
  later removed at `crates/vpsctl/src/output.rs:112-161`, with the temp path
  built under `std::env::temp_dir()` at `crates/vpsctl/src/output.rs:181-186`.
  Auth responses include live tokens at
  `crates/api/src/repository_auth.rs:1974-2017`, and CLI login/refresh print
  those responses at `crates/vpsctl/src/commands_auth.rs:46-67`. Job output
  rendering includes `data_base64`, storage mode, and artifact object keys at
  `crates/vpsctl/src/commands_jobs.rs:448-460`; rendered hot config is printed
  at `crates/vpsctl/src/commands_inventory.rs:760-779`.
- Notes: This is distinct from AUD-085, which covers explicit local download
  staging paths. A clean fix should make capture storage owner-only from file
  creation, preferably by using a private temp directory or anonymous/unlinked
  temp file, and add a regression that verifies the capture file is not
  group/world readable while a command is running.

### AUD-211: Restore Jobs Do Not Bind The Declared Source Backup To The Submitted Archive Bytes

- Severity: High
- Status: Confirmed
- Area: API/CLI/Agent/Backups/Restore
- Context: Operators can restore a backup through the CLI or direct job API by
  naming a `source_backup_request_id` and pointing the target agent at an
  operator-staged archive file. This is practical during recovery/migration
  work, especially when an operator is restoring from externally staged files
  instead of the API object store.
- Root Cause: The restore command model treats `source_backup_request_id` as
  independent metadata and accepts `archive_path`, `archive_size_bytes`, and
  `archive_sha256_hex` as the actual restore source. The API job validator
  checks archive path shape, size/hash metadata, paths, and post-restore argv,
  but it does not verify that the staged archive belongs to the declared source
  backup. The agent restores the decoded archive and reports the archive client
  ID in status, but it has no authoritative source-backup record to compare
  against before mutation.
- Impact: A real operator can restore bytes from backup B while the job,
  privilege intent, audit context, restore plan, or migration link names backup
  A. This can write the wrong source system's files onto a target VPS while the
  durable control-plane evidence points at a different backup request. In a
  20+ VPS fleet this is a serious recovery and migration consistency risk:
  stale downloaded artifacts, similarly named files, copied commands, or shared
  recovery directories can produce a destructive wrong-source restore that is
  difficult to explain after the fact.
- Evidence: `JobCommand::Restore` carries `source_backup_request_id` and
  staged archive fields independently at `crates/common/src/protocol.rs`.
  Direct job validation requires an absolute archive path, positive size, and
  SHA-256 at `crates/api/src/job_request.rs`, but it does not look up the
  source backup or decode the archive before dispatch. The CLI restore path
  forwards the operator-provided archive path, size, and SHA-256 while reusing
  the operator-provided `source_backup_request_id` in
  `crates/vpsctl/src/commands_backups.rs`, and migration restore does the same
  through `crates/vpsctl/src/commands_migrations.rs`. Dashboard restore and
  migration restore forms collect the same staged archive metadata in
  `frontend/src/panels/backups/RestoreRunForm.tsx` and
  `frontend/src/panels/backups/MigrationLinkForm.tsx`.
- Notes: This is distinct from AUD-091, which covers mutable agent-local
  archive paths without a required hash, AUD-152, which covers hidden stale
  migration-run UI options, and fixed AUD-171, which covered plaintext restore
  archive persistence. A clean fix should bind restore archive identity to the
  declared source backup before dispatch: at minimum require CLI/API restore commands to
  prove the archive artifact client and hash match the selected backup request,
  and reject direct restore jobs whose source-backup metadata and archive
  identity cannot be reconciled.

### AUD-212: User-Session Inventory Timeouts Are Reported As Generic Failures

- Severity: Medium/High
- Status: Confirmed
- Area: Agent/API/User Sessions/Job Status
- Context: Operators can run user-session inventory jobs from the dashboard,
  CLI, or VTY to inspect logged-in sessions across selected VPSs. The default
  source is the Linux `w`/`who` preset, and configs can also use a bounded
  custom command for nonstandard images. In a production fleet, NSS/PAM lookup
  stalls, broken provider images, slow custom wrappers, or wedged login
  accounting can make this command hit the job timeout.
- Root Cause: The user-session executor delegates to the normal shell command
  runner, which correctly returns a final status output with
  `type = "command_timeout"` and exit code 124 when the child times out. The
  user-session wrapper then removes every final status output from that inner
  command and appends a replacement status with `type = "user_sessions"`,
  preserving only the exit code. The API target-status classifier treats a
  command as `agent_timeout` only when the final status JSON type is
  `command_timeout`; it does not infer timeout from exit code 124.
- Impact: A real timed-out user-session inventory job is stored and counted as
  a generic failed target instead of `agent_timeout`. Operators lose the
  distinction between "the user inventory source timed out on this VPS" and
  "the source ran and failed normally." At 20+ VPS scale this makes incident
  triage, alert counts, schedule health, and timeout-capacity tuning misleading
  for a built-in read-only inventory workflow.
- Evidence: The user-session dispatch branch calls `execute_user_sessions`
  directly at `crates/agent/src/executor.rs:345-347`. The shared child runner
  emits `type = "command_timeout"` on timeout at
  `crates/agent/src/executor.rs:566-581`. `execute_user_sessions` removes the
  inner done/status output and appends a new `user_sessions` status at
  `crates/agent/src/executor.rs:821-839`. The API maps timeout targets only
  through `output_indicates_timeout`, which checks the final status JSON type
  for `command_timeout`, at `crates/api/src/routes_jobs.rs:1057-1075` and
  `crates/api/src/routes_jobs.rs:1114-1127`. The dashboard presents this as a
  normal user-session job source at
  `frontend/src/panels/jobs/JobOperationControls.tsx:608-614`, and the CLI
  exposes the same operation at `crates/vpsctl/src/commands_process.rs:35-63`.
- Notes: This is distinct from AUD-163. AUD-163 covers custom JSON child
  processes that can outlive the configured timeout after stdout closes; this
  issue occurs after the timeout has already been detected and encoded, but the
  wrapper hides the timeout classification before the API sees it. A clean fix
  should preserve timeout and cancellation final statuses from the inner shell
  runner, or annotate user-session metadata without replacing terminal status
  types that drive target classification.

### AUD-213: Failed Backup Jobs Leave Auto-Created Backup Requests Permanently In Progress

- Severity: Medium/High
- Status: Confirmed
- Area: API/Frontend/Backups/Job Lifecycle
- Context: Operators can run backup jobs directly or through schedules. The
  dispatcher auto-creates or attaches a backup request record for each claimed
  backup target before delivering the command so the eventual encrypted
  artifact can be linked to backup history. In production, backup jobs can fail,
  time out, be rejected, lose the agent process, or fail dispatch before any
  artifact is produced.
- Root Cause: Backup request status only supports
  `requested_metadata_only` and `artifact_metadata_recorded`. The dispatcher
  records the backup request as `requested_metadata_only` before dispatch, and
  artifact recording later moves it to `artifact_metadata_recorded` only on a
  completed backup with a valid artifact. There is no failure status or
  reconciliation path that updates the auto-created backup request when the
  linked backup job target becomes `failed`, `rejected`, `agent_lost`,
  `agent_timeout`, `control_timeout`, or `canceled`.
- Impact: A failed backup execution leaves a durable backup request displayed
  as an in-progress metadata-only backup forever. At 20+ VPS scale, scheduled
  backups and bulk backup runs can accumulate stale "requested" rows that look
  like pending backups rather than terminal failures. This misleads operators
  reviewing backup health, selecting restore sources, auditing backup coverage,
  and diagnosing schedule outcomes after agent or gateway instability.
- Evidence: The canonical backup request statuses are only
  `requested_metadata_only` and `artifact_metadata_recorded` at
  `crates/common/src/protocol.rs:1769-1770` and
  `crates/common/src/protocol.rs:2210-2231`; generated frontend contracts mark
  `requested_metadata_only` as `in_progress` at
  `frontend/src/generated/protocolContracts.ts:626-635`. The dispatcher
  pre-records backup requests with `BackupRequestStatus::RequestedMetadataOnly`
  at `crates/api/src/job_dispatcher.rs:456-516`, then only attempts artifact
  auto-recording after successful backup target terminalization at
  `crates/api/src/job_dispatcher.rs:425-441`. Artifact metadata recording is
  the only normal status transition and sets `artifact_metadata_recorded` at
  `crates/api/src/repository_backup_artifacts.rs:312-327`. The backup request
  table renders these records directly in the dashboard at
  `frontend/src/panels/backups/BackupHistoryTables.tsx:241-254`, and restore
  planning lists backup requests from the same unfiltered collection at
  `frontend/src/panels/backups/RestorePlanForm.tsx:66-79`.
- Notes: This is distinct from AUD-106, which covers artifact metadata recorded
  without object-store verification, AUD-110/AUD-111, which cover
  migration/restore-plan consistency, and AUD-211, which covers restore archive
  bytes not being bound to a declared source backup. A clean fix should add a
  terminal unsuccessful backup-request status or derived job-status projection
  for auto-created backup requests, update it when the linked backup job target
  terminalizes unsuccessfully, and keep intentionally manual metadata-only
  requests distinguishable from failed execution-backed requests.

### AUD-214: Queued Jobs Keep Dispatching After Actor Disable/Delete Or Scope Loss

- Severity: High
- Status: Fixed
- Area: API/Dispatcher/Auth/Job Lifecycle
- Context: Operators create privileged jobs through the private API, dashboard,
  CLI, or schedules. Dispatch can be delayed by gateway downtime, offline
  agents, exclusive-job serialization, dispatch leases, or normal 20+ VPS
  queue depth. During that delay an admin may disable, delete, downgrade, or
  remove scopes from the actor because of offboarding, compromise response, or
  access cleanup.
- Root Cause: Job creation verifies the submitting operator's current
  `jobs:write` authority once and stores `jobs.actor_id`. Later dispatch claims
  queued or stale-dispatching targets by joining only `job_targets`, `jobs`, and
  `clients`; it does not join `operators`, require the actor to still be active,
  or re-check that the actor still has authority for the stored job command.
  Operator disable/delete revokes sessions, and role/scope update changes the
  operator row, but neither path cancels or revalidates already queued normal
  jobs.
- Impact: A privileged destructive job submitted before an operator is disabled
  or downgraded can still run afterward if it was queued or retry-delayed. In a
  20+ VPS deployment this can keep file writes/deletes, restores, network
  changes, process operations, scripts, backups, or update jobs executing after
  the human owner no longer has production authority. This weakens incident
  response: revoking sessions and removing scopes does not actually stop work
  already waiting in the control-plane queue.
- Evidence: Job creation requires `operator` role plus `jobs:write` at
  `crates/api/src/routes_jobs.rs:36-44`, then stores the actor on the job at
  `crates/api/src/repository_jobs.rs:1319-1332` and
  `crates/api/src/repository_jobs.rs:1404-1419`. Dispatch claim selects and
  promotes work from `job_targets`, `jobs`, and `clients` without an
  `operators` join or actor-status predicate at
  `crates/api/src/repository_jobs.rs:1567-1740`, returning the stored actor ID
  to the dispatcher at `crates/api/src/repository_jobs.rs:1748-1764`.
  Operator role/scope update mutates only the operator row and audit log at
  `crates/api/src/repository_auth.rs:1121-1228`. Operator disable/delete
  revokes sessions at `crates/api/src/repository_auth.rs:1231-1375`, but does
  not update queued jobs or job targets.
- Notes: This is distinct from AUD-207, which covers recurring schedules after
  owner authority changes; AUD-208, which covers backup-policy retention; and
  AUD-209, which covers queued artifact-cleanup server jobs. A clean fix should
  define an explicit policy for already-queued normal jobs when actor authority
  is revoked: cancel/skip them at operator lifecycle change, or make dispatch
  compare-and-set them to a visible terminal state if the stored actor is no
  longer active with the required scope. Already running jobs may need a
  separate policy, but queued and redispatchable jobs should not silently start
  after authority loss.
- Fix Notes: The API dispatcher now reloads `jobs.actor_id` authority before
  gateway dispatch. Targets whose actor is missing, inactive, or lacks
  `jobs:write` are terminalized as rejected with `actor_authority_revoked` and
  are not sent to the gateway.

### AUD-215: Terminal Replay Loads Full Session Output History Before Applying Replay Bounds

- Severity: High
- Status: Fixed
- Area: API/Frontend/CLI/Terminal/Resource Bounds
- Context: Operators can open long-lived terminal sessions during incident
  response, run noisy commands, and later use dashboard or CLI terminal replay
  to inspect persisted PTY output. The replay API exposes `limit`, `from_seq`,
  `max_bytes`, and `include_data` controls that appear to bound the request.
- Root Cause: The terminal replay repository first loads every `job_outputs` row
  belonging to every terminal job associated with the session, with no SQL
  `LIMIT`, terminal-sequence cursor, or byte budget. It also reads inline
  `output.data` and base64-encodes each row before `build_terminal_replay`
  applies the requested chunk limit. The route applies `max_bytes` and
  `include_data=false` even later, after the full session history has already
  been fetched and transformed in memory.
- Impact: A realistic high-output terminal session can make a normal replay,
  follow, or metadata-only request allocate and process the full retained PTY
  history even when the operator asks for a small bounded slice. In a 20+ VPS
  deployment this turns noisy terminal sessions into repeated API memory/CPU and
  database pressure, can make dashboard replay/follow and `vpsctl
  terminal-replay` unusable, and can interact badly with the existing unbounded
  terminal-output retention issue.
- Evidence: `terminal_session_replay` calls `list_terminal_replay_outputs`
  before any replay bounding at `crates/api/src/repository_terminal_sessions.rs:118-130`.
  The Postgres query selects all outputs for the session's terminal jobs and
  orders them without a limit at
  `crates/api/src/repository_terminal_sessions.rs:184-226`, then reads
  `output.data` and base64-encodes every row at
  `crates/api/src/repository_terminal_sessions.rs:227-250`. Only afterward does
  `build_terminal_replay` group all outputs, scan PTY chunks, sort all chunks,
  and truncate to the requested limit at
  `crates/api/src/repository_terminal_sessions.rs:712-818`. The API route then
  applies the 4 MiB `max_bytes` cap and `include_data` stripping at
  `crates/api/src/routes_terminal_sessions.rs:70-118`. The dashboard calls this
  path for replay/follow at
  `frontend/src/panels/jobs/TerminalSessionsPanel.tsx:57-84`, and the CLI uses
  it through `crates/vpsctl/src/commands_terminal_sessions.rs:74-115`.
- Notes: This is distinct from AUD-073, which covers terminal output storage
  growth, and from AUD-200, which covers general job-output listing and chunk
  downloads. A clean fix should push `client_id`, `session_id`, `from_seq`,
  `limit`, metadata/data selection, and byte-budget enforcement into repository
  queries or a streaming cursor so replay requests only materialize the reviewed
  bounded slice.

### AUD-216: Gateway Spool Replay Can Strand Valid Events After Per-Target Queue Saturation

- Severity: High
- Status: Confirmed
- Area: Gateway/Spool/Replay
- Context: Controlled gateway restart is expected to replay pending forwarder
  events from the gateway spool after API downtime, deploys, or operator
  restarts. A single busy VPS can generate hundreds of command-output,
  lifecycle, telemetry, or terminal-output events while the API is unavailable.
- Root Cause: Gateway startup replay scans spooled files once and immediately
  tries to enqueue every item into an in-memory per-target channel. If that
  target channel is already full, the code preserves the spooled file on disk
  and returns an enqueue error, but there is no later replay pass, wakeup, or
  background scanner in the same gateway process. The preserved file therefore
  remains valid but unprocessed until another gateway restart.
- Impact: After a normal controlled restart with API outage or slow API
  recovery, more than 512 pending events for one VPS can leave later events
  stranded on disk even after the gateway has successfully delivered the first
  queued replay batch. If one of the stranded events is final command output or
  lifecycle evidence, the API can show jobs timing out or missing restart/update
  evidence despite the gateway having a valid durable spool file. Operators then
  need another gateway restart or manual spool intervention to recover the
  event, which is not an expected production workflow for 20+ VPS operation.
- Evidence: `PER_TARGET_QUEUE_CAPACITY` is fixed at 512 in
  `crates/gateway/src/api_client.rs:427`. `start_spool_replay` performs a
  one-shot scan of `pending_items()` and logs enqueue failures without
  rescheduling them at `crates/gateway/src/api_client.rs:518-535`. When a
  spooled item hits a full target queue, `enqueue_queue_item` records a
  critical drop/error but intentionally preserves the spool file for later
  replay at `crates/gateway/src/api_client.rs:747-775`. The test
  `full_target_queue_preserves_spooled_command_output_file` verifies that the
  file remains on disk after this path at
  `crates/gateway/src/api_client.rs:2128-2176`, but no code consumes that
  preserved file again before the next process start.
- Notes: This is distinct from AUD-127, which covers losing queued RAM events
  during controlled shutdown, and from AUD-109, which covers deleting command
  output based on sequence-only ACK. A clean fix should make replay durable and
  self-draining: keep a pending-spool scanner or retry loop that re-enqueues
  preserved files after queue space opens, while preserving ordering per target
  and avoiding duplicate delivery beyond the existing idempotent ingest rules.

### AUD-217: Chunked Backup Artifact Upload Defaults Exceed The Route Body Limit

- Severity: High
- Status: Fixed
- Area: API/Frontend/CLI/Backups
- Context: Operators use chunked backup artifact upload for encrypted backup
  artifacts that are too large for the inline upload route. This is the normal
  dashboard and CLI path for retained backup artifacts above the small inline
  envelope, especially when restoring or migrating VPSs from stored artifacts.
- Root Cause: The backup upload-session code advertises and validates 4 MiB
  binary chunks, and both the dashboard and CLI default to sending 4 MiB chunks
  as base64 JSON. The chunk upload route itself does not install the enlarged
  `DefaultBodyLimit` used by direct artifact upload, so Axum's normal JSON body
  limit can reject the default chunk request before
  `upload_backup_artifact_session_chunk` reaches its own validator.
- Impact: The practical chunked upload workflow can fail for ordinary backup
  artifacts even though the server-created session reports `max_chunk_bytes =
  4194304` and clients follow that value. Frontend operators have no chunk-size
  control, so a large artifact selected in the dashboard can repeatedly fail at
  the first 4 MiB chunk. CLI/VTY operators can work around it only by manually
  choosing a much smaller chunk size, which is not an expected production
  workflow during restore or migration operations.
- Evidence: `MAX_BACKUP_ARTIFACT_UPLOAD_CHUNK_BYTES` is 4 MiB at
  `crates/api/src/backup_upload_sessions.rs:27-29`, and the session view
  exposes that value as `max_chunk_bytes` at
  `crates/api/src/backup_upload_sessions.rs:330-336`. The server validator
  permits base64 for that full chunk size at
  `crates/api/src/backup_upload_sessions.rs:367-399`. The frontend default is
  also 4 MiB and sends `data_base64` to the chunk route at
  `frontend/src/hooks/useBackupsData.ts:142-179`; the CLI default and allowed
  range are the same at `crates/vpsctl/src/commands_backups.rs:30` and
  `crates/vpsctl/src/commands_backups.rs:380-430`. The router defines the chunk
  endpoint at `crates/api/src/routes.rs:476-482` without a `DefaultBodyLimit`,
  while the direct upload route does have a larger body limit at
  `crates/api/src/routes.rs`.
- Notes: This is distinct from AUD-168, which covers memory pressure during
  chunked commit after upload succeeds, and AUD-204, which covers abandoned
  upload-session staging files. A clean fix should make the advertised
  `max_chunk_bytes`, frontend/CLI default chunk size, per-route body limit, and
  JSON/base64 overhead agree.
- Resolution: Fixed by adding a chunk-upload route body limit that explicitly
  covers the advertised 4 MiB binary chunk plus base64/JSON overhead. The
  advertised server chunk size and existing frontend/CLI defaults remain
  unchanged.

### AUD-218: Chunked File-Push Jobs Exceed The Job-Create Route Body Limit

- Severity: High
- Status: Fixed
- Area: API/Frontend/CLI/File Operations
- Context: Operators use file push from the dashboard, CLI, and VTY to place
  small operational files on selected VPSs. The system advertises inline push
  up to 1 MiB and non-resumable chunked push up to 8 MiB, while larger transfers
  use the separate resumable file-transfer workflow.
- Root Cause: `file_push_chunked` chunks the file inside one `JobCommand`, but
  the frontend and CLI still submit the whole base64 chunk list in a single
  `POST /api/v1/jobs` JSON request. The `/api/v1/jobs` route does not install a
  body limit that matches the 8 MiB command limit, so the request can hit Axum's
  default JSON body cap before `create_job` and `validate_chunked_file_push`
  run.
- Impact: A backend-valid, operator-reviewed file push between roughly the
  inline limit and the advertised chunked limit can fail from the dashboard or
  CLI even though the UI/CLI accepted the file and built a valid
  `file_push_chunked` command. In production this blocks common operational
  actions such as pushing generated configs, scripts, or bundles that are larger
  than 1 MiB but smaller than 8 MiB, and the failure appears as a transport/body
  rejection rather than a domain validation error.
- Evidence: Shared file-transfer limits define 1 MiB inline push, 64 KiB command
  chunks, and 8 MiB chunked push at `crates/common/src/file_transfer.rs:7-17`.
  The frontend reads the whole selected file, switches to `chunks` above 1 MiB,
  and allows up to 8 MiB at `frontend/src/fileTransfer.ts:27-58`. The dashboard
  dispatch flow builds that payload before creating the job at
  `frontend/src/panels/JobDispatchPanel.tsx:712-725`, and the CLI builds the
  same `JobCommand::FilePushChunked` at `crates/vpsctl/src/commands_files.rs:73-108`.
  The API validator accepts the chunked command at `crates/api/src/job_files.rs:37-47`,
  but the router maps `POST /api/v1/jobs` directly at
  `crates/api/src/routes.rs:293-297` without a matching `DefaultBodyLimit`.
- Notes: This is distinct from resumable file-transfer upload, which sends
  separate 64 KiB transfer chunks, and from AUD-217, which covers backup
  artifact upload-session chunks. A clean fix should either give job creation a
  precise body limit that matches all accepted command payloads plus JSON
  overhead, or steer non-trivial file pushes through the resumable workflow and
  stop presenting embedded `file_push_chunked` as an 8 MiB path.
- Resolution: Fixed by giving the job-create route a bounded body limit that
  covers the existing 8 MiB `file_push_chunked` command envelope plus
  base64/JSON overhead. Direct reviewed file push remains inline up to 1 MiB
  and chunked up to 8 MiB; larger files continue to use resumable transfer.

### AUD-219: Disabled Integrations Can Still Deliver Already Queued Outbound Work

- Severity: Medium/High
- Status: Confirmed
- Area: API/Worker/Integrations/Delivery State
- Context: Operators can disable webhook rules and fleet-alert notification
  channels to stop outbound HTTP delivery to integration endpoints. This is a
  normal production response when a webhook target is wrong, noisy,
  compromised, or under maintenance.
- Root Cause: Rule/channel `enabled` is checked when materializing new
  deliveries, but automatic workers and manual delivery processors select and
  send existing delivery rows using only delivery-row status. They do not join
  back to the current webhook rule or alert notification channel, and they do
  not re-check `enabled` immediately before HTTP. A delete cascades unclaimed
  rows, but it does not prevent a delivery already claimed and loaded by a
  worker from making the external HTTP call.
- Impact: An operator can disable an integration and still see queued or
  already-claimed rows call the external endpoint afterward. In a 20+ VPS
  deployment, alert and webhook queues can contain many rows during incidents;
  a disable action should be a reliable stop signal for future outbound side
  effects, not only for future materialization. This can leak event payloads to
  an endpoint the operator believed was disabled and can create duplicate/noisy
  downstream automation after a bad rule is turned off.
- Evidence: Webhook materialization lists only enabled rules at
  `crates/worker/src/webhook_rules.rs:213-244`, but delivery processing claims
  rows from `webhook_rule_deliveries` by status only at
  `crates/worker/src/webhook_rules.rs:694-735` and sends HTTP from the loaded
  row at `crates/worker/src/webhook_rules.rs:737-843`. The manual API webhook
  delivery processor also lists delivery rows by status only and sends HTTP at
  `crates/api/src/webhook_rules.rs:140-190`. Alert notification delivery has
  the same shape: channels store `enabled` at
  `migrations/0003_telemetry_alerts_history.sql:168-190`, but the worker
  claims `fleet_alert_notification_deliveries` by status only at
  `crates/worker/src/alert_notifications.rs:103-145` and performs HTTP at
  `crates/worker/src/alert_notifications.rs:147-250`. The manual alert
  notification processor likewise lists by status and sends HTTP at
  `crates/api/src/fleet_alert_notifications.rs:71-124`. Upsert paths can set
  `enabled = false` without canceling delivery rows at
  `crates/api/src/repository_webhook_rules.rs:131-185` and
  `crates/api/src/repository_alert_notifications.rs:161-228`.
- Notes: This is distinct from AUD-065, which covers frontend/backend
  confirmation binding for manual delivery processing, and from AUD-118, which
  covers manual processors performing HTTP before state update. A clean fix
  should make disabled/deleted integrations quiesce queued and in-flight
  deliveries with visible terminal states such as `canceled_disabled`, or make
  the worker re-check current rule/channel enablement under a delivery lease
  before any HTTP side effect.

### AUD-220: Queued Integration Deliveries Are Not Bound To The Originating Actor Authority

- Severity: High
- Status: Fixed
- Area: API/Worker/Integrations/Auth
- Context: Operators can manually dispatch webhook rules and fleet-alert
  notifications, and the system can queue matching outbound HTTP deliveries for
  asynchronous worker processing. In production, an operator may be disabled,
  deleted, demoted, or have integration/write authority removed while those
  delivery rows are still queued or waiting for retry.
- Root Cause: Integration delivery rows are treated as autonomous once queued.
  The delivery processors select rows by delivery status and send external HTTP
  without checking whether the row's originating actor still exists, is active,
  and still has the authority that allowed queuing or processing. For webhook
  events, the worker also loses actor evidence before delivery materialization:
  manual dispatch records `actor_id` on the event, but the worker event row and
  candidate structs omit `actor_id`, and the worker insert writes delivery
  `actor_id = NULL`.
- Impact: Revoking or reducing an operator's access does not reliably stop
  already queued integration HTTP side effects created by that operator. A
  demoted or disabled account can still cause outbound webhook/alert payloads
  to be sent after access removal, and webhook deliveries generated by the
  worker can lose the audit trail tying the side effect to the operator who
  queued the event. This is practical during incident response or offboarding,
  especially when webhook/alert queues contain delayed retries or many rows
  during a 20+ VPS incident.
- Evidence: Manual webhook dispatch authenticates an operator and records a
  `WebhookEventCandidate` with `actor_id: Some(operator.operator.id)` at
  `crates/api/src/webhook_rules.rs:79-129`. The worker then materializes
  events with an `EventRow` that has no `actor_id` field at
  `crates/worker/src/webhook_rules.rs:122-131`, selects unprocessed events
  without `actor_id` at `crates/worker/src/webhook_rules.rs:408-420`, builds a
  `DeliveryCandidate` with no actor field at
  `crates/worker/src/webhook_rules.rs:95-108` and
  `crates/worker/src/webhook_rules.rs:541-553`, and inserts the delivery with
  `actor_id` set to `NULL` at
  `crates/worker/src/webhook_rules.rs:653-676`. Webhook delivery processing
  claims rows by status only and sends HTTP from loaded row data at
  `crates/worker/src/webhook_rules.rs:694-843`; the manual API processor has
  the same status-only shape at `crates/api/src/webhook_rules.rs:143-190`.
  Fleet-alert notification dispatch stores the current operator on delivery
  rows at `crates/api/src/repository_alert_notifications.rs:391-505` and
  `crates/api/src/repository_alert_notifications.rs:711-735`, but the worker
  claim does not select or validate `actor_id` before sending at
  `crates/worker/src/alert_notifications.rs:103-151` and
  `crates/worker/src/alert_notifications.rs:220-250`; the manual API processor
  likewise lists delivery rows by status and sends them at
  `crates/api/src/fleet_alert_notifications.rs:74-124`.
- Notes: This is distinct from AUD-058, which covers the wrong write scope on
  integration mutation routes; AUD-118, which covers manual delivery processors
  performing HTTP before recording state; and AUD-219, which covers disabling
  an integration not stopping queued delivery rows. A clean fix should preserve
  actor identity through event materialization, and either revalidate active
  actor authority immediately before external delivery or cancel queued rows
  visibly when the originating actor is no longer authorized.
- Fix Notes: Webhook event materialization now carries actor identity from
  event/rule into deliveries, and worker/manual processors revalidate active
  `inventory:write` authority before HTTP. Revoked webhook deliveries are marked
  permanently failed and alert notifications failed with
  `actor_authority_revoked`, without sending the external request.

### AUD-221: System Dashboard Omits Agent-Lost Lifecycle Failures

- Severity: Medium/High
- Status: Confirmed
- Area: API/Frontend/System Dashboard/Job Lifecycle
- Context: `agent_lost` is the terminal target status used when the control
  plane has positive evidence that the executing agent process was lost or
  restarted, or an expected update activation heartbeat never arrived.
  Operators use the System Dashboard as the broad fleet health view during
  incidents and long-running operation.
- Root Cause: The dashboard target snapshot and metric series model still
  expose only `control_timeout_last_24h` and `agent_timeout_last_24h`.
  Repository queries count `status = 'agent_timeout'` but never count
  `status = 'agent_lost'`, and the frontend derives its lifecycle failure
  summary from only those two fields.
- Impact: A detected agent restart/loss can terminalize job targets as
  `agent_lost` and appear in job details, while the System Dashboard reports
  zero such lifecycle failures. In a 20+ VPS fleet this hides restart/lost-agent
  events and update-activation heartbeat losses from the view operators are
  likely to watch during incidents, and makes dashboard health totals disagree
  with job target history.
- Evidence: `crates/api/src/repository_system_dashboard.rs:58-65` and
  `crates/api/src/repository_system_dashboard.rs:112-119` count only
  `control_timeout` and `agent_timeout`;
  `crates/api/src/model_dashboard.rs:262-263` has no
  `agent_lost_last_24h` field; `crates/api/src/routes_system.rs:222-223` maps
  only timeout metric labels; `frontend/src/panels/SystemPanel.tsx:1161-1162`
  and `frontend/src/panels/SystemPanel.tsx:1257-1258` display only control and
  agent timeout values. Meanwhile, `crates/api/src/repository_jobs.rs` records
  target status `agent_lost` in lifecycle paths, and
  `frontend/src/bulkJobProgress.ts:118-119` already treats `agent_lost` as a
  first-class target failure elsewhere.
- Notes: Add an `agent_lost_last_24h` dashboard counter and series metric,
  label it distinctly from `agent_timeout`, and include it in the frontend
  lifecycle failure summary. Keep `agent_lost` distinct from `control_timeout`;
  do not relabel it as a timeout.

### AUD-222: Suite Config Editor Still Presents The Private API Bind As A Public API Setting

- Severity: Medium
- Status: Confirmed
- Area: Frontend/System Config/Security
- Context: The API is a private operator/control-plane service and must not be
  exposed publicly. Operators can edit suite runtime config from the System
  panel, including `api.bind`, so wording in that editor is operational
  guidance, not just decorative copy.
- Root Cause: The structured suite config editor still describes the API group
  as `Public API bind and gateway control settings`, even though the current
  architecture and docs require the API to remain private and separate from any
  public update or dashboard URL.
- Impact: The UI reinforces the wrong mental model while the operator is
  editing the setting that controls the API listener. A direct binary
  deployment, compose adaptation, host-network container, or private/public
  reverse-proxy change can expose the operator API if the operator treats
  `api.bind` as a public API setting. This is practical because System config is
  where operators make persistent runtime changes.
- Evidence: `frontend/src/panels/SystemPanel.tsx:1602-1604` labels the API
  config group as public while exposing `api.bind`; project guidance in
  `TO_AGENTS.md` and `docs/operator-access-scopes.md` says the API and gateway
  are private operator/control-plane services and must not be exposed publicly.
  AUD-066/AUD-067 fixed defaults and proxy exposure, but this stale operator UI
  wording remains.
- Notes: Rename the group description to private-control-plane wording and keep
  public URLs limited to separately supplied external artifact/dashboard access
  paths. Do not reintroduce API-hosted public URLs.

### AUD-223: Lifecycle Disconnect Can Report Success While Older Queued Commands Still Deliver

- Severity: High
- Status: Confirmed
- Area: API/Gateway/Client Lifecycle
- Context: Client delete, key revocation, and key replacement are access
  deactivation workflows. Operators use them when a VPS is retired, rebuilt, or
  suspected compromised, and expect no new command frames to be delivered to
  that live agent process after the deactivation action is accepted.
- Root Cause: The gateway uses one per-session FIFO `mpsc` queue for command
  dispatch, cancel, and disconnect messages. The internal disconnect endpoint
  returns `accepted: true, disconnected: true` as soon as it enqueues a
  `GatewaySessionMessage::Disconnect`; it does not remove the session from the
  in-memory map, close the transport, or wait until the session loop has
  processed that disconnect. Any already queued `Command` messages ahead of the
  disconnect are still written to the agent before the disconnect is handled.
  The API treats `accepted` as sufficient and proceeds with key/delete
  lifecycle mutation.
- Impact: A dispatch that reached the gateway shortly before a client delete,
  key revoke, or key replacement can still be delivered and executed after the
  operator-facing lifecycle action reports success. This is practical in a
  20+ VPS fleet because dispatcher retries, manual jobs, and lifecycle actions
  can overlap under load or incident response. It weakens the operational
  meaning of revoke/delete/replace-key and can leave audit/job history showing
  a deactivated client while the old live process receives one or more final
  commands.
- Evidence: The shared queue is created at
  `crates/gateway/src/main.rs:546-548` with
  `SESSION_COMMAND_QUEUE_CAPACITY = 1024` from
  `crates/gateway/src/state.rs:18-20`. The session loop writes queued commands
  to the agent before inserting them into pending state at
  `crates/gateway/src/main.rs:581-610`, and handles queued disconnect only
  later by breaking the loop at `crates/gateway/src/main.rs:628-630`. The
  control endpoint enqueues disconnect and immediately returns
  `disconnected: true` at `crates/gateway/src/control.rs:360-396`. The API
  lifecycle helper accepts any `result.accepted` at
  `crates/api/src/state.rs:158-180`, and delete/key-revoke/key-replace call it
  before repository mutation at `crates/api/src/routes_inventory.rs:96-104`
  and `crates/api/src/routes_key_lifecycle.rs:39-49`,
  `crates/api/src/routes_key_lifecycle.rs:91-100`.
- Notes: This is distinct from AUD-113/AUD-114, which covered missing live
  disconnects, and from AUD-145, which covers reconnect before DB invalidation.
  A clean fix should make lifecycle disconnect a barrier: mark the session
  non-dispatchable immediately, reject or drain queued command messages without
  sending them, close the transport, and report success only when the active
  in-memory session has actually been removed or proven absent. The API should
  not treat merely enqueued disconnect as proof that old work cannot still be
  delivered.

### AUD-224: File Pull Byte Caps Can Be Bypassed When A File Grows After Stat

- Severity: Medium/High
- Status: Confirmed
- Area: Agent/CLI/Frontend/File Pull
- Context: Operators can run the `file_pull` job from the dashboard, CLI, and
  VTY to retrieve a file from one or many VPSs. This is commonly pointed at
  logs, generated diagnostics, and application files that can continue growing
  while the pull is in progress.
- Root Cause: The agent validates `file_pull` size only from pre-read metadata.
  In the non-streaming path, it checks `metadata.len()` against
  `MAX_FILE_PULL_BYTES`, then calls `tokio::fs::read(path)` and chunks whatever
  bytes were read. In the streaming path, it checks the original metadata size
  against `MAX_STREAMING_FILE_PULL_BYTES`, then keeps reading and forwarding
  chunks until EOF; it reports a size-change error only after all chunks have
  already been sent.
- Impact: A file that is below the cap at stat time but grows during the pull
  can make the agent read more than the intended 1 MiB non-streaming limit or
  forward more than the intended 64 MiB streaming limit before the job fails.
  In a 20+ VPS fleet, pulling active logs or diagnostics can therefore consume
  unexpected agent memory, gateway/API output bandwidth, output storage, and
  job history space even though the command appears to have explicit byte
  limits.
- Evidence: `execute_file_pull` checks `metadata.len()` at
  `crates/agent/src/file_pull.rs:41-56` and then reads the full path at
  `crates/agent/src/file_pull.rs:59-61` without enforcing a post-read cap.
  `execute_streaming_file_pull` checks only the initial `size_bytes` at
  `crates/agent/src/file_pull.rs:81-90`, forwards every read chunk at
  `crates/agent/src/file_pull.rs:100-121`, and detects a size change only at
  `crates/agent/src/file_pull.rs:125-127`. The command is exposed as
  `JobCommand::FilePull` at `crates/common/src/protocol.rs:2383-2385`, via the
  CLI at `crates/vpsctl/src/commands_dispatch_jobs.rs:305-323`, and in the
  dashboard file-pull controls at
  `frontend/src/panels/jobs/JobOperationControls.tsx:351-361`.
- Notes: This is distinct from AUD-014, which fixed capped direct file
  read/download paths but does not cover `file_pull`. The clean fix is to
  enforce the cap during reads and streaming sends, abort before emitting any
  chunk beyond the configured limit, and make the final status reflect a
  capped/changed file without persisting oversized output.
- Resolution: Fixed by moving `file_pull` to bounded read/open helpers. The
  non-streaming path enforces the 1 MiB cap while reading, and the streaming
  path aborts before sending a chunk that would exceed the 64 MiB cap. The
  command now carries explicit `follow_symlinks` through API/CLI/VTY/frontend
  so the reviewed path semantics are part of the job intent.

### AUD-225: Text Save Hash Checks Read The Whole Destination File Into Memory

- Severity: Medium/High
- Status: Fixed
- Area: Agent/File Browser/Resource Bounds
- Context: Operators edit small text files from the single-VPS file browser or
  dispatch text-write jobs from bulk file workflows. The frontend caps the new
  editor payload at 1 MiB, so operators can reasonably expect the agent-side
  save path to stay small even when it verifies that the destination has not
  changed since review.
- Root Cause: The agent's `file_write_text` implementation uses
  `tokio::fs::read(destination)` to hash the current destination file for both
  `policy = ensure` on create-existing paths and for
  `expected_sha256_hex` stale-write checks. There is no size cap or streaming
  hash before reading the destination into a single `Vec<u8>`.
- Impact: A small text save can allocate and hash the entire current
  destination file if the file has grown or been replaced between browser read
  and save. This is practical for service logs, generated configs, rotated
  files, or operator-selected paths that become large during normal operation.
  Across many VPSs, a bulk text write with expected hashes can create avoidable
  memory pressure on agents and fail unrelated work on small machines.
- Evidence: The frontend builder rejects new text payloads above 1 MiB at
  `frontend/src/fileBrowser.ts:162-166`, then includes
  `expected_sha256_hex` in the operation at `frontend/src/fileBrowser.ts:168-174`.
  The protocol carries `expected_sha256_hex` in `JobCommand::FileWriteText` at
  `crates/common/src/protocol.rs:2483-2491`. The agent reads the existing
  destination fully for create/ensure idempotency at
  `crates/agent/src/file_browser.rs:423-439` and again for expected-hash
  validation at `crates/agent/src/file_browser.rs:461-466`.
- Notes: This is distinct from AUD-089, which covers staging-file
  permissions, and from AUD-014, which covers read/download byte caps. The
  clean fix is to stream-hash the current destination under a clear maximum,
  reject files above that maximum before allocating their full content, and
  keep unchanged/idempotent checks bounded.
- Resolution: Fixed by replacing whole-file destination reads in
  `file_write_text` with a no-follow streaming hash bounded by the existing 1
  MiB text-read limit. Strict policies fail closed when verification would
  exceed the bound, while ignore policy returns a skipped
  `verification_failed` status without modifying the file. Regression tests
  cover oversized current files for both policies.

### AUD-226: Final Output Insertion Is Not Atomic With Target Terminalization

- Severity: High
- Status: Fixed
- Area: API/Job Outputs/State Machine
- Context: Command-output ingest is the authoritative path where an agent's
  final output turns a job target into `completed`, `failed`, or `canceled`.
  Operators expect the final output event and the target terminal state to
  explain each other, especially near control deadlines or during manual
  cancellation.
- Root Cause: The ingest route writes the output chunk in one repository call
  and terminalizes the target in a later repository call. The output write does
  lock and verify the target is active before inserting, but that transaction
  commits before `update_job_target_result` starts. Timeout expiry, agent-lost
  reconciliation, or cancellation can acquire the target row in that gap and
  terminalize it first. The later final-output target update then loses its
  compare-and-set and leaves the accepted final output in normal output
  history without making it the terminal state.
- Impact: A final successful or failed agent output can be durably stored while
  the target ends as `control_timeout`, `agent_lost`, or `canceled`. In
  production this is most likely under load, slow API/storage paths, timeout
  sweeps, gateway retry bursts, or an operator canceling a job just as the
  agent finishes. It creates a forensic inconsistency: job-output downloads,
  comparisons, backup/file-transfer derivation, and audit review can show a
  final agent result that did not actually determine the target status.
- Evidence: `ingest_command_output` first calls
  `record_active_job_output_chunk_checked_with_config` at
  `crates/api/src/routes_ingest.rs:180-191`, publishes the output event, and
  only then calls `update_job_target_result` for `done` output at
  `crates/api/src/routes_ingest.rs:211-216`. The output writer enters its own
  transaction at `crates/api/src/repository_job_outputs.rs:679-688`, locks the
  target row with `FOR UPDATE` in
  `crates/api/src/repository_job_outputs.rs:988-1005`, inserts into
  `job_outputs` at `crates/api/src/repository_job_outputs.rs:747-770`, and
  commits before returning to the route. Timeout expiry independently locks
  active targets and writes synthetic timeout/agent-lost output at
  `crates/api/src/repository_jobs.rs:2404-2480`, then terminalizes the target
  with an active-state CAS at `crates/api/src/repository_jobs.rs:2481-2516`.
  Final-output terminalization uses a separate active-state CAS at
  `crates/api/src/repository_jobs.rs:2774-2920`.
- Notes: This is distinct from AUD-108, which covers parent job aggregate
  completion after target terminalization, and from AUD-122, which covers new
  output arriving after the target is already terminal. The clean fix should
  make final-output insert plus target terminalization one transaction under
  the same target/output-stream lock, or make competing terminalizers detect
  and honor an already-persisted final output before writing timeout/cancel
  evidence.
- Resolution: Fixed by routing `done` command-output ingest through a single
  repository operation that records the final output and terminalizes the
  active target under the same output/target lock and Postgres transaction.
  Parent job completion, webhook side effects, terminal/file-transfer refresh,
  and backup artifact auto-recording now run only after that atomic operation
  commits. Focused regression tests cover final-output terminalization and
  late-output rejection behavior.

### AUD-227: Directory Listing Reads And Sorts Every Entry Before Applying The Page Limit

- Severity: Medium/High
- Status: Fixed
- Area: Agent/Frontend/File Browser/Resource Bounds
- Context: Operators use the single-VPS file browser to inspect directories on
  managed VPSs. Real production paths such as log directories, spool
  directories, package caches, container layers, and application upload trees
  can contain many thousands or millions of entries.
- Root Cause: The `file_list_dir` operation has an operator-visible `limit`,
  but the agent implementation reads every directory entry into a `Vec`, stats
  each entry, sorts the whole vector, and only then slices the requested page.
  The limit bounds only the returned JSON, not the amount of agent filesystem
  walking, memory, stat calls, or sort work.
- Impact: Browsing one very large directory can consume substantial agent CPU
  and memory, hold a command slot until timeout, and produce slow or failed
  file-browser jobs. In a 20+ VPS fleet this can make a routine operator
  inspection of common high-cardinality directories look like a hung agent or
  compete with more important jobs on small VPSs, despite the UI showing a
  bounded page size.
- Evidence: The frontend always dispatches a page-limited list request with
  `offset: 0` and `limit: FILE_BROWSER_LIST_LIMIT` at
  `frontend/src/panels/jobs/FileBrowserPanel.tsx:229-234`. The protocol
  exposes `offset` and `limit` for `JobCommand::FileListDir` at
  `crates/common/src/protocol.rs:2467-2475`, and API validation caps the
  requested limit at `crates/api/src/job_files.rs:295-300`. The agent clamps
  the limit at `crates/agent/src/file_browser.rs:253-263`, but then pushes
  every entry into `entries` at `crates/agent/src/file_browser.rs:269-292`,
  sorts all entries at `crates/agent/src/file_browser.rs:293-299`, and only
  applies `offset`/`limit` at `crates/agent/src/file_browser.rs:300-305`.
- Notes: This is distinct from job-output pagination issues such as AUD-200.
  The clean fix should stream or incrementally maintain only the requested
  window plus enough ordering state for deterministic pagination, or introduce
  a separate hard scan cap with an explicit `truncated_by_scan_cap` status so
  operators know the directory is too large for the browser path.
- Resolution: Fixed with a hard 10,000-entry scan cap for file-browser
  directory listings. Capped responses return the requested page from scanned
  entries, set `total_entries` to null, and include `scanned_entries`,
  `visible_entries_scanned`, `scan_cap_entries`, and
  `truncated_by_scan_cap` so the frontend can present the bounded result
  honestly. Regression tests cover the capped status shape.

### AUD-228: Network Speed-Test Server Accepts The First TCP Peer Without Verifying The Expected Tunnel Peer

- Severity: Medium/High
- Status: Fixed
- Area: Agent/API/Network Speed Tests
- Context: Operators can run topology speed tests from the dashboard, CLI, or
  VTY. The frontend and API treat `network_speed_test` as a two-endpoint tunnel
  operation: both tunnel endpoint clients are targeted, and the resulting
  `network_speed_test` observations can be persisted and used as topology
  evidence for OSPF cost recommendations.
- Root Cause: The agent server-side speed-test role binds the tunnel address
  and accepts the first incoming TCP connection. It records `peer_socket`, bytes,
  and success based on that accepted stream, but it never verifies that the
  remote peer address is the expected tunnel peer address from the selected
  `TunnelPlan`. The client-side role dials the server tunnel address, but the
  server-side role does not enforce symmetry.
- Impact: A wrong process or wrong VPS that can reach the speed-test listener
  can connect first and produce successful or degraded speed-test evidence for
  the selected tunnel pair. This is practical in a 20+ VPS deployment when
  operators run overlapping tests on shared ports, when a previous/manual test
  is still connecting, or when another host on the management/tunnel network can
  reach the listener. Bad throughput evidence can mislead operators during
  incident response and can feed persisted network observation trends that are
  later used for OSPF cost recommendations.
- Evidence: The dashboard speed-test action targets both endpoint client IDs at
  `frontend/src/panels/topology/TopologyApplyControls.tsx:159-164`, and API
  tests enforce that speed-test jobs include both tunnel endpoints at
  `crates/api/src/tests_network.rs:1425-1454`. On the agent, the server role
  accepts exactly one connection and records success without checking the
  remote IP against the expected peer at
  `crates/agent/src/network_speed.rs:148-185`. The persisted observation parser
  accepts `network_speed_test` status output and marks health from `success` at
  `crates/api/src/repository_network_observations.rs:428-475`. OSPF
  recommendation code consumes persisted speed-test trends at
  `crates/api/src/repository_network_recommendations.rs:62-83`.
- Notes: This is distinct from stale topology confirmations and network
  read-scope issues. A clean fix should make the server role reject or ignore
  connections whose remote IP does not match the expected peer tunnel address,
  include an explicit `peer_mismatch` result when that happens, and preferably
  include a per-test nonce in the stream so simultaneous tests on the same port
  cannot cross-contaminate evidence.
- Resolution: Fixed by binding the client stream to the peer tunnel address,
  validating the server-side remote IP, and requiring a per-job command-hash
  nonce handshake before throughput bytes are counted.

### AUD-229: Topology Evidence And OSPF Recommendations Are Keyed By Mutable Tunnel-Plan Names

- Severity: High
- Status: Confirmed
- Area: API/Frontend/Network Topology
- Context: Operators save tunnel plans, run network status/probe/speed-test
  jobs, and then use the topology graph, evidence table, OSPF recommendation
  list, and OSPF update-plan flow to decide whether a tunnel is healthy or
  whether Bird2 costs should change.
- Root Cause: Persisted network observations are keyed to `plan_name`,
  `interface_name`, client ID, and optional peer client ID, but not to the
  immutable `tunnel_plans.id` or an exact endpoint/topology identity. Tunnel
  plans can be overwritten in place by active name, and soft-deleted names can
  later be reused. The topology graph summarizes observations by plan name
  only, while OSPF recommendations accept trends for the same plan name when
  any current endpoint overlaps the trend's client or peer.
- Impact: Old measurements from a previous tunnel identity can appear as
  current evidence for a newer tunnel that reused the same name or changed one
  endpoint. For example, replacing `prod-edge` from A-B to A-C can leave A-B
  latency, throughput, and runtime-status rows counted against A-C because the
  name and one endpoint still match. Operators can then see incorrect topology
  health, stale sparklines/sample counts, and reviewed OSPF update plans derived
  from the wrong tunnel evidence. In a 20+ VPS fleet, reusing stable names while
  replacing peers or rebuilding regions is normal, so this is a practical
  routing and incident-response risk rather than a cosmetic display bug.
- Evidence: The canonical schema stores `network_observations.plan_name` but
  no `plan_id` or endpoint identity at `migrations/0005_network_tunnels.sql:57-77`.
  Active tunnel-plan saves update an existing non-deleted row by name and reset
  endpoint/status fields at `crates/api/src/repository_network.rs:148-181`;
  deleted plan names are reusable because the unique index only covers
  non-deleted names at `migrations/0005_network_tunnels.sql:43-45`.
  Observation ingestion extracts the mutable `plan` and `interface` strings
  from output metadata at
  `crates/api/src/repository_network_observations.rs:428-475`. The topology
  graph calls `summarize_edge_trends(&plan.name, ...)` and
  `summarize_edge_observations(&plan.name, ...)` at
  `crates/api/src/repository_topology_graph.rs:39-49`, and those helpers filter
  by plan name only at
  `crates/api/src/repository_topology_graph.rs:288-365` and
  `crates/api/src/repository_topology_graph.rs:387-389`. OSPF recommendations
  consume all recent trends at
  `crates/api/src/repository_network_recommendations.rs:15-24`, match by
  plan name plus any endpoint overlap at
  `crates/api/src/repository_network_recommendations.rs:241-255`, and then
  build OSPF update plans from that recommendation at
  `crates/api/src/repository_network_recommendations.rs:158-217`.
- Notes: This is distinct from frontend stale-confirmation bugs and from the
  fixed backend confirmation contract for tunnel-plan saves. A clean fix should
  bind observations to the immutable plan ID or to a stored topology identity
  hash containing plan ID/name, interface, left/right client IDs, tunnel
  addresses, and endpoint side. Recommendations and graph summaries should
  require an exact current identity match and should either ignore old evidence
  or label it as historical after a plan is overwritten or recreated.

### AUD-230: Autonomous Latency Monitoring Captures Custom Probe Output Without A Byte Limit

- Severity: Medium/High
- Status: Confirmed
- Area: Agent/Telemetry/Network Probes
- Context: Agents can run runtime tunnel telemetry and latency monitoring for
  managed tunnel plans. The default config enables latency monitoring, and
  operators can set `[network].probe_ping_argv` for custom wrappers,
  nonstandard ping paths, or provider-specific probe behavior.
- Root Cause: The autonomous telemetry latency probe uses
  `tokio::process::Command::output()` under a 10-second timeout. That API
  collects complete stdout and stderr into memory. There is no
  `runtime_command_max_output_bytes`, ping-output, or telemetry-output cap on
  this path. The on-demand `network_probe` job uses the bounded child-process
  helper, so the recurring telemetry path is the outlier.
- Impact: A bad or noisy custom probe command can allocate unbounded agent
  memory for up to the timeout on every telemetry latency check. This is
  practical in production because custom probe wrappers are an advertised
  extension point and latency monitoring is recurring. On small VPSs, or across
  many tunnel plans, one accidentally chatty wrapper such as a script that logs
  debug output or ignores the appended ping arguments can cause agent memory
  pressure, delayed telemetry, missed gateway work, or process restarts.
- Evidence: `run_latency_probe` builds the configured or preset ping argv,
  pipes stdout and stderr, and then awaits `command.output()` inside
  `time::timeout(Duration::from_secs(10), ...)` at
  `crates/agent/src/telemetry.rs:483-514`. The recurring telemetry monitor
  calls this helper for the primary and possible fallback target at
  `crates/agent/src/telemetry.rs:650-654`. Agent network defaults enable
  latency monitoring at `crates/common/src/config/models.rs:397-402`, and the
  operator-facing config documents `probe_ping_argv` as a custom wrapper path
  at `docs/agent-config.example.toml:96-100`. The on-demand probe path shows
  the intended bounded model by calling
  `run_child_with_bounded_output_cancelable(..., MAX_PING_OUTPUT_BYTES, ...)`
  at `crates/agent/src/network_probe.rs:71-79`.
- Notes: This is distinct from AUD-163, which covers custom JSON telemetry and
  traffic commands that can outlive timeout handling after stdout closes. This
  issue is specifically about recurring latency probes using an unbounded
  output-collection API. A clean fix should route autonomous latency probes
  through the same bounded child-process helper used by on-demand network
  probes, with a small output cap and process-group cleanup.

### AUD-231: Network Speed Tests Are Treated As Confirmation-Free Read-Only Jobs Despite Opening Listeners And Sending Traffic

- Severity: Medium/High
- Status: Fixed
- Area: API/CLI/Agent/Network Speed Tests
- Context: Operators can run topology speed tests from the frontend, CLI, or
  VTY to measure tunnel throughput. The operation targets both tunnel endpoint
  VPSs, opens a TCP listener on one endpoint, and sends traffic from the other
  endpoint for the requested duration and byte budget.
- Root Cause: The shared command contract classifies `network_speed_test` as
  `read_only`. The backend derives `job_command_requires_confirmation()` from
  whether the command is `exclusive`, so speed-test jobs do not require the
  `confirmed` flag. The CLI network speed-test command submits
  `confirmed: false` and `destructive: false` while still building a privilege
  assertion and dispatching the operation.
- Impact: Direct API and CLI callers can start a production tunnel bandwidth
  test without the backend confirmation guard used for other high-impact
  network work. This is practical because the configured limits allow up to
  30 seconds, 256 MiB, and 1,000,000 Kbps per speed-test job. An accidental
  command, stale automation payload, or mistaken target pair can consume tunnel
  capacity, open a service port on the server-side VPS, and record misleading
  speed-test evidence without the same explicit review semantics that the
  frontend already presents with its "Review speed test" flow.
- Evidence: `JOB_COMMAND_SAFETY_BY_OPERATION_TYPE` maps
  `network_speed_test` to `read_only` at
  `crates/common/src/protocol.rs:1295-1318`. Confirmation is derived from
  `job_command_safety(command) == JobCommandSafety::Exclusive` at
  `crates/common/src/protocol.rs:2926-2928`, and the generated frontend
  contract exposes `network_speed_test: false` for confirmation requirement at
  `frontend/src/generated/protocolContracts.ts:226-235`. The CLI submits
  network speed tests with `confirmed = false` and `destructive = false` at
  `crates/vpsctl/src/commands_network.rs:706-733`. The agent enforces speed
  budgets up to `NETWORK_SPEED_TEST_MAX_MAX_BYTES` and
  `NETWORK_SPEED_TEST_MAX_RATE_LIMIT_KBPS` at
  `crates/common/src/protocol.rs:11-16` and
  `crates/agent/src/network_speed.rs:105-120`, then binds a TCP listener and
  accepts a peer at `crates/agent/src/network_speed.rs:148-157` while the
  client side writes traffic in the loop at
  `crates/agent/src/network_speed.rs:188-208`.
- Notes: This is distinct from AUD-228. AUD-228 covers accepting the wrong peer
  on the server-side listener. This issue covers the API/CLI confirmation and
  safety classification for the legitimate speed-test operation. A clean fix
  should either classify speed tests as confirmation-required network operations
  or add a dedicated `network_io`/`requires_confirmation` flag so read-only
  topology inspections remain lightweight while traffic-generating tests still
  require explicit operator review.
- Resolution: Fixed by classifying `network_speed_test` as `exclusive`, making
  API confirmation mandatory, and updating frontend, CLI, VTY, and tutorial
  flows to send or require explicit confirmation.

### AUD-232: Network Speed Tests Bypass Exclusive Dispatch Serialization And Can Overlap On The Same Tunnel Endpoints

- Severity: Medium/High
- Status: Fixed
- Area: API/Dispatcher/Agent/Network Speed Tests
- Context: Operators can start tunnel speed tests from the topology panel, CLI,
  or VTY. Each test targets both endpoint VPSs, opens a TCP listener on the
  selected server side, and sends traffic from the peer side. The default port
  used by the frontend and CLI is 5201, with configurable duration, byte, and
  rate budgets.
- Root Cause: Dispatch serialization is driven by the shared
  `exclusive_operation_types()` list. That list is generated only from operation
  types classified as `exclusive`; `network_speed_test` is classified as
  `read_only`, so the dispatcher does not acquire the per-client exclusive
  advisory lock or check for active exclusive work before claiming speed-test
  targets. The same classification also means active speed tests do not block
  later exclusive network mutations.
- Impact: Two normal operator actions can run speed tests against the same
  tunnel endpoints at the same time. With the default port, the second
  server-side target can fail to bind; with different ports, both tests can send
  traffic concurrently and corrupt the measured throughput. Because active speed
  tests are also invisible to the exclusive-job exclusion, a tunnel apply,
  rollback, or OSPF cost update can be dispatched while a speed test is
  measuring the same tunnel. The resulting failures or misleading measurements
  are practical in a 20+ VPS fleet where operators may retry jobs, run checks
  from both CLI and dashboard, or run scheduled/manual network work close
  together.
- Evidence: `exclusive_operation_types()` is built from
  `job_command_safety_by_operation_type()` at
  `crates/api/src/repository_jobs.rs:488-495`. The Postgres dispatcher only
  applies the per-client advisory lock and active-job exclusion when
  `job.operation ->> 'type'` is in that exclusive list at
  `crates/api/src/repository_jobs.rs:1603-1671`, then binds that list into the
  claim query at `crates/api/src/repository_jobs.rs:1741-1745`.
  `network_speed_test` is mapped to `read_only` at
  `crates/common/src/protocol.rs:1295-1318`, and the runtime safety function
  also returns `JobCommandSafety::ReadOnly` for it at
  `crates/common/src/protocol.rs:2887`. The frontend and CLI default the speed
  test port to 5201 at
  `frontend/src/panels/topology/TopologyApplyControls.tsx:66-70` and
  `crates/vpsctl/src/commands_network.rs:437-444`. The agent binds the
  listener at `crates/agent/src/network_speed.rs:148-157` and sends traffic at
  `crates/agent/src/network_speed.rs:188-208`.
- Notes: This is distinct from AUD-231. AUD-231 covers missing explicit
  confirmation for a traffic-generating operation. AUD-232 covers dispatch
  overlap and resource isolation. A clean fix should either classify
  `network_speed_test` as exclusive or introduce a dedicated network-I/O
  serialization class that blocks overlapping tests and conflicting tunnel
  mutations without unnecessarily serializing lightweight status/probe reads.
- Resolution: Fixed by moving `network_speed_test` into the shared
  `exclusive` safety class, so existing durable dispatcher and agent runtime
  exclusivity blocks overlapping speed tests and conflicting exclusive work.

### AUD-233: Network Speed Tests Can Dispatch One Endpoint After The Peer Target Is Skipped

- Severity: Medium/High
- Status: Fixed
- Area: API/Worker/Agent/Network Speed Tests
- Context: Network speed tests are a paired operation: both tunnel endpoint VPSs
  must participate in the same job, one as the TCP listener and the other as
  the traffic sender. Normal production schedules and manual jobs can include a
  fixed target snapshot where one endpoint later becomes never-connected,
  hidden, deleted, revoked, or otherwise unavailable.
- Root Cause: Manual job creation and schedule materialization validate
  `network_speed_test` against the original resolved/fixed target list before
  target availability and precompletion logic removes unclaimable targets from
  the dispatch set. The unavailable endpoint is converted to a skipped target,
  but the other endpoint remains queued and can still be dispatched with the
  paired speed-test command.
- Impact: A scheduled or manual speed test can become a one-sided run. If the
  surviving endpoint is the server side, it opens the speed-test TCP listener
  and waits for a peer that will never be dispatched. If it is the client side,
  it retries connection to a listener that is not running. This consumes the
  surviving VPS's job slot for the command timeout, can leave a production
  listener briefly exposed on the tunnel address, and records a failed or
  partial speed-test result for a test that was never actually runnable as a
  pair. In a 20+ VPS fleet, this is practical when long-lived schedules survive
  endpoint replacement, key revocation, or first-agent provisioning gaps.
- Evidence: Manual job creation calls `validate_network_apply_target(&job_command,
  &resolved_targets)` before computing `claimable_targets`, capability skips,
  and never-connected precompletion in `crates/api/src/routes_jobs.rs`. Schedule
  materialization calls the same validation against the saved `targets` before
  `load_schedule_target_capabilities`, `available_schedule_targets`, and
  skipped-target construction in `crates/worker/src/main.rs`. The shared speed
  test validator only checks that the original target set equals the plan's
  left/right client IDs in `crates/server-core/src/lib.rs`. The agent then runs
  one role independently: the server side binds and waits in
  `crates/agent/src/network_speed.rs::receive_speed_test`, while the client
  side retries connection in `send_speed_test`.
- Notes: This is distinct from AUD-231 and AUD-232. Those cover missing
  confirmation and missing serialization for valid two-sided speed tests. This
  issue covers a paired operation remaining dispatchable after one required
  participant has already been converted into a skipped target. A clean fix
  should treat `network_speed_test` as all-or-nothing after availability
  filtering: if either endpoint is skipped or unclaimable, both target rows
  should become terminal with a clear peer-unavailable reason and no endpoint
  should receive the command.
- Resolution: Fixed by applying an all-or-none speed-test dispatch filter in
  manual job creation and schedule materialization. If only one endpoint
  remains dispatchable, that endpoint is precompleted as
  `network_speed_test_peer_unavailable` and no speed-test command is sent.

### AUD-234: Job-Created Webhooks Deliver Full Job Operation Payloads To External Targets

- Severity: High
- Status: Skipped
- Area: API/Worker/Webhooks/Security
- Context: Operators can configure expression webhook rules for job events such
  as `job.created`, `job.type:<type>`, or `job.status:<status>`. Those
  webhooks are normal production notification paths to external systems such as
  chat, incident tooling, ticketing, or automation endpoints.
- Root Cause: The API records the complete `JobCommand` under
  `job.operation` in the `job.created` webhook event payload. The worker then
  merges event roots into the delivery payload and posts the whole JSON payload
  to the configured webhook URL, regardless of whether the rule's rendered
  message template needs the operation.
- Impact: A normal job-created webhook can transmit full command payloads to an
  external integration endpoint. Practical sensitive fields include shell
  scripts and argv, process supervisor argv/cwd/environment values, file paths,
  backup and restore path/hash metadata, network plan details, and update
  artifact URLs or hashes. This is especially risky because operators may
  reasonably configure a simple "job created" notification and expect only
  summary metadata plus the rendered message to leave the private control
  plane. It can also persist the same operation payload in webhook delivery
  rows until retention cleanup.
- Evidence: `record_job_created_webhook_event` serializes
  `event.operation` into `payload.job.operation` at
  `crates/api/src/repository_jobs.rs:3482-3515`. Worker materialization merges
  event payload roots into the delivery payload at
  `crates/worker/src/webhook_rules.rs:511-553`, and delivery posts
  `delivery.payload` directly at `crates/worker/src/webhook_rules.rs:829-835`.
  The shared command model includes secret-prone process environment values in
  `JobCommand::ProcessStart.env` and still includes operationally sensitive
  command fields such as shell argv, file paths, staged restore archive paths,
  network intents, and update artifact URLs in `crates/common/src/protocol.rs`.
- Notes: Fixed AUD-171 removed plaintext restore archive bytes from the job
  command model, but full operation payload exposure remains broader than that
  one field. A clean fix should define a redacted webhook job summary and make
  payload-bearing fields opt-in only where an integration workflow explicitly
  requires them.
- Skip Rationale: Product decision: expression webhook templates and job
  commands are operator-managed surfaces, and this project intentionally allows
  operator-controlled webhook payloads to contain reviewed command context. The
  added redaction/product-policy complexity is not aligned with the current
  pre-release enterprise console design.

### AUD-235: Job-Create Retries Can Dispatch The Same Reviewed Action Under A New Job ID

- Severity: High
- Status: Fixed
- Area: API/Frontend/Jobs/Idempotency
- Context: Operators submit privileged and destructive jobs from the dashboard,
  CLI, VTY, or direct private API automation. Network ambiguity is normal in
  production: a request can reach the API and commit a job while the response is
  lost, or a post-commit side effect can fail before the API returns success.
- Root Cause: The backend idempotency model is keyed by `job_id`, but
  `CreateJobRequest.job_id` is optional and the route generates a new UUID when
  it is missing. The frontend shared client also generates a fallback UUID at
  request-send time, and the main dispatch panel generates a new UUID inside
  the submit handler instead of storing it in the frozen confirmation snapshot.
  Therefore a manual retry after an ambiguous submit can use a different job ID
  for the same reviewed operation, bypassing the request-fingerprint reuse
  check.
- Impact: A real operator can accidentally dispatch the same reviewed action
  twice after a timeout, browser/API disconnect, transient database error after
  job commit, or webhook-event failure after job creation. This applies to
  high-impact jobs such as file mutation, config apply, restore, update,
  process supervisor, and network changes. At 20+ VPS scale, duplicate retries
  can mutate the same targets twice while the UI or automation believed the
  first attempt failed before creation.
- Evidence: `CreateJobRequest.job_id` is optional at
  `crates/api/src/model.rs:878-882`, and the create-job route falls back to
  `Uuid::new_v4` at `crates/api/src/routes_jobs.rs:190-193`. The reuse guard
  only checks an existing row for the same `job_id` at
  `crates/api/src/routes_jobs.rs:493-519`. The repository commits the job and
  target rows, then records the job-created webhook event afterward at
  `crates/api/src/repository_jobs.rs:1398-1472`, so a post-commit failure can
  make a durable job look like a failed submit. The frontend client injects a
  fallback UUID during each API call at `frontend/src/hooks/useJobsData.ts:474-482`,
  and the main dispatch panel generates another new UUID inside submission at
  `frontend/src/panels/JobDispatchPanel.tsx:1028-1038` rather than freezing it
  in the review snapshot. Multi-file review snapshots similarly store only the
  operation, selector, and targets at
  `frontend/src/panels/jobs/MultiFileActionsPanel.tsx:158-160`; the eventual
  submit delegates to `onCreateJob` without a frozen `job_id` at
  `frontend/src/panels/jobs/MultiFileActionsPanel.tsx:254-268`.
- Notes: The CLI generally sends a generated UUID before posting, for example
  `crates/vpsctl/src/jobs.rs:89-98`, but the API still accepts missing IDs and
  dashboard retry semantics are not anchored to the reviewed snapshot. A clean
  fix should require a client-supplied job ID for job creation and include that
  ID in every frontend confirmation snapshot before the first submit.
- Resolution: Fixed by requiring a client-supplied `job_id` at the start of
  job creation, returning `job_id_required` when it is missing, and returning
  the existing job response when the same actor retries the same frozen request
  fingerprint. Reusing a job ID for a different actor or different request now
  conflicts. The shared frontend client no longer injects fallback IDs at send
  time; reviewed confirmations freeze the UUID in their snapshots, and direct
  actions generate one UUID when the action starts.

## Issue Template

Copy this section for each new issue. One issue means one production-impact
defect, not a bundle of related concerns.

### AUD-###: Short Production-Impact Title

- Severity: Critical | High | Medium/High | Medium | Low
- Status: Open | Confirmed | Fixed | Won't Fix
- Area: Component, boundary, or workflow affected.
- Context: Normal production workflow where the issue appears.
- Root Cause: Specific implementation or design cause.
- Impact: Concrete production consequence for operators, data integrity,
  security, safety, reliability, or 20+ VPS operation.
- Evidence: File/function/line references or a concise reproducible condition.
- Notes: Optional constraints, affected scope, or why this is practical.
