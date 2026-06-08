# Tutorial 02: Enroll Agents

Agents are headless Linux clients. They can run as root for full host control
or as an unprivileged user for reporting and best-effort operations.

## Create Enrollment Material

Create a short-lived enrollment token from the control plane:

```sh
cargo run -p vpsctl -- enrollment-token-create \
  --default-display-name edge-01 \
  --default-tags edge,provider-a \
  --ttl-secs 1800
```

For a new VPS, the server assigns the durable client ID when the token is
created. Use the display name and default tags for operator-facing labels.
Copy the `token` field from the response; the token is consumed when the agent
claims it.

Install directly on the target with the release installer:

```sh
curl -fsSL https://raw.githubusercontent.com/mnihyc/vpsman/main/deploy/enroll-agent.sh | env VPSMAN_INSTALL_MODE=root VPSMAN_ENROLLMENT_API_URL=https://panel.example.com VPSMAN_ENROLLMENT_TOKEN=<token> bash
```

The installer downloads the released agent and `vpsctl`, claims the token with
`vpsctl enroll-config`, writes the generated `client_id` into `agent.toml`, and
starts the systemd service. Agent config stores agent identity, gateway trust,
and server signing trust only; it does not store super-password material or
gateway privilege verifier material. Server-facing tags come from token
defaults or later panel/CLI inventory edits.

## Configure Signed Discovery

Production agents should use HTTPS discovery with signed endpoint documents.
The command-signing key stays pinned in `server_ed25519_public_key_hex`; for
discovery-only signing rotation, start the API with additional public keys:

```sh
export VPSMAN_DISCOVERY_TRUSTED_SERVER_PUBLIC_KEYS_HEX=<next_32_byte_public_key_hex>
```

New enrollment configs include those keys in
`discovery_trusted_server_ed25519_public_keys_hex`. Privileged hot config may
update this discovery trust ring before rotating the discovery signer, without
changing the command-signing authority.

## Install A Root Agent

Root mode is the production default. It installs under `/opt/vpsman`, writes
`/etc/vpsman/agent.toml`, and renders a systemd service.

```sh
curl -fsSL https://raw.githubusercontent.com/mnihyc/vpsman/main/deploy/enroll-agent.sh | env \
  VPSMAN_INSTALL_MODE=root \
  VPSMAN_ENROLLMENT_API_URL=https://panel.example.com \
  VPSMAN_ENROLLMENT_TOKEN=<token> \
  bash
```

Use this mode for tunnels, Bird2 config, process limits, backups/restores under
privileged paths, and in-place agent updates.

## Install An Unprivileged Agent

Unprivileged mode installs under the user's home and reports reduced
capabilities to the server:

```sh
mkdir -p ~/vpsman-agent
cd ~/vpsman-agent
curl -fsSL https://raw.githubusercontent.com/mnihyc/vpsman/main/deploy/enroll-agent.sh | env \
  VPSMAN_INSTALL_MODE=unprivileged \
  VPSMAN_ENROLLMENT_API_URL=https://panel.example.com \
  VPSMAN_ENROLLMENT_TOKEN=<token> \
  bash
```

Expect privileged operations such as tunnel mutation, cgroup limits, restore to
root-owned paths, or agent replacement to report `degraded_unprivileged` unless
you explicitly use a best-effort `--force-unprivileged` path where available.

## Re-Enroll A Rebuilt VPS

When a VPS is rebuilt but should keep server-side tags, history, and
restore/migration context, create a bound re-enrollment token:

```sh
cargo run -p vpsctl -- reenrollment-token-create \
  --client-id edge-01 \
  --ttl-secs 1800 \
  --default-tags rebuilt \
  --confirmed
```

Install the rebuilt agent with that token. The token is already bound to the
existing client ID; the installer does not send a client ID during claim. The
server rotates the client key only through the bound token path and preserves
server-side state.

If the current enrolled key is compromised or a rebuilt VPS should be forced to
stop using an old key before it re-enrolls, revoke the current key explicitly:

```sh
cargo run -p vpsctl -- client-key-revoke \
  --client-id edge-01 \
  --reason rebuilt-or-compromised \
  --confirmed

cargo run -p vpsctl -- key-lifecycle-report
cargo run -p vpsctl -- client-key-revocations --limit 20
```

The revoked key is rejected during gateway identity validation. A confirmed
bound rebuild token can then rotate the same `client_id` to a new key while
preserving tags, history, and migration context.

## Inspect Enrollment

```sh
cargo run -p vpsctl -- agents
cargo run -p vpsctl -- gateway-sessions
cargo run -p vpsctl -- fleet-alerts
```

The panel shows the same information in the fleet and session views.
