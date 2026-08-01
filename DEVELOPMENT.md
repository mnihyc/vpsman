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
  `control_plane`, `gateway_ingest`, or `worker`; `component` is the stable name
  of the writer.
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
- Put create and refresh controls in the table header. Put selected-row
  operations in the header **Actions** menu; expose the same row operations by
  right-click on desktop. On mobile, keep selection and the header **Actions**
  menu available instead of repeating a large action set on every card. Use
  row or card expansion for inspection. Do not add a fixed rightmost Action
  column.
- Keep shared table pagination choices consistent through 1,000 rows per page;
  individual tables may retain a smaller task-appropriate initial page size.
- Keep action feedback inside the workflow that produced it. Long status or
  error content must wrap or scroll within the visible page.
- Extend the existing console components and styles before introducing a new
  interaction pattern.

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
