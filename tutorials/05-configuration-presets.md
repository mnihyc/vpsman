# Tutorial 05: Configuration Presets

Configuration presets let an operator reuse supported agent settings without
editing every VPS separately. Open **Config > Sources** in the console.

## The Model

Each VPS has one effective preset for each supported behavior:

- `host_metrics`
- `tunnel_traffic`
- `latency_probe`
- `ospf_update_command`
- `process_inventory`
- `user_sessions`
- `command_execution`

A **System** preset ships with vpsman and is immutable. The **System default**
is the preset automatically inherited for that behavior when the VPS has no
explicit override. Nothing has to be assigned to receive it.

A **Custom** preset is operator-created. Assigning it stores an explicit
per-VPS override. **Reset to system default** deletes that override, so future
system-default changes are inherited normally.

A non-default System preset is an immutable shipped alternative and may also
be selected as an explicit override. Clone it only when its definition must
change.

The System default for `ospf_update_command` is intentionally unconfigured.
This prevents vpsman from guessing a routing stack or executable. Assign a
configured preset to a VPS before using that preset for OSPF cost control.

Names such as “Linux host metrics” describe the complete preset, while its
definition shows the exact paths or bounded command involved. A bare
implementation detail such as “procfs” is not an assignment state.

Runtime-tunnel definitions and optional per-plan OSPF command overrides are not
Configuration presets. They belong to **Network > Tunnel plans > Adapter
definitions**. The reusable VPS-level OSPF updater is an
`ospf_update_command` Configuration preset. See
[Tutorial 06](06-tunnels-routing-adapters.md).

## Assign In The Console

Open **Config > Sources**. In **Effective configuration**, choose **Change
configuration** or select one row and choose **Actions > Change**. Then:

1. Choose a behavior and preset. **Inherit system default** removes an explicit
   override; it does not create an assignment to the default preset.
2. Under **Targets**, use **Add an individual VPS** when you know the exact
   machine.
3. Optionally enter a selector for a broader scope. Its matches are added to
   the direct choices and duplicates are removed.
4. Check the immediate local count, removable direct-VPS chips, selector match
   count, and previewed VPS list.
5. Choose **Review change**. The server resolves the request again and
   freezes the exact target IDs shown in the confirmation.
6. Confirm only after checking each before/after preset. Follow runtime sync
   separately; saving an assignment is not proof that an agent applied it.

The selector is not stored as a live rule, and there is no separate bulk mode.
For one VPS, add that VPS directly. For many, use the same form with more direct
choices, a selector, or both. **Reset to system default** uses the same reviewed
target workflow and removes explicit overrides.

## See What A VPS Uses

The **Effective configuration** table shows the preset name, whether its origin
is **Inherited system default** or **Explicit override**, runtime
synchronization state, and readiness evidence. It has one row for each
VPS/behavior combination. **Configuration presets** shows each preset's
effective and explicit-use counts; open its details to see the **Effective on**
and **Explicitly selected on** VPS lists. For a single selected VPS, **Inspect
current effective config** renders the configuration currently stored by the
server; a newly chosen preset remains only a candidate until it is reviewed and
applied.

```sh
cargo run -p vpsctl -- config-sources --client-id edge-01
cargo run -p vpsctl -- config-render --client-id edge-01 --format toml
```

Keep these states distinct:

- **Desired selection** is the stored override, or the absence of one.
- **Effective configuration** is the composed configuration after inheritance.
- **Applied state** is reported by `runtime_sync`; a saved selection must not be
  presented as applied until the agent acknowledges the matching configuration.

Readiness may remain `unverified` when the agent has not supplied evidence for
a selected path or executable. That is explicit uncertainty, not a silent
fallback.

## Assign A Preset To One VPS

List the available traffic presets:

```sh
cargo run -p vpsctl -- config-presets --behavior tunnel_traffic
```

Choose the ID of an existing non-default System or Custom preset. Preview the
change for one VPS before applying it:

```sh
assignment_preview="$(
  cargo run -p vpsctl -- config-source-set \
    --behavior tunnel_traffic \
    --preset-id <preset_uuid> \
    --clients edge-01
)"
printf '%s\n' "$assignment_preview" | jq .
assignment_preview_hash="$(jq -er '.preview_hash' <<<"$assignment_preview")"
```

After checking every target and before/after value, repeat the same target and
preset arguments with local privilege material, the reviewed hash, and
confirmation:

```sh
export VPSMAN_SUPER_PASSWORD='<local-super-password>'
source ./path/to/secrets/operator-privilege.env

cargo run -p vpsctl -- config-source-set \
  --behavior tunnel_traffic \
  --preset-id <preset_uuid> \
  --clients edge-01 \
  --preview-hash "$assignment_preview_hash" \
  --confirmed
```

The confirmed command rechecks the reviewed hash and stops before privilege
signing if the preset, current assignments, or resolved target set changed.

For a multi-VPS change, `--clients`, `--tags`, and `--selector` can
contribute to one target set. Selectors are resolved for that operation; they
are not stored as live assignment rules. Later tag changes do not silently
change which VPSs have an override.

## Return To Normal Inheritance

Preview and then confirm the reset:

```sh
reset_preview="$(
  cargo run -p vpsctl -- config-source-reset \
    --behavior tunnel_traffic \
    --clients edge-01
)"
printf '%s\n' "$reset_preview" | jq .
reset_preview_hash="$(jq -er '.preview_hash' <<<"$reset_preview")"

cargo run -p vpsctl -- config-source-reset \
  --behavior tunnel_traffic \
  --clients edge-01 \
  --preview-hash "$reset_preview_hash" \
  --confirmed
```

The effective source should then report `system_default`, not a fabricated
assignment to a system preset row.

For OSPF, resetting to the System default intentionally returns that VPS to
`unconfigured`. A tunnel plan can still provide an explicit endpoint override;
otherwise OSPF status or apply dispatch is rejected rather than silently
choosing a command.

## Advanced Preset Customization

Most daily work only selects an existing preset. When no shipped choice fits,
the console provides a labeled preset editor. The headless equivalent accepts
the complete definition as JSON:

```sh
cargo run -p vpsctl -- config-preset-create \
  --behavior tunnel_traffic \
  --name traffic-vnstat \
  --description "Use the packaged vnStat binary" \
  --definition-json '{"source":"vnstat","vnstat_argv":["/usr/bin/vnstat"]}'
```

An OSPF updater preset uses the same bounded status/update contract as a
per-plan routing-cost override:

```sh
cargo run -p vpsctl -- config-preset-create \
  --behavior ospf_update_command \
  --name frr-ospf-updater \
  --description "Use the operator-owned FRR helper" \
  --definition-json '{"contract_version":2,"status_command":{"argv":["/opt/operator/frr-ospf-cost","status","--plan-id","{plan_id}","--interface","{interface}","--side","{endpoint_side}"],"max_timeout_secs":10,"max_output_bytes":16384},"update_command":{"argv":["/opt/operator/frr-ospf-cost","apply","--plan-id","{plan_id}","--interface","{interface}","--side","{endpoint_side}","--cost","{desired_cost}"],"max_timeout_secs":10,"max_output_bytes":16384}}'
```

The agent invokes each array as exact argv with closed stdin. Status must print
one decimal cost from 1 through 65535. Update succeeds only with exit code zero;
its bounded stdout becomes the job message, and the agent runs status again to
verify the requested cost.

Assign that preset with the same preview-and-confirm flow shown above. One
reviewed assignment may target many VPSs; no separate bulk screen or live
selector rule is created.

System presets cannot be edited or deleted. Clone one when customization is
needed:

```sh
cargo run -p vpsctl -- config-preset-clone \
  --preset-id <system_or_custom_preset_uuid> \
  --name traffic-vnstat-lab
```

Preview a complete candidate definition:

```sh
preset_preview="$(
  cargo run -p vpsctl -- config-preset-preview \
    --preset-id <custom_preset_uuid> \
    --definition-json '{"source":"interface_counters"}'
)"
printf '%s\n' "$preset_preview" | jq .
preset_preview_hash="$(jq -er '.preview_hash' <<<"$preset_preview")"

cargo run -p vpsctl -- config-preset-update \
  --preset-id <custom_preset_uuid> \
  --definition-json '{"source":"interface_counters"}' \
  --preview-hash "$preset_preview_hash" \
  --confirmed
```

The preview lists changed keys and every VPS whose effective configuration
would change. Update rechecks that exact preview and rejects stale hashes rather
than silently including newly affected VPSs.

Delete is intentionally limited to unused custom presets:

```sh
cargo run -p vpsctl -- config-preset-delete \
  --preset-id <unused_custom_preset_uuid> \
  --confirmed
```

## Operator Rules

- Prefer system inheritance until a real host difference requires an override.
- Use a shared custom preset for genuinely identical settings; do not create a
  one-name wrapper around a system default.
- Review affected VPSs before updating a reused preset.
- Reset overrides that are no longer necessary.
- Treat runtime sync and readiness evidence as the truth about apply state; do
  not infer success merely because the desired selection was saved.
