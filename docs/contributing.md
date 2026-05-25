# Contributing

Thanks for hacking on the booth! A few conventions:

> AI-agent conventions (Copilot etc.) live in
> [`.github/copilot-instructions.md`](../.github/copilot-instructions.md)
> and the path-scoped files under [`.github/instructions/`](../.github/instructions/).
> Humans and agents should follow the same rules.

## Branches

- `main` is the **legacy Node.js** code (kept on tag `legacy-node` for
  history; will eventually be replaced by the Rust client).
- `rust-client` is the active branch for the new Rust client.
- Feature branches: `<github-username>/<short-topic>` —
  e.g. `djensenius/audio-meter-tweaks`.

## Commits

Conventional Commits are preferred but not strictly enforced:

```text
feat(core): allow rotary gate inversion via config
fix(pi): handle USB-audio device disappearing mid-recording
docs(authentik): clarify required groups claim path
```

## Before pushing

```sh
just check          # fmt + clippy -D warnings + tests
just docs-check     # markdownlint + lychee
```

CI runs the same commands plus `cargo doc -D warnings`, a cross-compile
matrix, and `cargo-deny` / `cargo-audit`.

## Before merging

- Wait for the Copilot PR review to complete.
- Address every actionable Copilot or human review comment before merging. If a
  comment is a false positive, reply with the reason and resolve the thread.
- Wait for all required CI jobs to pass.

## Style

- `unsafe_code` is **denied** workspace-wide.
- Public items in `booth-core` and `booth-hal` must have rustdoc.
- New external dependencies need a one-line justification in the PR
  description and a license that's on the cargo-deny allow-list in
  [`deny.toml`](../deny.toml).
- Significant architecture changes get an ADR in `docs/adr/`.

## Adding a HAL adapter

To support a new SBC (Pico, ESP32, an industrial controller, …):

1. Read [`docs/porting/overview.md`](porting/overview.md).
2. Add a new crate under `crates/booth-<name>/`.
3. Implement the HAL traits relevant to your target. Anything you can't
   support (e.g. the operator HTTP client on a tiny no_std MCU) should
   return `Err(NotSupported)` so the runtime can compose around it.
4. Add a porting doc, a CI build matrix entry, and one integration test
   using your adapter.
