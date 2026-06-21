# Build Notes

Use the user's profile-managed tools. Do not install build software through
`apt` for this project.

## Rust

The repo pins Rust through `rust-toolchain.toml` and uses rustup-managed targets:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build -p vpsman-agent --target x86_64-unknown-linux-musl
cargo build -p vpsman-agent --target aarch64-unknown-linux-musl
cargo build -p vpsctl --target x86_64-unknown-linux-musl
cargo build -p vpsctl --target aarch64-unknown-linux-musl
cargo build -p vpsman-agent --release --target x86_64-unknown-linux-musl
cargo build -p vpsman-agent --release --target aarch64-unknown-linux-musl
cargo build -p vpsctl --release --target x86_64-unknown-linux-musl
cargo build -p vpsctl --release --target aarch64-unknown-linux-musl
```

Build numbers are component-scoped and self-increment from `1` during local
builds. The build scripts update these checkout-local counter files directly:

- `build/build-numbers/server.txt`
- `build/build-numbers/agent.txt`
- `build/build-numbers/cli.txt`
- `build/build-numbers/frontend.txt`

The server, agent, and CLI numbers are intentionally separate. API, gateway,
and worker share the same server build number through the server build-info
crate. The agent sends its agent build number in `AgentHello`; the gateway
sends the server version and server build number in `ServerHello`;
`vpsctl --version` and CLI User-Agent headers use the CLI build number. Do not
reintroduce a common shared build number or timestamp-based build number for
all components.

GitHub Actions reads the current positive counter values without incrementing
them. Only local builds advance the counters.

`.cargo/config.toml` uses `rust-lld` for musl targets, so final linking does
not require system cross linkers. Native C build scripts still use Cargo's
default tool discovery and require target-named tools on `PATH`:

- `x86_64-linux-musl-gcc`
- `x86_64-linux-musl-ar`
- `aarch64-linux-musl-gcc`
- `aarch64-linux-musl-ar`

Local development can satisfy those names with profile-managed wrappers in
`~/bin` or a real musl-cross distribution. The GitHub Actions CI, Release
Check, and Release Build workflows create temporary wrappers around
runner-provided `clang` and `llvm-ar` before static agent and CLI builds.

Generate development Noise keypairs with:

```sh
cargo run -p vpsctl -- noise-keygen
```

## Frontend

The noninteractive login shell may not expose Node. Use the interactive shell
path configured by the user's profile/NVM:

```sh
cd frontend
bash -ic 'npm install'
bash -ic 'npm run build'
bash -ic 'npm audit --audit-level=moderate'
```

`npm run build` runs `../build/frontend-build-number.mjs` before `tsc` and
`vite build`. Local builds increment `build/build-numbers/frontend.txt`;
`GITHUB_ACTIONS=true` reads the stored frontend counter without incrementing.
