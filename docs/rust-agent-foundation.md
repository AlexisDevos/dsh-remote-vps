# Rust agent foundation

## Scope

This first vertical slice establishes the versioned Protobuf contract and a real
Rust gRPC health path. It deliberately listens on loopback only; it is not yet
the production VPS daemon.

```text
DeepSeek Harness plugin (TypeScript)
              |
              | future local Unix-socket controller
              v
     dsh-controller (Rust)
              |
              | future mTLS/Tailscale or SSH Unix-socket tunnel
              v
       dsh-agentd (Rust)
              |
              +-- Health / Capabilities (implemented in this slice)
              +-- Files / Search / Exec (next slices)
```

## Decisions

- Rust is used for the controller and the VPS agent.
- `tonic` + `prost` define the gRPC/Protobuf boundary.
- `protoc-bin-vendored` keeps builds independent from a system `protoc`.
- The protocol is kept in `proto/` and generated in `dsh-protocol`.
- The TypeScript plugin remains the DSH integration layer only.

## Local run

Terminal 1:

```bash
cargo run -p dsh-agentd -- serve --listen 127.0.0.1:7443
```

Terminal 2:

```bash
cargo run -p dsh-controller -- health --endpoint http://127.0.0.1:7443
```

The production design will replace the loopback endpoint with a private Unix
socket locally and mTLS over Tailscale, with a persistent SSH tunnel as the
fallback transport.

## Quality gates

Before adding file operations:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace --all-features`
- integration tests against a real agent process
- Protobuf compatibility checks
- resource limits and cancellation tests
