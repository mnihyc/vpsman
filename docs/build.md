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
cargo build -p vpsman-agent --release --target x86_64-unknown-linux-musl
cargo build -p vpsman-agent --release --target aarch64-unknown-linux-musl
```

`.cargo/config.toml` uses `rust-lld` for musl targets so static agent builds do
not require system cross linkers.

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
