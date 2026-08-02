# Development and Update Maintenance

This is the durable checklist for starting, validating, and concluding a
vpsman update. Read it before changing product behavior, deployment files,
database migrations, protocols, release paths, or operator workflows.

## 1. Establish the Current State

Start from the repository root and inspect before editing:

```sh
git status -sb
git fetch origin --prune --tags
git log --oneline --decorate -12
vpsman_repository=OWNER/vpsman
gh release view --repo "$vpsman_repository"
```

- Understand every existing worktree change; preserve unrelated user work.
- Read the latest release notes and recent commits that affect the intended
  subsystem.
- Read the relevant operating-model documentation in `README.md`, plus
  `docs/build.md` and `docs/migration-compatibility.md` when applicable.
- State the concrete product defect or maintenance outcome. Do not broaden a
  correction into new functionality, automation, policy, or dependencies.

## 2. Preserve Product Design

Every update must keep these boundaries explicit:

- Reviewed targets are frozen before privileged, broad, destructive, or
  difficult-to-reverse work is dispatched.
- Desired state, queued work, applied evidence, and current observation are
  different facts. Never silently substitute one for another.
- Exact identity operations must not depend on a bounded list result.
- Missing, stale, unsupported, ambiguous, capped, and failed states remain
  visible. Do not invent success or add a silent fallback.
- Durable mutation and its audit evidence stay atomic where the storage model
  supports transactions.
- Audit producers record explicit outcomes, origins, and canonical operator,
  session, job, and VPS identifiers at write time. Presentation code must not
  infer those facts from unrelated metadata, and evidence links require an
  exact ID-backed destination. Fresh schemas reject audit metadata unless
  `result`, `origin_kind`, and `component` are explicit non-empty strings;
  update every memory and PostgreSQL producer together.
- Daily operator paths remain concise. A correctness fix must not add routine
  prompts, concepts, or controls that operators do not need.
- Keep provider integrations behind explicit product-owned adapter definitions
  or configuration presets. Do not add
  third-party services, bots, scanners, dependencies, CI jobs, or release
  artifacts unless the product change specifically requires them and they have
  been reviewed.

## 3. Change the Smallest Complete Surface

- Reproduce the defect or invariant failure before changing source.
- Trace all producers and consumers of a changed contract. Update code, tests,
  generated contracts, and user-facing documentation together.
- Prefer deleting obsolete coupling over layering another compatibility shim.
- Keep compatibility only where an already-published interface requires it.
- Do not hard-code a release snapshot into normal development paths.
- Do not edit `AGENTS.md`.

### Database migrations

- The current migrations are a canonical fresh-database model, not an in-place
  upgrade path from an earlier schema.
- Use the canonical `visible_clients` view for live operator workflows and
  assignments. Use the base `clients` table only when identity lifecycle or
  historical evidence must include tombstoned VPSs; do not hide deleted VPSs by
  deleting their related records. VPS deletion is an irreversible hidden
  tombstone and its ID cannot be reused. Key revocation keeps the VPS visible,
  blocks the old key permanently, and permits recovery of the same ID only by
  explicitly assigning a different key.
- Suggested VPS IDs are derived from the greatest persisted numeric `N` in
  either `N` or `v-N` form, including tombstones, and are presented as `v-`
  followed by `N + 1` (for example, `v-223`). An abandoned registration consumes
  nothing; concurrent use of the same suggestion must fail as an ordinary
  identity conflict. Do not add a separate reservation registry or recycle a
  deleted ID.
- Change a canonical migration only when the product decision explicitly
  accepts a clean break. Update every code, test, and documentation consumer in
  the same change.
- Never edit `_sqlx_migrations`, replace checksums in an existing database, or
  mark an unapplied migration as applied.
- When a deployed schema becomes a compatibility boundary, document and pin it
  from that release onward; later compatible changes must be append-only.
- Add the compatibility note and run `bash scripts/audit-migrations.sh`.

### Adding an audit event

- Write the canonical non-empty `result`, `origin_kind`, and `component` fields
  at the producer. Do not derive an outcome or origin in repository reads or UI
  presentation. `origin_kind` is one of `operator_request`, `authentication`,
  `control_plane`, `gateway_ingest`, `worker`, or `public_share`; `component` is
  the stable name of the writer.
- Attribute operator work with `operator_id`, `operator_username`,
  `operator_role`, and `operator_session_id`. Link affected resources with their
  exact canonical keys, including `job_id`, `client_id`, `target_client_ids`,
  `terminal_session_id`, `gateway_session_id`, or `schedule_id` when that
  resource exists.
- Keep the memory and PostgreSQL writers equivalent, and add tests for both the
  required metadata and each exact-ID correlation or evidence destination.
- Reject missing or malformed canonical fields. Do not add aliases, legacy-key
  reads, substring matching, or presentation-time identity guesses.

### Protocol and rolling-component changes

- Account for mixed server, gateway, worker, CLI, frontend, and agent versions.
- Preserve explicit schema/version rejection; do not parse an unknown shape as
  an older one.
- Regenerate and verify frontend protocol contracts when shared types change.

### Configuration preset changes

- Keep configuration presets limited to supported agent behaviors. Workflow
  status, backup storage, update artifacts, and tunnel adapters are separate
  product objects, not synthetic presets.
- System presets are immutable. A VPS with no explicit override inherits the
  system default; do not materialize a fake assignment row or timestamp.
- Selector/tag targeting is resolved for each preview/apply operation and is
  not a live assignment rule. Sign the exact backend-preview target IDs, and
  reject apply when the preview hash no longer matches. Headless confirmation
  must consume the hash shown in the separately human-reviewed preview; an
  internal replacement preview must never become implicit authorization.
- Preserve desired selection, effective composed config, runtime apply state,
  and readiness evidence as separate facts.
- Runtime-tunnel adapter definitions and optional per-plan OSPF command
  overrides belong to Tunnel plans. A referenced definition is replaced
  through a reviewed plan change, not mutated behind the plan.
- `ospf_update_command` presets provide the reusable per-VPS OSPF updater.
  Resolve each endpoint from its explicit plan override first, otherwise from
  that endpoint VPS's effective preset. An explicit missing or invalid override
  is an error; never fall back around it. If neither source is configured, keep
  the endpoint visibly unconfigured and reject dispatch.

### Monitoring changes

- Preserve one model across resource, network, traffic, and general Ping data:
  accepted high-resolution samples for realtime/short queries, and
  minute-derived authoritative long-term history. Defaults are 90 and 3,650
  days respectively. Do not introduce an hourly/day authority or a parallel
  retention model.
- Materialize the authoritative minute before its accepted sample can be
  pruned. Resource, network, and Ping storage may merge settled adjacent logical
  minutes only when their complete retained values are exactly equivalent.
  Keep spans minute-aligned and query them with correct duration/sample
  weighting. Traffic counters remain minute-derived and keep selector-aware
  per-stream baselines. Preserve counter-epoch changes and counter decreases as
  reset evidence: reset-only buckets retain nullable volumes and chart gaps;
  mixed buckets retain valid selected deltas plus their reset count. Never turn
  a reset into zero traffic.
- Keep **15m** as the existing rolling 15-minute sample view. 15m through 90d use
  high-resolution data only while the complete range is retained; 180d, 1y,
  All, and older custom ranges use minute history. Preserve missing intervals,
  sample count, coverage, extrema, freshness, and CSV behavior across the
  source boundary.
- Keep CPU utilization optional and derived from valid `/proc/stat` deltas;
  never substitute load. Live interface activity and configured authoritative
  traffic accounting are different metrics and must remain labeled separately.
  Store interface rates as bits per second, but present live RX/TX activity as
  decimal bytes per second (`KB/s`, `MB/s`, `GB/s`). Keep declared port speed,
  tunnel bandwidth, rate limits, and speed-test throughput in bit-rate units
  (`Kbps`, `Mbps`, `Gbps`). Use separate, explicitly named formatters so these
  presentation meanings cannot drift together.
  Keep traffic history diagnostic: RX and TX are initially visible, Total is
  legend-selectable, and selector direction affects quota accounting rather
  than diagnostic-series visibility. A configured date boundary restarts both
  RX and TX cycle usage together even when only one direction is quota-billed.
- Ping selectors resolve to frozen assignments. Probe-affecting edits advance a
  generation; current/history reads must not mix generations. Preserve the
  explicit primary selection and never silently replace a removed or disabled
  primary.
- Monitoring-share target and visibility scope are immutable after creation.
  Store only the URL-secret digest and a persisted random 256-bit public key for
  each frozen share target. Never derive a public target key from the share
  digest or internal VPS ID. Keep public DTOs allowlisted, and audit each
  distinct visitor bootstrap without auditing every poll. Public projections
  must not reuse private fleet DTOs. Unauthenticated visitor reads belong only
  under `/api/v1/public/monitoring-shares/{share_id}/bootstrap` and `/data`;
  authenticated management remains under `/api/v1/monitoring-shares`.
- Update memory and PostgreSQL behavior together. Test source-tier selection,
  adaptive-span equivalence, compaction-before-pruning, partial coverage/gaps,
  Ping generation/failure modes, frozen selectors, primary uniqueness, share
  expiry/extension/revocation, visitor evidence, and public-field allowlists.

### Console interaction changes

- Use the shared searchable VPS combobox for one-VPS fields. A listed choice
  commits immediately; arbitrary text must not become a hidden selection.
- Keep multi-VPS work in its normal workflow. Direct VPS choices and a selector
  expression form one deduplicated union, with an immediate local count and
  list. The backend Review step is authoritative and freezes the exact IDs for
  confirmation; never present a local match as reviewed.
- Anchor suggestions to their input. Place them directly below when the
  rendered results fit, otherwise directly above, and keep them inside the
  viewport.
- Use `ConsoleDataGrid` for operator registries. It is the common contract for
  search, sorting, pagination, selectable rows, resizable/reorderable/hideable
  columns, saved table preferences, keyboard access, and responsive cards.
  A bounded read-only summary, comparison, or chart companion may keep a
  purpose-built layout, but it must expose explicit headings and table
  semantics when the visual content is tabular.
- Put create and refresh controls in the table header. Put selected-row
  operations in the header **Actions** menu; expose the same row operations by
  right-click on desktop. On mobile, keep selection and the header **Actions**
  menu available instead of repeating a large action set on every card. Use
  the shared V-chevron row or card expansion for inspection; navigate to a
  separate page only when the detail is itself a substantial workflow. Do not
  add a fixed rightmost Action column or a second selection/action bar.
- Keep shared table pagination choices consistent through 1,000 rows per page;
  individual tables may retain a smaller task-appropriate initial page size.
  Keep wide desktop tables horizontally scrollable, and show an explicit
  shown/hidden mark for every hideable column in the **Fields** menu.
- Keep action feedback inside the workflow that produced it. Long status or
  error content must wrap or scroll within the visible page. A terminal outcome
  rendered outside the current viewport must scroll into view (and receive
  focus when it is the active form/drawer result). Editing an input clears its
  stale error and invalidates any review snapshot derived from the old draft.
- Extend the existing console components and styles before introducing a new
  interaction pattern.

### TOTP changes

- Setup creates pending encrypted secret material but does not enable TOTP.
  Confirmation must validate the submitted password and a current code against
  that exact pending secret before enabling the factor and recording its
  accepted time step. Wrong-secret, invalid, and replayed codes remain failures.
- Reopening setup with the same pending password returns the same pending secret
  so the displayed QR code and server confirmation state cannot drift. Changing
  operator or password context invalidates the browser's pending enrollment.

### Deployment and reverse-proxy changes

The supported operator path is:

```text
client -> external TLS provider -> bundled Nginx -> private API
```

- API has no published Compose port and intentionally trusts the complete
  forwarded chain with `0.0.0.0/0` and `::/0`.
- Nginx preserves the provider chain with `$proxy_add_x_forwarded_for`.
- Nginx reaches `http://api:8080` through Docker DNS.
- Do not pin frontend/API container addresses or add a fixed subnet to
  implement proxy trust.
- If API is ever exposed directly, this trust model must be redesigned before
  that exposure is accepted.

## 4. Use Builds and the Workspace Deliberately

- Use the repository-pinned, profile-managed Rust and Node toolchains described
  in `docs/build.md`.
- Build each required binary once. Multi-VPS simulations must reuse those
  binaries; never compile separately for every simulated VPS.
- Do not run redundant full builds, Clippy, and workspace tests concurrently.
- Redirect verification builds with `CARGO_TARGET_DIR` and
  `VPSMAN_BUILD_NUMBER_DIR` when tracked build counters must remain unchanged.
- Keep generated output under an ignored task directory, never loose in the
  repository root. Remove exact temporary, cache, browser, container, and
  network resources after use.

## 5. Validate in Proportion to the Change

Run narrow checks while iterating, then one consolidated gate:

```sh
git diff --check
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

cd frontend
npm ci
npm run build
npm audit --audit-level=moderate
```

- Run the matching smoke scripts for every affected boundary.
- For UI changes, use a real browser, exercise correct and mistaken operations,
  and inspect every retained desktop/mobile screenshot.
- For monitoring UI changes, inspect Comfortable and Compact grid/detail/share
  states with 0, 1, 8, 20, 100, and 1,000 VPS fixtures. Confirm the complete
  matching fleet remains reachable, both densities differ materially, range
  controls preserve gaps, and missing data is never presented as healthy.
- For fleet behavior, include offline, reconnecting, partial, stale, timeout,
  cancellation, duplicate, and recovery paths as relevant.
- For deployment changes, render Compose, package the bundle, and perform an
  isolated `first-start` with already-built or published application assets.
- For proxy changes, send a provider-style multi-hop `X-Forwarded-For` request
  through Nginx and confirm the audit record contains the leftmost
  provider-supplied client address.
- A mock or packaging smoke does not replace verification of the actual
  published bundle.

`scripts/release-check.sh` is the aggregate local gate. Run it once after
focused checks rather than repeatedly rebuilding the same source.

## 6. Prepare a Release Without Expanding Its Contract

- Bump the Rust workspace version, every local package entry in `Cargo.lock`,
  and frontend package/lock versions together.
- Compare the candidate with the last release and advance each changed
  component's tracked build counter exactly once. The isolated aggregate
  release check intentionally does not advance release identity.
- Confirm the candidate tag is newer than every published stable release.
- The annotated tag and binaries must point to the exact reviewed commit.
- `version.json` is the authoritative release asset manifest.
- Do not publish `install-agent.sh` as a release asset or put it in the
  deployment archive. The stable installer lives at
  `deploy/install-agent.sh` on the repository branch and resolves the requested
  agent through `version.json`.
- Do not add a repository-generated `SHA256SUMS` layer. GitHub records asset
  digests; another unsigned manifest does not change the trust boundary.
- Releases and their assets are immutable. Never move a published tag or
  replace a published asset; issue a newer patch release.
- Push the reviewed branch and annotated tag atomically.

## 7. Verify the Published Result

Do not conclude when the push succeeds:

1. Wait for the complete release workflow.
2. Confirm the release tag resolves to the reviewed commit.
3. Validate the exact asset set, GitHub digests, `version.json`, archive paths,
   executable architectures, and reported versions.
4. Extract the published deployment bundle in an isolated directory and run a
   real first start.
5. Verify health, release identity, same-release no-op behavior, bounded Docker
   log configuration, and any changed business invariant.
6. Download the stable installer from the repository and use it to install the
   published agent in staged/unprivileged mode. Confirm its bytes and version
   match the selected release asset.

## 8. Close the Update Cleanly

- Remove exact build targets, frontend dependencies/output, browser artifacts,
  temporary archives, test containers, networks, volumes, and root-owned bind
  data created by the update.
- Confirm no vpsman test process or listener remains.
- Confirm `git status -sb` is clean and `main` matches `origin/main`.
- Record the commit, tag, workflow result, runtime evidence, residual risks,
  and cleanup outcome in the change review or maintainer handoff.
- Report a blocker explicitly. Never call a release verified when a required
  live path failed.

## 9. Maintainer Audit Log

### 2026-08-02 — post-v0.2.7 correctness audit

- **Category:** frozen selector integrity. **Decision:** editing a schedule or
  backup policy without changing its selector preserves its exact reviewed VPS
  IDs, including an intentionally empty set or an ID later tombstoned. Changing
  the selector is the only ordinary edit that resolves current visible VPSs.
- **Category:** authentication evidence. **Decision:** bootstrap and successful
  login now commit the operator/session, TOTP replay state, throttle cleanup,
  and success audit together. Password-only issuance rechecks the active
  operator row, unchanged password hash, and disabled-TOTP state inside that
  transaction so a concurrent password reset, disable, deletion, or TOTP enable
  cannot mint a session from stale verification.
- **Category:** console correctness. **Decision:** async TOTP actions invalidate
  stale responses when inputs change; monitoring pagination rejects incomplete
  or duplicate page sequences; detail polling permits only one in-flight read;
  invalid table searches and custom ranges remain visible; public monitoring
  keeps live transfer-rate presentation, capacity bit rates, and byte-count
  totals distinct; common form controls retain stable identifiers.
- **Category:** deployment documentation. **Decision:** the deployment archive
  rewrites both shipped runbook links to bundle-local paths, while selector and
  public-share documentation describe exact-empty maintenance and allowlisted
  public labels precisely.
- **Evidence:** focused memory and PostgreSQL authentication/selector tests,
  `cargo check`, API Clippy with warnings denied, frontend type and contract
  checks, deterministic deployment-bundle smoke, and Chrome desktop/mobile
  interaction checks passed. Chrome reported no console issues and scored the
  touched authenticated page 100 for accessibility and best practices.
