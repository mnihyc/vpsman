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
export VPSMAN_AGENT_UPDATE_ALLOWED_CHANNELS=stable,beta
export VPSMAN_AGENT_UPDATE_TRUSTED_SIGNING_KEYS_HEX=<64_hex_public_key>
```

Inspect releases:

```sh
cargo run -p vpsctl -- agent-update-releases --limit 10
cargo run -p vpsctl -- agent-update-release-latest --name vpsman-agent --channel stable
```

## Dispatch A Direct Update Job

Agent update jobs follow the same model as other privileged jobs: resolve the
targets, dispatch to every resolved VPS, then poll job/target/output endpoints
for progress and results. There is no staged server-side promotion queue.

```sh
cargo run -p vpsctl -- agent-update \
  --artifact-url https://updates.example/vpsman-agent \
  --sha256-hex <64_hex_sha256> \
  --artifact-signature-hex <128_hex_signature> \
  --artifact-signing-key-hex <64_hex_public_key> \
  --tags edge \
  --confirmed
```

The agent downloads over HTTPS, verifies SHA-256, verifies the signature when a
trusted key is pinned, stages the binary, and creates local rollback material.
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

## Verify Update State

```sh
cargo run -p vpsctl -- jobs --limit 20
cargo run -p vpsctl -- audit --limit 50
```

The API records activation-pending, activation-failed, heartbeat-verified, and
rollback evidence in job outputs and audit rows without storing artifact URLs,
detached signatures, public signing keys, plaintext super password, or trust
anchors in operator-facing lifecycle metadata.
