# Tutorial 05: Source Templates

Source templates are the business model for heterogeneous VPS fleets. A VPS
selects a template for each source domain instead of forcing commands directly
into every workflow.

## Template Types

- Built-in default: cheap Linux defaults such as procfs/sysfs telemetry.
- Built-in alternative: curated optional sources such as `vnstat` traffic,
  pinned `ping`, BusyBox `ash`, or ifupdown/Bird2 hooks.
- Shared custom template: one operator-managed template assigned to tags or
  explicit clients.
- VPS-local custom template: a one-off template owned by a single VPS.

Bulk update means updating the template definition. Assignment decides which VPSs
select that template.

## Inspect Templates And Assignments

```sh
cargo run -p vpsctl -- source-templates
cargo run -p vpsctl -- source-template-assignments
cargo run -p vpsctl -- source-status
```

Use the Tags panel for the same workflow when operating visually.

## Create A Shared Template

Example: create a shared traffic template for hosts where `vnstat` is installed:

```sh
cargo run -p vpsctl -- source-template-create \
  --domain runtime_traffic_accounting_source \
  --name traffic-vnstat-json \
  --scope shared \
  --definition-json '{"source":"vnstat","runtime_vnstat_argv":["/usr/bin/vnstat"]}'
```

Then assign it to a tag:

```sh
cargo run -p vpsctl -- source-template-assign \
  --domain runtime_traffic_accounting_source \
  --template-id <template_uuid> \
  --tags edge \
  --confirmed
```

Example: create a locked-down command execution policy for a provider group
where scripts should always run from a work directory with a clean environment
and no PTY:

```sh
cargo run -p vpsctl -- source-template-create \
  --domain command_execution_policy \
  --name provider-clean-shell \
  --scope shared \
  --definition-json '{"shell_script_argv":["/bin/sh","-lc"],"working_directory":"/var/lib/vpsman/work","environment_policy":"clean","environment_keep":["PATH"],"environment_set":{"VPSMAN_ENV":"production"},"pty_policy":"disabled","process_cleanup":"direct_child"}'

cargo run -p vpsctl -- source-template-assign \
  --domain command_execution_policy \
  --template-id <template_uuid> \
  --tags provider:provider-a \
  --confirmed
```

Render the selected incremental config patch for a VPS:

```sh
cargo run -p vpsctl -- source-config-patch --client-id edge-01
```

Apply source template config through a privileged incremental patch:

```sh
cargo run -p vpsctl -- source-config-patch-apply --client-id edge-01 --confirmed
```

## Read Active Source Status

`source-status` shows the selected source model for each VPS. Continuous
sources such as telemetry and tunnel traffic report samples when available.
Workflow-only sources such as process inventory, user sessions, latency probes,
speed tests, and command execution policy report `ready_on_demand` with the
privileged workflow and sanitized selected-policy evidence. Process inventory
rows also show whether the agent can enforce process limits, cannot report that
yet, or is running unprivileged and will degrade root-only limit operations
unless the operator forces a best-effort run:

```sh
cargo run -p vpsctl -- source-status --client-id edge-01
cargo run -p vpsctl -- source-status --domain command_execution_policy
```

Use this before debugging a host. It tells you which template the VPS is using
without exposing env values, privilege material, command output, or object-store
paths.

## Clone, Test, And Update

Clone a shared template before changing production assignments:

```sh
cargo run -p vpsctl -- source-template-clone \
  --template-id <template_uuid> \
  --name traffic-vnstat-json-lab
```

Test the candidate definition and compare it:

```sh
cargo run -p vpsctl -- source-template-test \
  --template-id <template_uuid> \
  --definition-json '{"source":"interface_counters"}'
cargo run -p vpsctl -- source-template-diff \
  --template-id <template_uuid> \
  --definition-json '{"source":"vnstat"}'
```

Update the shared template only after the lab candidate is clean:

```sh
cargo run -p vpsctl -- source-template-update \
  --template-id <template_uuid> \
  --definition-json '{"source":"vnstat","runtime_vnstat_argv":["/usr/bin/vnstat"]}' \
  --confirmed
```

## Operator Rules

- Do not hardcode `/proc`, `/sys`, `vnstat`, `ping`, `birdc`, `ip`, or `tc`
  assumptions into workflows. Put them in templates or agent config.
- Prefer shared templates for provider families and VPS-local templates for one-off
  images.
- Keep defaults cheap for 1-core, 256MB VPSs. Use custom commands only when
  their value justifies CPU and memory cost.
- Use status readback to confirm which source is active before debugging a
  host-specific issue.
