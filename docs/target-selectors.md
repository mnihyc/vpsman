# Target Selectors

`vpsman` target selection is tag-first. Operators should model provider,
country, role, application, and optional grouping as ordinary tags such as
`provider:alpha`, `country:US`, `role:edge`, `app:nginx`, or `pool:legacy`.

## Persisted Tags

Persisted tags are created with `tag-create`, assigned with `agent-tag`, stored
on the VPS record, and listed in the Tags panel. Namespaced values are just tag
names; `provider:alpha`, `country:US`, and `pool:legacy` have no separate
server-side object model.

`id:` and `name:` are reserved and cannot be stored as custom tags.

## Inner Selectors

Inner selectors are resolver-only target expressions accepted anywhere bulk
target selectors are accepted through the tag selector path:

- `id:<client_id>` matches the exact VPS client id.
- `name:<display_name>` matches the exact display name. Display names are
  operator labels and can match more than one VPS if reused.
- `tag:<name>` explicitly targets a persisted tag named `<name>`.
- Any other non-empty token is treated as a persisted tag name.

Examples:

```sh
vpsctl bulk-resolve --tags id:edge-01
vpsctl bulk-resolve --tags provider:alpha,country:US
vpsctl job-create --command uptime --tags name:edge-a,role:edge --confirmed
```

`client:<id>` is not an operator target selector. Internal audit and signed
command records may still render scopes as `client:<id>` because those records
identify a concrete VPS after resolution.
