# Contributing to vpsman

Thanks for helping improve vpsman. Keep changes focused, reviewable, and
grounded in the operator workflow they affect.

## Issues and Support

Use GitHub issues for reproducible bugs, documentation gaps, and bounded
feature proposals. Search existing issues first and include the vpsman release,
control-plane platform, managed-VPS platform, reproduction steps, expected and
actual behavior, and sanitized logs when relevant.

Community support is best-effort; there is no response or resolution SLA. The
project supports the current stable release, so reproduce an issue there when
possible. Do not post credentials, private keys, tokens, production data, or
exploit details. Follow [SECURITY.md](SECURITY.md) for vulnerabilities.

## Development

Before starting a product update, review the repository's [build
notes](docs/build.md) and the documentation for the affected product area.
Keep schema and protocol changes explicit, validate the affected workflows,
and include release verification and cleanup in the change.

Use the repository-pinned Rust and Node toolchains. The common local gates are:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

cd frontend
npm ci
npm run build
npm audit --audit-level=moderate
```

Run the narrowest relevant smoke tests while iterating. Changes to release
paths, migrations, protocol behavior, or cross-component contracts should also
run the matching checks documented in [docs/build.md](docs/build.md).

## Change Guidelines

- Preserve explicit target review for broad, privileged, destructive, or
  difficult-to-reverse operations.
- Keep desired state, queued work, applied evidence, and current observation
  distinct.
- Prefer explicit product-owned adapter definitions and operator-configurable
  configuration presets over provider-specific command assumptions.
- Add tests for behavior changes and update operator documentation in the same
  pull request.
- Treat current migrations as a canonical fresh-database model until a deployed
  schema is explicitly declared a compatibility boundary. Pin that boundary;
  later compatible changes must be append-only.
- Avoid unrelated formatting or generated-file churn.

Pull requests should explain the operator problem, the chosen behavior, risk
and rollback considerations, and the exact validation performed.

## Licensing

Unless explicitly stated otherwise, contributions intentionally submitted for
inclusion in vpsman are licensed under either
[Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option, without
additional terms.
