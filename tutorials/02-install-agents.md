# 02 - Install Agents With Direct Gateway Identity

Agents do not call the panel or the HTTP API during install. Each VPS receives
all immutable gateway identity material up front, then connects directly to the
raw TCP gateway endpoint list. Runtime config changes are delivered later over
the gateway channel.

## 1. Generate agent identity material

Generate a Noise keypair for each VPS and keep the private key on that VPS only:

```sh
cargo run -p vpsctl -- noise-keygen
```

Record the public key for registration and the private key for the install
environment.

## 2. Register the public identity

Register the client id and public key from an operator shell or from Access > VPS
keys in the panel:

```sh
cargo run -p vpsctl -- agent-identity-upsert \
  --client-id edge-nrt-04 \
  --client-public-key-hex <agent_noise_public_key_hex> \
  --display-name edge-nrt-04 \
  --tags country:JP,provider:acmecloud,role:edge \
  --confirmed
```

A key change requires `--replace-existing-key --confirmed`. Revoked or deleted
client ids are intentionally blocked and must not be reused.

## 3. Install the agent service

Root service:

```sh
curl -fsSL https://raw.githubusercontent.com/mnihyc/vpsman/main/deploy/install-agent.sh | env \
  VPSMAN_INSTALL_MODE=root \
  VPSMAN_AGENT_CLIENT_ID=edge-nrt-04 \
  VPSMAN_AGENT_DISPLAY_NAME=edge-nrt-04 \
  VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX=<agent_noise_private_key_hex> \
  VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX=<gateway_noise_public_key_hex> \
  VPSMAN_GATEWAY_ENDPOINTS='primary=gw.example.com:9443=10,backup=gw-backup.example.com:9443=20' \
  bash
```

Unprivileged service:

```sh
curl -fsSL https://raw.githubusercontent.com/mnihyc/vpsman/main/deploy/install-agent.sh | env \
  VPSMAN_INSTALL_MODE=user \
  VPSMAN_AGENT_CLIENT_ID=edge-nrt-04 \
  VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX=<agent_noise_private_key_hex> \
  VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX=<gateway_noise_public_key_hex> \
  VPSMAN_GATEWAY_ENDPOINTS='primary=gw.example.com:9443=10' \
  bash
```

`VPSMAN_GATEWAY_ENDPOINTS` accepts comma or newline separated
`label=host:port=priority` entries. DNS names are supported; lower priority
numbers are tried first. There is no separate panel-side endpoint lookup.

## 4. Verify connectivity

```sh
cargo run -p vpsctl -- agents
cargo run -p vpsctl -- gateway-sessions
cargo run -p vpsctl -- key-lifecycle-report
```

In the panel, open Fleet > Instances and Access > VPS keys. The VPS should have
a direct identity record and a recent gateway session after first telemetry.

## 5. Rebuild or rotate safely

For a planned rebuild that keeps the same client id, generate a new agent keypair
and run:

```sh
cargo run -p vpsctl -- agent-identity-upsert \
  --client-id edge-nrt-04 \
  --client-public-key-hex <new_agent_noise_public_key_hex> \
  --replace-existing-key \
  --confirmed
```

Then reinstall the service with the new private key. If the old key was revoked
or the client was deleted, choose a new client id instead of reusing the old one.
