# Tutorial 08: Agent Updates

Agent updates use external HTTPS artifacts. The API is a private operator
control-plane service; it records release metadata and dispatches jobs, but it
does not host public update binaries and does not expose artifact byte routes.

The default self-update source is the GitHub release manifest:

```text
https://github.com/mnihyc/vpsman/releases/latest/download/version.json
```

Agents installed by the one-line installer are configured with this default
manifest URL and 24 hour interval/jitter values, but autonomous updates are
disabled unless the install command sets
`VPSMAN_AGENT_UNMANAGED_UPDATE_ENABLED=1` or a later incremental config patch
enables `update.unmanaged_enabled`. When enabled, the autonomous updater reads
`version.json`, checks `SHA256SUMS`, downloads the matching musl agent asset
from the release, stages it, activates it, and restarts the agent according to
local update settings.

The release tag owns the shipped version. Release binaries embed the same
tag-derived version that appears in `version.json`, so an agent reports
`current` only when its embedded release version matches the manifest.

Operators can enable or disable autonomous updates from the dashboard under
Config -> Rules with the predefined updater rule templates. These templates are
ordinary operator-managed records: they can be edited, cloned, or deleted.
The CLI can apply the same setting with an incremental config patch:

```toml
[update]
unmanaged_enabled = true
unmanaged_version_url = "https://github.com/mnihyc/vpsman/releases/latest/download/version.json"
unmanaged_interval_secs = 86400
unmanaged_jitter_secs = 86400
unmanaged_activate = true
unmanaged_restart_agent = true
```

```sh
cargo run -p vpsctl -- config-patch \
  --config-file ./enable-autonomous-updater.toml \
  --tags edge \
  --confirmed
```

## Publish External Release Metadata

Build the static agent and publish it through GitHub Releases or another
operator-controlled HTTPS host:

```sh
cargo build -p vpsman-agent --release --target x86_64-unknown-linux-musl
sha256sum ./target/x86_64-unknown-linux-musl/release/vpsman-agent
```

Record the external release metadata in the private API:

```sh
cargo run -p vpsctl -- agent-update-release-record \
  --name vpsman-agent \
  --version 0.1.0 \
  --channel stable \
  --artifact-url https://github.com/mnihyc/vpsman/releases/download/v0.1.0/vpsman-agent-linux-x86_64-musl \
  --sha256-hex <64_hex_sha256> \
  --size-bytes <artifact_size_bytes> \
  --confirmed
```

Add rollback metadata when the previous binary is also externally hosted:

```sh
cargo run -p vpsctl -- agent-update-release-record \
  --name vpsman-agent \
  --version 0.1.1 \
  --channel stable \
  --artifact-url https://github.com/mnihyc/vpsman/releases/download/v0.1.1/vpsman-agent-linux-x86_64-musl \
  --sha256-hex <64_hex_sha256> \
  --rollback-artifact-url https://github.com/mnihyc/vpsman/releases/download/v0.1.0/vpsman-agent-linux-x86_64-musl \
  --rollback-sha256-hex <64_hex_rollback_sha256> \
  --confirmed
```

Inspect recorded releases:

```sh
cargo run -p vpsctl -- agent-update-releases --limit 10
cargo run -p vpsctl -- agent-update-release-latest --name vpsman-agent --channel stable
```

When `require_registered_agent_updates` is enabled, every explicit update
lifecycle job is hash-bound to the release registry: manual staging and
activation require the registered primary artifact SHA, rollback requires the
registered rollback artifact SHA, and manifest-check jobs must use an explicit
resolvable manifest whose per-target architecture artifact hashes are all
registered. A check is rejected before dispatch when the manifest is missing,
the target architecture is unsupported, or any stageable artifact hash is not in
the release registry.

## Dispatch A Direct Update Job

The dashboard exposes this under Jobs -> Command dispatch -> Check update, and
Fleet -> Instances -> Check update opens the same dispatch flow with the selected
VPSs prefilled and the default GitHub manifest URL selected.

Manifest-check jobs are privileged mutating jobs when activation is enabled. They
read the supplied external `version.json`, use the explicit release asset URLs in
that manifest, verify `SHA256SUMS`, stage the binary, and can activate/restart
the agent in the same reviewed job. Agents only stage semver-newer manifests;
current versions report `current`, older versions report `downgrade_blocked`,
and non-semver versions report `version_not_orderable`:

```sh
cargo run -p vpsctl -- agent-update-check \
  --version-url https://github.com/mnihyc/vpsman/releases/latest/download/version.json \
  --tags edge \
  --confirmed
```

Manual update jobs download from the supplied external HTTPS URL, verify
SHA-256, stage the binary, and create local rollback material. Use Jobs ->
Command dispatch -> Manual update or the CLI when the operator wants to pin an
exact artifact URL and digest instead of using the release manifest. The release
registry stores URL hashes for admission/audit and does not expose raw artifact
URLs back into dispatch shortcuts.

```sh
cargo run -p vpsctl -- agent-update \
  --artifact-url https://github.com/mnihyc/vpsman/releases/download/v0.1.0/vpsman-agent-linux-x86_64-musl \
  --sha256-hex <64_hex_sha256> \
  --tags edge \
  --confirmed
```

Use normal job inspection commands to review progress:

```sh
cargo run -p vpsctl -- jobs --limit 20
cargo run -p vpsctl -- job-targets --job-id <job_uuid>
cargo run -p vpsctl -- job-target-status-download \
  --job-id <job_uuid> \
  --output-file ./agent-update-status.tar
cargo run -p vpsctl -- job-outputs --job-id <job_uuid>
cargo run -p vpsctl -- job-follow --job-id <job_uuid>
```

## Activate Or Roll Back

Activation and rollback are explicit privileged jobs. Dispatch them to the
operator-reviewed targets just like any other mutating job.

Activate a staged binary:

```sh
cargo run -p vpsctl -- agent-update-activate \
  --staged-sha256-hex <64_hex_sha256> \
  --tags edge \
  --restart-agent \
  --confirmed
```

Roll back to the local rollback binary:

```sh
cargo run -p vpsctl -- agent-update-rollback \
  --rollback-sha256-hex <64_hex_rollback_sha256> \
  --tags edge \
  --confirmed
```

Unprivileged targets report degraded mutation capability by default. Use
`--force-unprivileged` only for a deliberate best-effort attempt.
