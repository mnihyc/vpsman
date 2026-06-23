# Direct Gateway Agent Install

Agents never contact the browser panel or HTTP API during installation. A VPS is
provisioned with immutable agent identity material, the pinned gateway Noise public
key, and a prioritized raw TCP gateway endpoint list. The panel may register the matching public key for
inventory and revocation, but it does not mint install tokens.

## Required material

Generate or obtain these values before running the installer on a VPS:

- `VPSMAN_AGENT_CLIENT_ID`: stable client id, such as `agent-nrt-04`.
- `VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX`: agent Noise private key hex.
- `VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX`: gateway Noise public key hex.
- `VPSMAN_GATEWAY_ENDPOINTS`: comma or newline separated endpoint list in
  `label=host:port=priority` format. DNS names are supported; lower priority
  numbers are tried first.

Optional values:

- `VPSMAN_AGENT_BINARY_URL`: release artifact URL to download before installing.
- `VPSMAN_AGENT_BINARY_SHA256`: required 64-character SHA-256 hex when
  `VPSMAN_AGENT_BINARY_URL` is set.
- `VPSMAN_AGENT_ENABLE_SERVICE=0`: staging-only install that writes files but
  does not enable or start the service. The default is to start the service.

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
install.
It does not call `/api`, `/.well-known`, or any panel-side lookup endpoint. The
local file contains only the client id, gateway endpoints and priority, Noise
key material, server trust, and gateway retry/connect timing. Display names,
tags, update policy, execution policy, telemetry, backup limits, and tunnel
settings are server runtime config. Configure them through the API/frontend/CLI
after registering the identity; runtime changes are pushed as visible
`runtime_config_sync` jobs after the agent connects.

Runtime command traffic is protected by the gateway Noise session. No extra
server-side command-authentication key is provisioned. Operator authentication
stays at the API token layer, and privileged mutation authorization stays
request-bound through the local super-password assertion that the private
gateway verifies.
