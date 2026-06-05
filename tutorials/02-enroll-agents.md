# Tutorial 02: Enroll Agents

Agents are headless Linux clients. They can run as root for full host control
or as an unprivileged user for reporting and best-effort operations.

## Create Enrollment Material

Create a short-lived enrollment token from the control plane:

```sh
cargo run -p vpsctl -- enrollment-token-create \
  --allowed-client-id edge-01 \
  --default-display-name edge-01 \
  --default-tags edge,provider-a \
  --ttl-secs 1800
```

Render an enrolled agent config on the target or in a secure install
environment:

```sh
export VPSMAN_API_URL=https://panel.example.com
export VPSMAN_ENROLLMENT_TOKEN=<token>
export VPSMAN_CLIENT_ID=edge-01
export VPSMAN_SUPER_PASSWORD=<local_super_password>
export VPSMAN_SUPER_SALT_HEX=<64_hex_salt>

cargo run -p vpsctl -- enroll-config \
  --client-id edge-01 \
  --super-salt-hex "$VPSMAN_SUPER_SALT_HEX" \
  --output-file ./agent.toml
```

The rendered config stores derived proof material and agent identity. It does
not store the plaintext super password. Server-facing alias, pool, country, and
tags come from enrollment-token defaults or later panel/CLI inventory edits;
the agent does not claim or mutate them.

## Configure Signed Discovery

Production agents should use HTTPS discovery with signed endpoint documents.
The command-signing key stays pinned in `server_ed25519_public_key_hex`; for
discovery-only signing rotation, start the API with additional public keys:

```sh
export VPSMAN_DISCOVERY_TRUSTED_SERVER_PUBLIC_KEYS_HEX=<next_32_byte_public_key_hex>
```

New enrollment configs include those keys in
`discovery_trusted_server_ed25519_public_keys_hex`. Proof-gated hot config may
update this discovery trust ring before rotating the discovery signer, without
changing the privileged command-signing authority.

## Install A Root Agent

Root mode is the production default. It installs under `/opt/vpsman`, writes
`/etc/vpsman/agent.toml`, and renders systemd plus SysV init autostart assets.

```sh
config_b64="$(base64 < ./agent.toml | tr -d '\n')"
env VPSMAN_INSTALL_MODE=root \
  VPSMAN_AGENT_URL=https://updates.example/vpsman-agent \
  VPSMAN_AGENT_SHA256_HEX=<64_hex_sha256> \
  VPSMAN_AGENT_CONFIG_B64="$config_b64" \
  bash scripts/install-agent.sh
```

Use this mode for tunnels, Bird2 config, process limits, backups/restores under
privileged paths, and in-place agent updates.

## Install An Unprivileged Agent

Unprivileged mode installs under the user's home and reports reduced
capabilities to the server:

```sh
env VPSMAN_INSTALL_MODE=unprivileged \
  VPSMAN_SERVICE_HOME=/home/vpsman \
  VPSMAN_AGENT_URL=https://updates.example/vpsman-agent \
  VPSMAN_AGENT_SHA256_HEX=<64_hex_sha256> \
  VPSMAN_AGENT_CONFIG_B64="$config_b64" \
  bash scripts/install-agent.sh
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

Install the new agent with that token and the same `client_id`. The server
rotates the client key only through the bound token path and preserves
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
