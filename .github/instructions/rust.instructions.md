---
applyTo: "**/*.rs,**/Cargo.toml"
---

# Rust-specific instructions

These tighten the workspace-wide rules in `.github/copilot-instructions.md`
for `.rs` and `Cargo.toml` files. Read that file first.

## Hard requirements

- **No `unsafe`.** `unsafe_code` is `forbid` at the workspace level. If you
  truly need it, write an ADR before opening the PR.
- **No `unwrap()` / `expect()` in long-running runtime code.** They're warn-
  level lints (`clippy::unwrap_used`, `clippy::expect_used`). Acceptable in
  tests, `#[test]`-style examples, and one-shot CLI bootstrap. Anywhere else,
  propagate the error with `?` or handle it explicitly.
- **No `println!` / `eprintln!`** for runtime logging. Use `tracing::info!`
  / `warn!` / `error!` / `debug!` / `trace!`. The TUI simulator owns
  stdout — emitting to stdout from inside it will corrupt the framebuffer.
- **No `std::thread::spawn`** for ongoing work. Use `tokio::spawn` (async)
  or `tokio::task::spawn_blocking` (sync blocking work that yields back).
- **No new HTTP/TLS stack.** Use the workspace deps: `reqwest` (client),
  `axum` (server), `tokio-rustls` (TLS), `rustls-pemfile`. Don't pull in
  `hyper-tls` / `native-tls` / `openssl`.

## `booth-core` (extra constraints)

- `no_std + alloc`-friendly. Don't import `std::*` — use `core::*` and
  `alloc::*`.
- No `tokio`, no `std::time::*`, no `std::fs::*`, no random number
  generators, no allocations of OS resources.
- The transition function is `pub fn handle(state: State, event: Event) ->
  (State, Vec<Effect>)`. Don't add a second entry point with side effects.
- Every legal `(State, Event)` pair must be exhaustively matched. A
  `_ => unreachable!()` arm is a smell — be explicit.
- New `State` / `Event` / `Effect` variants must be `Serialize`-able for
  telemetry replay.

## `booth-hal` (extra constraints)

- Traits only, plus the value types they reference. No concrete adapters.
- Trait methods that perform I/O should return a boxed future
  (`Pin<Box<dyn Future<...> + Send + 'static>>`) via `async_trait` — we
  accept the heap cost because it lets adapters hold non-`Send` resources
  behind their own runtime.
- All trait items have rustdoc. The HAL is a published API surface, even if
  it only ships with our binary today.

## Feature gating

- Use the narrowest feature for `#[cfg(feature = "…")]`. In `booth-pi` that
  means `audio`, `operator`, or both — not `pi` unless you need rppal.
- Target gating for OS-specific code: `#[cfg(all(feature = "pi",
  target_os = "linux"))]` for real GPIO; a stub `#[cfg(not(all(…)))]`
  module must keep the same public type signatures so callers don't change.
- Don't push `target_os` checks into `Cargo.toml` `[target."cfg(...)"]
  .dependencies` unless the dep itself fails to compile on the other OS
  (e.g. `rppal`).

## Error handling

- Library crates: define a `pub enum FooError` with `#[derive(thiserror::Error,
  Debug)]`. Variants should carry context, not just an opaque string.
- Binaries / tests: `anyhow::Result<()>` with `.context("doing X")` for the
  layer that converts to user-facing errors.
- HAL-trait methods return their crate's typed error, not `anyhow::Error`.

## Async

- Public trait methods that are async use `async fn` (stable in 2024 edition
  for traits, but be aware of the `+ Send` capture rules).
- Use `+ Sync` bounds on helpers that move `Self: ?Sized + Sync` impls
  across await points — see `crates/booth-pi/src/operator.rs` for worked
  examples.
- Cancellation: prefer `tokio::select!` with a shutdown signal over ad-hoc
  `Arc<AtomicBool>` flags.

## Testing

- Use `proptest!` for state-machine properties and `insta::assert_yaml_snapshot!`
  for snapshot tests. Snapshots live next to the test file in `snapshots/`.
- Integration tests in `crates/<crate>/tests/` should mock everything via
  `booth-mock` — no `tokio::net::TcpListener::bind("0.0.0.0:0")` and no
  hitting `api.example.com`.
- Use `tokio-test` for time manipulation when testing timeout behavior.

## Cargo.toml

- New deps go into `[workspace.dependencies]` in the root `Cargo.toml` and
  are referenced as `dep.workspace = true` (or
  `dep = { workspace = true, features = [...] }`) in the consuming crate.
  This keeps versions in lockstep.
- Don't enable `default-features` on `reqwest` — we want `rustls-tls`, not
  `native-tls`. The workspace dep already sets `default-features = false`.
- License of any new dep must already be on the `deny.toml` allow-list, or
  the PR must explicitly add it with a justification.

## Documentation comments

- `//!` module-level doc on every public module.
- `///` item-level doc on every `pub` item in `booth-core` and `booth-hal`.
- Intra-doc links to items in other crates use the absolute crate path
  (e.g. `[`Effect`](booth_core::Effect)`), not `crate::Effect`.
- Items defined only in the binary (e.g. `install_simulator_tracing` in
  `booth-bin/src/main.rs`) are not reachable from library rustdoc — refer
  to them by plain name, not as an intra-doc link.
