# 02 - Install Agents With Direct Gateway Identity

Agents do not call the panel or the HTTP API during install. Each VPS receives
its per-install gateway identity material up front, then connects directly to
the raw TCP gateway endpoint list. Runtime config changes are delivered later
over the gateway channel.

## 1. Generate agent identity material

Generate a unique Noise keypair for each VPS and keep the private key on that
VPS only. Never copy one agent keypair across machines:

```sh
cargo run -p vpsctl -- noise-keygen
```

Record the public key for registration and the private key for the install
environment.

## 2. Register the public identity

Register the client id and public key from an operator shell or from Access >
VPS identities in the panel:

```sh
cargo run -p vpsctl -- agent-identity-upsert \
  --client-id v-1 \
  --client-public-key-hex <agent_noise_public_key_hex> \
  --display-name edge-nrt-04 \
  --tags country:JP,provider:acmecloud,role:edge \
  --confirmed
```

A key change requires `--replace-existing-key --confirmed`. Public-key ownership
is global: registration rejects a key already assigned to another client and a
key retired by any rotation, revocation, or deletion. Revoked or deleted client
IDs are intentionally blocked and must not be reused.

## 3. Install the agent service

Download the stable bootstrap installer. It resolves the requested agent
release from authoritative `version.json` metadata.

The installer requires a standard Linux userspace with `awk`, `curl`, `flock`,
and `mktemp`.
The generated line contains the one-time agent private key. Run it only in a
trusted shell with command history disabled, then clear the clipboard; do not
paste it into tickets, shared terminals, chat, or logs.

```sh
release_tag=vX.Y.Z
curl -fLO https://raw.githubusercontent.com/mnihyc/vpsman/main/deploy/install-agent.sh
```

Root service:

```sh
env \
  VPSMAN_AGENT_RELEASE="$release_tag" \
  VPSMAN_INSTALL_MODE=root \
  VPSMAN_AGENT_CLIENT_ID=v-1 \
  VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX=<agent_noise_private_key_hex> \
  VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX=<gateway_noise_public_key_hex> \
  VPSMAN_GATEWAY_ENDPOINTS='primary=gw.example.com:9443=10,backup=gw-backup.example.com:9443=20' \
  bash ./install-agent.sh
```

Unprivileged service:

```sh
env \
  VPSMAN_AGENT_RELEASE="$release_tag" \
  VPSMAN_INSTALL_MODE=user \
  VPSMAN_AGENT_CLIENT_ID=v-1 \
  VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX=<agent_noise_private_key_hex> \
  VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX=<gateway_noise_public_key_hex> \
  VPSMAN_GATEWAY_ENDPOINTS='primary=gw.example.com:9443=10' \
  bash ./install-agent.sh
```

`VPSMAN_GATEWAY_ENDPOINTS` accepts comma or newline separated
`label=host:port=priority` entries. DNS names are supported; lower priority
numbers are tried first. The installer enables and starts the systemd service by
default; set `VPSMAN_AGENT_ENABLE_SERVICE=0` only for a staging-only install.
The staging-only installer prints the exact foreground start command; run that
command explicitly in a container or other no-systemd environment. There is no
separate panel-side endpoint lookup.

## 4. Verify connectivity

```sh
cargo run -p vpsctl -- agents
cargo run -p vpsctl -- gateway-sessions
cargo run -p vpsctl -- key-lifecycle-report
```

In the panel, open Fleet > Instances and Access > VPS identities. The VPS should
have a direct identity record and a recent gateway session after first
telemetry.

## 5. Retire or delete safely

For emergency access lockout, revoke the current client key:

```sh
cargo run -p vpsctl -- client-key-revoke --client-id v-1 --confirmed
```

For inventory retirement, use **Fleet > Instances**, select exactly one VPS,
then choose **Actions > Review VPS deletion**. Deletion requires local privilege
unlock and confirmation, hides the VPS from normal fleet views, disconnects any
active gateway session, retires tunnel declarations using that endpoint,
immediately queues runtime-config cleanup for surviving peers, and marks pending
or active work for that VPS as skipped or lost. If cleanup cannot be queued, the
panel reports the affected peers so the operator can retry convergence before
trusting those interfaces. A deleted client id is not reused; rebuild with a new
id unless you are only rotating the current key.

## 6. Rebuild or rotate safely

For a planned rebuild that keeps the same client id, generate a new agent keypair
and run:

```sh
cargo run -p vpsctl -- agent-identity-upsert \
  --client-id v-1 \
  --client-public-key-hex <new_agent_noise_public_key_hex> \
  --replace-existing-key \
  --confirmed
```

Then reinstall the service with the new private key. Rotation permanently
retires the old key and disconnects its live session; gateway hello validation
also rejects a stale connection that reaches registration after the rotation.
If the old key was revoked or the client was deleted, choose a new client ID
instead of reusing the old one.
