# Enroll Remote VPS Agents

This guide is for operators who deploy the control plane with `./deploy` and
want remote VPS agents to connect back to it.

Remote agents use the gateway TCP listener on `9443`. Do not expose the gateway
control listener on `9444`; it is only for API-to-gateway traffic inside the
Compose network.

Protocol surfaces are separate:

- Enrollment API: HTTP(S) URL used by `vpsctl` and `deploy/enroll-agent.sh` to
  claim enrollment tokens.
- Agent gateway: raw TCP `host:port` endpoint, usually `your-gateway.example:9443`.
  Do not add `http://` or `https://` to this endpoint.
- Gateway control: HTTP listener on `9444`, private/internal only.

## 1. Prepare The Public Gateway

Public agent ingress must use enrolled Noise IK identity. `dev_xx` mode is
disabled unless `VPSMAN_DEBUG_INTERNAL_TEST_MODE=true` and is for dangerous
internal tests only.

Generate a gateway Noise keypair with the released `vpsctl` binary, or from a
source checkout with `cargo run -p vpsctl -- noise-keygen`:

```sh
vpsctl noise-keygen
```

Set the deploy environment:

```env
VPSMAN_GATEWAY_NOISE_MODE=enrolled_ik
VPSMAN_GATEWAY_PRIVATE_KEY_HEX=<gateway_private_key_hex>
VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX=<gateway_public_key_hex>
VPSMAN_PUBLIC_GATEWAY_ENDPOINTS=public=your-gateway.example:9443=10
VPSMAN_DISCOVERY_URL=https://panel.example.com/.well-known/vpsman/endpoints.json
```

Leave `VPSMAN_GATEWAY_EXPECT_CLIENT_PUBLIC_KEY_HEX` empty for normal fleets so
the gateway validates each enrolled agent key through the API.

`VPSMAN_PUBLIC_GATEWAY_ENDPOINTS` is a raw TCP endpoint list in
`label=host:port=priority` form. `VPSMAN_DISCOVERY_URL` is an optional HTTP(S)
URL for publishing that raw TCP endpoint list; it is not the gateway transport.

Expose TCP `9443` through one of these paths:

```yaml
gateway:
  ports:
    - "0.0.0.0:9443:9443"
```

or keep Compose localhost-only and forward raw TCP from a proxy, firewall, VPN,
or tunnel to `127.0.0.1:9443`. HTTP reverse proxying is not enough for this
path.

Apply the deploy changes:

```sh
cd deploy
./update.sh latest
```

## 2. Create An Enrollment Token

Create short-lived tokens. For a new VPS, the panel assigns an opaque UUID
client id when the token is created. Use `--default-display-name` for the human
label stored in the control plane.

```sh
export VPSMAN_ENROLLMENT_API_URL=https://panel.example.com
export VPSMAN_API_TOKEN=<operator_access_token>

vpsctl --api-url "$VPSMAN_ENROLLMENT_API_URL" --api-token "$VPSMAN_API_TOKEN" \
  --output pretty-json \
  enrollment-token-create \
  --default-display-name edge-01 \
  --default-tags edge,prod \
  --ttl-secs 1800
```

Copy the `token` value from the response. Tokens are consumed when claimed. The
`assigned_client_id` value is the server-side VPS identity that will be written
to the agent config during claim.

`--default-display-name` is not written into `agent.toml`. Do not supply a
client id for normal provisioning; use `reenrollment-token-create` when a
rebuilt VPS must keep an existing server identity.

Keep a local super password and a 32-byte salt for privileged operations. The
agent config stores only the derived proof key, not the plaintext password.

```sh
export VPSMAN_SUPER_PASSWORD=<local_super_password>
export VPSMAN_SUPER_SALT_HEX=<64_hex_salt>
```

## 3. Choose Install Mode

The agent binary does not install itself as a boot service. It can hot-replace
its own binary during an agent update, and it can restart itself after update
activation, but boot launch is owned by the deploy installer through systemd.

`deploy/enroll-agent.sh` has two modes:

- `root`: privileged service install for VPSs where the agent should manage
  root-owned operations.
- `unprivileged`: normal-user install with a user systemd unit and agent files
  in the current directory.

Use `VPSMAN_SKIP_SERVICE=1` only for manual or external-supervisor installs.

## 4. Root Privileged Install

Run as root on the VPS:

```sh
curl -fsSL https://raw.githubusercontent.com/mnihyc/vpsman/main/deploy/enroll-agent.sh | env \
  VPSMAN_INSTALL_MODE=root \
  VPSMAN_ENROLLMENT_API_URL=https://panel.example.com \
  VPSMAN_ENROLLMENT_TOKEN=<token> \
  VPSMAN_SUPER_PASSWORD=<local_super_password> \
  VPSMAN_SUPER_SALT_HEX=<64_hex_salt> \
  bash
```

Root mode writes:

- `/opt/vpsman/bin/vpsman-agent`
- `/opt/vpsman/bin/vpsctl`
- `/etc/vpsman/agent.toml`
- `/etc/systemd/system/vpsman-agent.service`

The service runs:

```sh
/opt/vpsman/bin/vpsman-agent --config /etc/vpsman/agent.toml run
```

The unit uses `Restart=always`, `RestartSec=5`, and starts after
`network-online.target`. It also sets
`VPSMAN_AGENT_RESTART_MODE=signal_only`, so update activation asks systemd to
restart the stable binary path instead of spawning a replacement process itself.

## 5. Unprivileged Install

Run as the normal user that should own the agent files. Start from the desired
agent directory; the installer intentionally keeps config and local state there.

```sh
mkdir -p ~/vpsman-agent
cd ~/vpsman-agent

curl -fsSL https://raw.githubusercontent.com/mnihyc/vpsman/main/deploy/enroll-agent.sh | env \
  VPSMAN_INSTALL_MODE=unprivileged \
  VPSMAN_ENROLLMENT_API_URL=https://panel.example.com \
  VPSMAN_ENROLLMENT_TOKEN=<token> \
  VPSMAN_SUPER_PASSWORD=<local_super_password> \
  VPSMAN_SUPER_SALT_HEX=<64_hex_salt> \
  bash
```

Unprivileged mode writes:

- `./bin/vpsman-agent`
- `./bin/vpsctl`
- `./agent.toml`
- `./supervisor/`
- `./systemd/vpsman-agent.service`
- `~/.config/systemd/user/vpsman-agent.service`

The user unit runs:

```sh
VPSMAN_SUPERVISOR_DIR=./supervisor ./bin/vpsman-agent --config ./agent.toml run
```

The unit uses `Restart=always`, `RestartSec=5`, and `WantedBy=default.target`.
It also sets `VPSMAN_AGENT_RESTART_MODE=signal_only`, so update activation asks
user systemd to restart the stable binary path instead of spawning a replacement
process itself. If user systemd is not available in the install session, the
script leaves the unit file in place and prints the `systemctl --user` command
to run later.

For boot before login, user lingering must be enabled by an administrator:

```sh
loginctl enable-linger <user>
```

Without lingering, the user service usually starts after that user logs in.

Unprivileged mode reports reduced capabilities to the control plane. Root-only
network, restore, file, process-limit, and system paths may be unsupported or
require explicit best-effort commands.

## 6. Agent-Owned Runtime Files

The installer owns the initial binary, CLI, config, and systemd unit. After the
service is running, the agent may also write these files:

- Agent self-update sidecars beside the running binary:
  `vpsman-agent.next`, `vpsman-agent.rollback`, and
  `vpsman-agent.activated.json`.
- Hot-config temp and rollback files beside `agent.toml`.
- Process supervisor state under `/var/lib/vpsman/supervisor` in root mode, or
  `./supervisor` in unprivileged mode.
- File-transfer temp files named `.vpsman-transfer-*` beside the commanded
  destination path.
- Explicit network, backup, restore, and command targets requested by an
  operator job.

The systemd unit and agent update flow are intentionally separated. Without
systemd, the agent's default activation path can spawn a replacement process and
then terminate the old process. The installer-generated systemd units set
`VPSMAN_AGENT_RESTART_MODE=signal_only`, so activation only terminates the old
process after replacing the file at the stable binary path. `Restart=always`
then starts exactly the unit command again from that stable path.

## 7. Verify The Agent

From the operator machine:

```sh
vpsctl --api-url "$VPSMAN_ENROLLMENT_API_URL" --api-token "$VPSMAN_API_TOKEN" agents
vpsctl --api-url "$VPSMAN_ENROLLMENT_API_URL" --api-token "$VPSMAN_API_TOKEN" gateway-sessions
```

On a root-mode VPS:

```sh
systemctl status vpsman-agent
journalctl -u vpsman-agent -n 100 --no-pager
```

On an unprivileged VPS:

```sh
systemctl --user status vpsman-agent
journalctl --user -u vpsman-agent -n 100 --no-pager
```

## Re-Enroll A Rebuilt VPS

Use a confirmed re-enrollment token to preserve server-side identity, tags,
history, and migration context:

```sh
vpsctl --api-url "$VPSMAN_ENROLLMENT_API_URL" --api-token "$VPSMAN_API_TOKEN" \
  --output pretty-json \
  reenrollment-token-create \
  --client-id <existing_client_id> \
  --ttl-secs 1800 \
  --default-tags rebuilt \
  --confirmed
```

Then rerun `deploy/enroll-agent.sh` on the rebuilt VPS with the new token. The
token is already bound to the existing server identity; the installer does not
send or generate a client id during claim.
