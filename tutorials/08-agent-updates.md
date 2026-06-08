# Tutorial 08: Agent Updates

Agent updates are privilege-gated and staged. The default flow is publish
metadata or upload a hosted artifact, dispatch staging, then activate or roll
back.
When the API hosts artifacts, set `VPSMAN_UPDATE_ARTIFACT_PUBLIC_BASE_URL` to
the HTTPS origin that fronts `/api/v1/agent-update-artifacts/{sha256}` so
release records can show operator-ready download URLs.

## Sign An Artifact

Build the static agent and create detached signature metadata:

```sh
cargo build -p vpsman-agent --release --target x86_64-unknown-linux-musl
cargo run -p vpsctl -- agent-update-signature \
  --artifact-file ./target/x86_64-unknown-linux-musl/release/vpsman-agent \
  --signing-seed-hex <64_hex_seed>
```

## Publish Or Upload Release Metadata

Metadata-only release record:

```sh
cargo run -p vpsctl -- agent-update-release-publish \
  --name vpsman-agent \
  --version 0.1.0 \
  --channel stable \
  --artifact-file ./target/x86_64-unknown-linux-musl/release/vpsman-agent \
  --artifact-url https://updates.example/vpsman-agent \
  --signing-seed-hex <64_hex_seed> \
  --confirmed
```

Add a rollback bundle to metadata-only release records when the previous binary
is hosted elsewhere:

```sh
cargo run -p vpsctl -- agent-update-release-publish \
  --name vpsman-agent \
  --version 0.1.1 \
  --channel stable \
  --artifact-file ./target/x86_64-unknown-linux-musl/release/vpsman-agent \
  --artifact-url https://updates.example/vpsman-agent \
  --signing-seed-hex <64_hex_seed> \
  --rollback-artifact-file ./target/previous/vpsman-agent \
  --rollback-artifact-url https://updates.example/vpsman-agent.previous \
  --confirmed
```

Hosted local-object-store artifact:

```sh
cargo run -p vpsctl -- agent-update-artifact-upload \
  --name vpsman-agent \
  --version 0.1.0 \
  --channel stable \
  --artifact-file ./target/x86_64-unknown-linux-musl/release/vpsman-agent \
  --signing-seed-hex <64_hex_seed> \
  --confirmed
```

Production streamed hosted artifact with rollback bundle:

```sh
cargo run -p vpsctl -- agent-update-artifact-upload \
  --name vpsman-agent \
  --version 0.1.1 \
  --channel stable \
  --artifact-file ./target/x86_64-unknown-linux-musl/release/vpsman-agent \
  --signing-seed-hex <64_hex_seed> \
  --rollback-artifact-file ./target/previous/vpsman-agent \
  --stream \
  --confirmed
```

`--stream` sends raw signed artifact bodies to
`/api/v1/agent-update-artifacts/stream`; the API hashes while writing a temp
file, verifies the detached signature, commits content-addressed objects, then
records the release from `/api/v1/agent-update-releases/hosted`. Use the
non-stream JSON/base64 upload only for small compatibility workflows.

Production deployments can restrict release policy:

```sh
export VPSMAN_AGENT_UPDATE_ALLOWED_CHANNELS=stable,canary
export VPSMAN_AGENT_UPDATE_TRUSTED_SIGNING_KEYS_HEX=<64_hex_public_key>
```

Inspect releases:

```sh
cargo run -p vpsctl -- agent-update-releases --limit 10
cargo run -p vpsctl -- agent-update-release-latest --name vpsman-agent --channel stable
```

## Configure Rollout Policy Presets

Create reusable canary and automation-gate defaults by hierarchy:

```sh
cargo run -p vpsctl -- agent-update-rollout-policy-create \
  --name hetzner-stable \
  --scope-kind provider \
  --scope-value hetzner \
  --channel stable \
  --canary-count 2 \
  --health-gate manual_after_canary \
  --priority 10 \
  --confirmed

cargo run -p vpsctl -- agent-update-rollout-policies --limit 10
```

The same command updates an existing policy with the same name. Scope can be
`global`, `tag`, `pool`, or `provider`. Channel-specific policies apply when
the staged artifact matches a registered release on that channel. Explicit
`agent-update --canary-count` overrides the preset, and rollout rows show the
applied policy id/name.

## Stage An Update

```sh
cargo run -p vpsctl -- agent-update \
  --artifact-url https://updates.example/vpsman-agent \
  --sha256-hex <64_hex_sha256> \
  --artifact-signature-hex <128_hex_signature> \
  --artifact-signing-key-hex <64_hex_public_key> \
  --tags edge \
  --canary-count 2 \
  --confirmed
```

The agent downloads over HTTPS, verifies SHA-256, verifies the signature when a
trusted key is pinned, stages the binary, and creates rollback material.

## Activate Or Roll Back

Operator-driven rollout batch:

```sh
cargo run -p vpsctl -- agent-update-rollouts --limit 10
cargo run -p vpsctl -- agent-update-rollout-control \
  --rollout-id <rollout_uuid> \
  --health-gate manual_after_canary \
  --confirmed
cargo run -p vpsctl -- agent-update-rollout-activate \
  --rollout-id <rollout_uuid> \
  --batch-size 2 \
  --restart-agent \
  --confirmed
```

Pause a rollout during maintenance, then resume normal assisted
recommendations:

```sh
cargo run -p vpsctl -- agent-update-rollout-control \
  --rollout-id <rollout_uuid> \
  --pause \
  --pause-reason maintenance \
  --confirmed
cargo run -p vpsctl -- agent-update-rollout-control \
  --rollout-id <rollout_uuid> \
  --resume \
  --health-gate heartbeat_verified \
  --confirmed
```

Rollout control is metadata-only. It does not send the super password or allow
the server to dispatch privileged activation without a request-bound privilege
assertion verified by the private gateway. Use `manual_after_canary` to stop
after the first verified canary, and `manual_only` when every promotion should
be explicitly selected by an operator.

The worker records rollout recommendations and timeout evidence, but
activation and rollback are still explicit operator actions. Run
`agent-update-rollout-activate` for the next reviewed batch and
`agent-update-rollout-rollback` when a target should be recovered.

Direct activation:

```sh
cargo run -p vpsctl -- agent-update-activate \
  --staged-sha256-hex <64_hex_sha256> \
  --tags edge \
  --restart-agent \
  --confirmed
```

Roll back:

```sh
cargo run -p vpsctl -- agent-update-rollout-rollback --rollout-id <rollout_uuid> --confirmed
cargo run -p vpsctl -- agent-update-rollback --rollback-sha256-hex <64_hex_rollback_sha256> --tags edge --confirmed
```

Unprivileged targets report degraded mutation capability by default. Use
`--force-unprivileged` only for a deliberate best-effort attempt.

## Verify Rollout State

```sh
cargo run -p vpsctl -- agent-update-rollouts --limit 10
cargo run -p vpsctl -- audit --limit 50
```

The API records activation-pending, activation-failed, heartbeat-verified,
heartbeat-timeout, and rollback evidence. Rollout records also show pause
state, health gate, and worker lease evidence without storing artifact URLs,
detached signatures, public signing keys, plaintext super password, or trust
anchors.
