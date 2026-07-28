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
gh release view --repo mnihyc/vpsman
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
- Daily operator paths remain concise. A correctness fix must not add routine
  prompts, concepts, or controls that operators do not need.
- Keep provider integrations adapter- or template-driven. Do not add
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

- Files already pinned in `migrations/released-checksums.sha384` are immutable.
- Compatible schema work after the v0.2.0 baseline starts with the next
  sequential migration and remains append-only.
- Never edit `_sqlx_migrations`, replace released checksums, or mark an
  unapplied migration as applied.
- Add the compatibility note and run
  `VPSMAN_REQUIRE_RELEASE_TAGS=1 bash scripts/audit-migrations.sh`.

### Protocol and rolling-component changes

- Account for mixed server, gateway, worker, CLI, frontend, and agent versions.
- Preserve explicit schema/version rejection; do not parse an unknown shape as
  an older one.
- Regenerate and verify frontend protocol contracts when shared types change.

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
