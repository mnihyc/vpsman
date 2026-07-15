# Direct Gateway Agent Install

Agents never contact the browser panel or HTTP API during installation. Each VPS
is provisioned with its own agent identity material, the pinned gateway Noise
public key, and a prioritized raw TCP gateway endpoint list. The panel registers
the matching agent public key for inventory and revocation; it does not mint
install tokens.

## Required material

Generate or obtain these values before running the installer on a VPS:

- `VPSMAN_AGENT_CLIENT_ID`: stable client ID. New panel registrations default to
  a numerical ID such as `1042`; imported legacy string IDs remain supported.
- `VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX`: the unique agent Noise private key for
  this VPS. Do not copy one VPS keypair to another VPS.
- `VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX`: gateway Noise public key hex.
- `VPSMAN_GATEWAY_ENDPOINTS`: comma or newline separated endpoint list in
  `label=host:port=priority` format. DNS names are supported; lower priority
  numbers are tried first.

Optional values:

- `VPSMAN_AGENT_BINARY_URL`: release artifact URL to download before installing.
- `VPSMAN_AGENT_BINARY_SHA256`: required 64-character SHA-256 hex when
  `VPSMAN_AGENT_BINARY_URL` is set.
- `VPSMAN_AGENT_ENABLE_SERVICE=0`: staging-only install that writes files but
  does not enable or start the service. The installer prints the exact
  foreground command after staging. The default is to start the service.

## Register the public identity

Register the agent public key in the panel/API so fleet inventory and gateway key
validation know the identity:

```sh
vpsctl noise-keygen
export VPSMAN_SUPER_PASSWORD='<local_super_password>'
export VPSMAN_SUPER_SALT_HEX='<server_super_salt_hex>'
vpsctl agent-identity-upsert \
  --client-id agent-nrt-04 \
  --client-public-key-hex <agent_noise_public_key_hex> \
  --display-name edge-nrt-04 \
  --tags country:JP,role:edge \
  --confirmed
```

Use `--replace-existing-key --confirmed` only for a planned key rotation of a
non-revoked, non-deleted identity. Revoked or deleted client ids are blocked and
must not be reused.

Public-key ownership is global, not scoped only by client ID. Registration
rejects a key already assigned to another VPS and rejects any key retired by
rotation, revocation, or deletion. Rotation permanently retires the previous key;
reinstall that VPS with the new private key. Gateway hello validation rechecks
current ownership, so a stale connection cannot continue by claiming the old key.

## Install on the VPS

Root service example:

```sh
curl -fsSL https://raw.githubusercontent.com/mnihyc/vpsman/main/deploy/install-agent.sh | env \
  VPSMAN_INSTALL_MODE=root \
  VPSMAN_AGENT_CLIENT_ID=agent-nrt-04 \
  VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX=<agent_noise_private_key_hex> \
  VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX=<gateway_noise_public_key_hex> \
  VPSMAN_GATEWAY_ENDPOINTS='primary=gw.example.com:9443=10,backup=gw-backup.example.com:9443=20' \
  bash
```

Unprivileged service example:

```sh
curl -fsSL https://raw.githubusercontent.com/mnihyc/vpsman/main/deploy/install-agent.sh | env \
  VPSMAN_INSTALL_MODE=user \
  VPSMAN_AGENT_CLIENT_ID=agent-nrt-04 \
  VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX=<agent_noise_private_key_hex> \
  VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX=<gateway_noise_public_key_hex> \
  VPSMAN_GATEWAY_ENDPOINTS='primary=gw.example.com:9443=10' \
  bash
```

The installer writes a bootstrap-only `agent.toml`, installs a systemd unit, and starts the agent
unless `VPSMAN_AGENT_ENABLE_SERVICE=0` is set for an intentional staging-only
install. A staging-only run prints the exact `VPSMAN_AGENT_STATE_DIR=... \
vpsman-agent --config ... run` command needed to start the agent in the
foreground.
It does not call `/api`, `/.well-known`, or any panel-side lookup endpoint. The
local file contains only the client id, gateway endpoints and priority, Noise
key material, server trust, and gateway retry/connect timing. Display names,
tags, update policy, execution policy, telemetry, backup limits, and tunnel
settings are server runtime config. Configure them through the API/frontend/CLI
after registering the identity; runtime changes are pushed as visible
`runtime_config_sync` jobs after the agent connects, and the server marks the
per-agent applied runtime config only after that agent's sync target succeeds.

Runtime command traffic is protected by the gateway Noise session. No extra
server-side command-authentication key is provisioned. Operator authentication
stays at the API token layer, and privileged mutation authorization stays
request-bound through the local super-password assertion that the private
gateway verifies.
