# Host Management

Host management is a direct VPS-operations surface. It does not model cloud
provider resources and it does not infer a replacement tool when the native
host capability is missing. Every inventory refresh is a normal durable job;
the latest successful snapshot remains visible when a later attempt fails.

## Product Model

| Surface | Purpose | Mutation boundary |
| --- | --- | --- |
| Remote > Processes > Host | Read the host process table from Linux procfs, ordered by resident memory. | Read-only. It never sends signals or changes priorities. |
| Remote > Processes > Managed | Operate only processes explicitly started through the vpsman process supervisor. | Start, stop, restart, and policy changes use reviewed jobs. |
| Remote > Services | Inventory and operate the one confirmed native init/service provider. | Actions carry the provider plus the reviewed active and boot states; provider or state drift rejects the action. |
| Remote > Storage | Inventory block devices and mounts using a confirmed `lsblk` output contract. | Read-only. No format, partition, mount, unmount, resize, or repair operation exists. |
| Automation > OS updates | Inspect cached native package candidates, explicitly refresh metadata where supported, and apply one reviewed per-VPS plan. | Apply carries the exact provider and SHA-256 plan hash. Drift rejects the update. No reboot is automatic. |
| Automation > Rollouts | Observe and control canary/batch delivery for jobs that explicitly requested a rollout. | Pause is immediate; resume is reviewed; cancel is terminal. Rollouts are never inferred from target count. |

Inventory pages distinguish four outcomes instead of presenting an empty table
as success: supported, unsupported, ambiguous, and probe failed. The operator
sees the selected provider, source version where available, last successful
observation time, and the latest failed attempt.

## Linux Provider Support

Capability probing is authoritative. The version examples below are parser and
selection fixtures, not a promise that every vendor-modified image exposes the
required native commands.

### Host Processes

The default source reads `/proc/<pid>/status` and `/proc/<pid>/cmdline`; it does
not depend on a particular `ps` version. Linux hosts that do not expose a usable
procfs return a failed refresh. An operator may explicitly configure the
existing custom JSON inventory command, but vpsman never selects one
automatically.

### Services

| Provider | Required evidence | Inventory/actions | Boot enablement | Logs |
| --- | --- | --- | --- | --- |
| systemd | PID 1 is `systemd`, `/run/systemd/system` exists, and `systemctl` is executable. | `systemctl` | `systemctl` | `journalctl`, when present |
| OpenRC | PID 1 is `init`, `openrc`, or `openrc-init`; `/run/openrc`, `rc-status`, and `rc-service` are present. | `rc-status` / `rc-service` | `rc-update`, when present | Explicitly unsupported |
| SysV | PID 1 is `init`, no systemd/OpenRC marker exists, `/etc/init.d` and `service` are present. | init scripts through `service` | Exactly one of `update-rc.d` or `chkconfig` | Explicitly unsupported |

Conflicting systemd/OpenRC markers are ambiguous. SysV enable/disable is also
ambiguous when both enablement tools are installed. In either case vpsman does
not guess. Inventory can remain available while a narrower action such as logs
or boot enablement is unavailable. Runtime actions require a root agent.

### OS Packages

| Distribution identity | Selected provider | Notes |
| --- | --- | --- |
| Debian, Ubuntu | APT | Requires the dpkg database and `apt-get`. Fixtures cover Debian 8/12 and Ubuntu 14.04/24.04 output shapes. |
| Arch Linux | Pacman | Requires the pacman database and `pacman`. Separate metadata refresh is unavailable because `-Sy` without the coupled full upgrade creates an unsafe partial-upgrade state. |
| CentOS/RHEL/Rocky/AlmaLinux/Oracle Linux major 7 or older | YUM | Requires the RPM database, `rpm`, and `yum`. Legacy `/etc/centos-release` is supported; fixtures include CentOS 6 and 7. |
| CentOS/RHEL/Rocky/AlmaLinux/Oracle Linux major 8 or newer, Fedora | DNF | Requires the RPM database, `rpm`, and `dnf`. Fixtures include CentOS 8. |

An unknown distribution, an RPM-family host without a parseable major version,
or a missing selected binary is unsupported. The agent never falls from DNF to
YUM, from APT to another package tool, or to a shell script.

Cached checks do not mutate package metadata. A metadata refresh is a separate
privileged action on APT, DNF, and YUM. Apply uses only the selected provider's
native full-upgrade operation against cached metadata, rechecks the exact plan
hash immediately before mutation, reports remaining candidates afterward, and
never reboots the VPS.

### Storage

Storage inventory probes the installed `lsblk --help` and chooses exactly one
advertised machine format:

- JSON (`--json`) when advertised.
- Legacy key/value pairs (`--pairs`) only when JSON is not advertised.
- Unsupported when neither format, `--paths`, or the required
  `NAME`, `TYPE`, `SIZE`, and `RO` columns are available.

There is no retry with another parser after a selected format fails. Newer
optional columns add filesystem usage, transport, model, and serial data;
missing columns remain visibly unavailable. Mounts come from bounded
`/proc/self/mountinfo` parsing. Pseudo/system mounts are hidden by default and
can be included explicitly. No `df` fallback is used, so a blocked remote mount
cannot turn inventory into an unbounded filesystem walk.

## Headless Workflows

Read-only refreshes do not require local privilege material:

```sh
vpsctl host-process-refresh --clients edge-01 --limit 200
vpsctl host-processes --client-id edge-01 --limit 200

vpsctl host-service-refresh --clients edge-01 --limit 500
vpsctl host-services --client-id edge-01 --limit 500
vpsctl host-service-logs --provider systemd --service nginx.service --clients edge-01

vpsctl host-storage-refresh --clients edge-01
vpsctl host-storage --client-id edge-01

vpsctl os-update-check --clients edge-01
vpsctl os-update-plans
vpsctl os-update-plan --client-id edge-01
```

Mutations require local unlock material and the values from the reviewed
snapshot:

```sh
vpsctl host-service-action \
  --provider systemd \
  --service nginx.service \
  --action restart \
  --expected-active-state active \
  --expected-enabled-state enabled \
  --clients edge-01 \
  --confirmed

vpsctl os-update-refresh \
  --expected-provider apt \
  --clients edge-01 \
  --confirmed

vpsctl os-update-apply \
  --client-id edge-01 \
  --provider apt \
  --plan-hash <reviewed_64_hex_plan_hash> \
  --confirmed
```

`os-update-apply` accepts one explicit VPS because its hash belongs to that
VPS's package state. Refresh each target, review each plan, and submit matching
per-VPS applies. Standard job target evidence reports queued, running,
completed, failed, skipped, or timed-out outcomes without collapsing mixed
fleet results.

For staged delivery of a suitable direct job, pass explicit canaries and batch
policy to `job-create`, then use `job-rollouts`, `job-rollout`,
`job-rollout-pause`, `job-rollout-resume --confirmed`, and
`job-cancel --confirmed`. A paused rollout permits retries only for work that
was already dispatched; later batches remain blocked until reviewed resume.

## Chart Investigation

Fleet and Network metric charts preserve missing intervals, define whether each
point is an interval average or one bounded diagnostic, and expose the exact
sample timestamp. Operators can focus a chart and use Left/Right, Home, and End
to inspect samples, toggle noisy series from the legend, restore all series,
and export only the visible series as CSV. These controls do not alter telemetry
or saved dashboard preferences.
