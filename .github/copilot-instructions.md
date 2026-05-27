# Copilot instructions for Telephone-Booth

These instructions apply to **all** code, docs, and PRs in this repository.
They encode the conventions and constraints that already live in the codebase
so an AI agent (or any new contributor) can act consistently without
re-deriving them from scratch.

If a section here disagrees with a long-form doc in `docs/`, the long-form doc
wins — but please update this file in the same PR so the two stay in sync.

## Highest-priority rules

1. **Do not add a `Co-authored-by: Copilot …` trailer to commits or PRs.**
   The project owner has explicitly opted out. If a default template adds it,
   strip it before committing.
2. **Keep `booth-core` pure.** No I/O, no clock reads, no allocation of OS
   resources, no `tokio`, no `std::time::Instant`. The core is a function
   `handle(State, Event) -> (State, Vec<Effect>)`. Effects are data; the
   runtime in `booth-bin` executes them via HAL traits.
3. **`unsafe_code` is forbidden workspace-wide** (`Cargo.toml` enforces this
   via `unsafe_code = "forbid"`). Do not add `#![allow(unsafe_code)]` to a
   crate. If you genuinely need `unsafe`, write an ADR first.
4. **`just check` must pass locally before pushing.** That command runs
   `cargo fmt --check`, `cargo clippy --workspace --all-targets --all-features
   -- -D warnings`, the test suite, and `markdownlint` + `lychee`. CI runs the
   same set plus cross-compile and rustdoc, so this catches >90 % of CI
   failures up front.

## Repository shape

| Crate            | Role                                                                 |
| ---------------- | -------------------------------------------------------------------- |
| `booth-core`     | Pure state machine. `no_std + alloc`-friendly. No I/O, no clock.     |
| `booth-hal`      | Trait definitions (`GpioPort`, `AudioSink/Source`, `OperatorClient`, `Clock`, `Storage`) and shared value types. |
| `booth-telemetry`| Telemetry bus types shared by adapters and the debug surface.        |
| `booth-mock`     | In-memory HAL adapters for tests and host runs.                      |
| `booth-debug`    | Embedded `axum` HTTPS server + htmx UI for live debugging.           |
| `booth-pi`       | Pi adapter: `rppal` GPIO (Linux-only), `cpal` audio (cross-platform), `reqwest` HTTP. Split into `audio`, `operator`, and `pi` features. |
| `booth-bin`      | Binary glue: tokio runtime, config loader, signal handling, the `--simulator` TUI. |

Adapters for a new SBC (e.g. ESP32, Pico) go in their own `crates/booth-<name>/`
crate and implement just the HAL traits they can support; everything else
returns `Err(NotSupported)`. See `docs/porting/overview.md`.

## Architecture rules

- **Hexagonal / ports-and-adapters.** Adapters depend on `booth-hal`; the
  binary composes them. The core never imports an adapter and never imports
  `booth-hal` either (it only deals with `Effect` data).
- **Effects must be fully describable as data.** If the core needs to do
  something new, add a new `Effect` variant — do not call out to a HAL trait
  from inside `handle`.
- **One state machine, one transition function.** Don't fork the state into
  multiple machines. If a new mode is needed, add a new `State` variant.
- **Significant architecture changes ship with an ADR** in `docs/adr/`,
  numbered sequentially, following the format of the existing five.
- **All public items in `booth-core` and `booth-hal` must have rustdoc.**
  `missing_docs` is `warn` workspace-wide; treat the warning as an error in
  these two crates.

## Cross-platform gating

The dev machine is macOS; the production target is Raspberry Pi OS (Debian).
CI exercises both. Any new code that pulls in OS-specific functionality must
keep both compilable.

- **`rppal` (GPIO) is Linux-only.** Gate real GPIO code with
  `#[cfg(all(feature = "pi", target_os = "linux"))]` and provide a stub
  implementation behind `#[cfg(not(all(feature = "pi", target_os = "linux")))]`
  so the crate still type-checks on macOS.
- **`cpal` audio works on macOS and Linux.** No target gating is needed for
  the audio adapter itself, but its build-time deps (e.g. `alsa-sys`) require
  `libasound2-dev` on Linux — CI installs it via `apt-get` for native jobs
  and via `Cross.toml` `pre-build` for cross-compile jobs.
- **Feature split inside `booth-pi`**: `audio` (cpal + FLAC), `operator`
  (`reqwest` HTTP client), `pi` (= `audio + operator + rppal`). Pick the
  narrowest feature when adding `#[cfg(feature = …)]` gates so the simulator
  on macOS can still pull in audio + operator without rppal.

## Rust style

- Pinned to **Rust 1.95.0** via `rust-toolchain.toml` and `mise.toml`.
  Don't bump this casually; if you must, update the ADR (`docs/adr/0005-…`)
  in the same PR.
- Edition 2024.
- **Lints**: `pedantic` and `nursery` are warn-level workspace-wide. CI runs
  `clippy -D warnings`. Fix lints; reach for `#[allow(clippy::…)]` only with
  a short comment justifying the suppression.
- **`unwrap_used` / `expect_used` are warn-level.** Tests and one-shot CLI
  helpers may unwrap. Long-running runtime code may not — propagate with `?`
  or handle the error.
- **Use `thiserror` for library error enums** and `anyhow` only in binaries
  / integration tests.
- **Logging**: use `tracing` macros (`info!`, `warn!`, `error!`, …). Don't
  add `println!` / `eprintln!` outside of bootstrap before the subscriber is
  installed, or inside the simulator TUI where stdout is the framebuffer.
- **Async**: spawn via `tokio::spawn` (or `tokio::task::spawn_blocking` for
  blocking work). Don't mix `tokio` and `async-std`. Adapter trait impls that
  hold non-`Send` resources need `+ Sync` bounds on the trait helpers
  (`booth-pi/src/operator.rs` has worked examples).
- **No new external dependency without a one-line justification in the PR
  description** plus a license that is already on the `deny.toml` allow-list
  (or a deliberate update to that allow-list).
- Public APIs that take a `String` should usually take `impl Into<String>` or
  `&str`. Public APIs that return a `Vec<…>` from `no_std` code in
  `booth-core` should use `alloc::vec::Vec` explicitly.
- Prefer `&str` arguments and `Cow<'_, str>` for owned-or-borrowed return
  types in `booth-core`.

## Testing

- **`proptest`** for property tests over the state machine — every legal
  event sequence should leave the booth in a known-good state, and a random
  walk should never panic.
- **`insta`** for snapshot tests of telemetry payloads, debug-API JSON, and
  state transitions. Run `cargo insta review` before committing snapshot
  changes.
- **`booth-mock`** is the integration-test fabric: every end-to-end scenario
  in `crates/booth-bin/tests/` (and similar) wires mock adapters into the
  real runtime, never the Pi adapter.
- **No test may require network access or hardware.** Mock the operator HTTP
  client with `wiremock` or a stub `OperatorClient` impl.
- Use `cargo nextest run --workspace --all-features` (preferred) or
  `cargo test --workspace --all-features` if nextest is unavailable.

## Commits & branches

- Conventional Commits are preferred, e.g. `feat(core): …`,
  `fix(pi): …`, `docs(adr): …`. Not strictly enforced, but it shapes the
  changelog and CI commit-status messages.
- **Default to `fix:` (patch bump)** for commit and PR titles unless the
  change genuinely adds user-visible new functionality (`feat:` → minor) or
  introduces a breaking change (`feat!:` / `BREAKING CHANGE:` → major). When
  in doubt, prefer `fix:` so release-please proposes a patch release. Pure
  documentation, refactors, CI, or chore work should still use their
  conventional prefixes (`docs:`, `refactor:`, `ci:`, `chore:`), which do not
  bump the version at all.
- Branch names: `<github-username>/<short-topic>` for personal work and
  `feat/<topic>`, `fix/<topic>`, `chore/<topic>`, `docs/<topic>` for shared
  branches.
- **No `Co-authored-by: Copilot` trailers.** See rule 1 above.
- Squash-merge into `main` unless the PR represents multiple cohesive
  commits that are worth preserving (rare).

## Pull requests

Every PR should:

1. Include a summary of the *why*, not just the *what*. A diff already shows
   the what.
2. List any new external dependencies and their license + reason for being
   added.
3. Note any new or modified feature flags.
4. Confirm that `just check` passed locally (mention which platform — macOS
   vs Linux — since some lints only fire on one).
5. Wait for the Copilot PR review to complete before merging. Address every
   actionable comment from the Copilot reviewer (or any human reviewer). If a
   comment is a false positive, reply with the reason and resolve the thread.
6. Wait for all CI jobs to pass:
   - `rustfmt`, `clippy`
   - `test (ubuntu-latest)`, `test (macos-latest)`
   - `build (aarch64-apple-darwin)`
   - `cross-build (aarch64-unknown-linux-gnu)`, `cross-build (armv7-unknown-linux-gnueabihf)`
   - `rustdoc + docs lint` (= `cargo doc -D warnings` + markdownlint + lychee)

If a CI failure is pre-existing on `main` (not caused by your PR), fix it in
the same PR rather than disabling the check — the project policy is to keep
`main` green at all times.

## "Ship it" workflow

When the maintainer says **"ship it"** (or an equivalent shorthand) on a PR
or after a change has been pushed, treat that as a single instruction to
drive the change all the way to a released version. Concretely:

1. Open the PR if one isn't already open. Default the title to a `fix:`
   prefix (patch bump) unless the change is clearly a `feat:` or breaking
   change. Do **not** add a `Co-authored-by: Copilot` trailer.
2. Wait for **all** required CI jobs to finish (`rustfmt`, `clippy`,
   `test (ubuntu-latest)`, `test (macos-latest)`,
   `build (aarch64-apple-darwin)`,
   `cross-build (aarch64-unknown-linux-gnu)`,
   `cross-build (armv7-unknown-linux-gnueabihf)`,
   `rustdoc + docs lint`). If any fails, fix the root cause (don't disable
   the check) and push again.
3. Wait for the Copilot PR review. Address every actionable comment from
   Copilot or any human reviewer. For false positives, reply with the
   reason and resolve the thread. Re-request review if the reviewer
   requested changes.
4. Once CI is green and review feedback is addressed, **squash-merge** the
   PR into `main`.
5. `release-please.yml` will (re)open or update the Release PR on the next
   `main` push. Wait for that Release PR to settle, then **merge the
   Release PR** to cut the new tag + GitHub Release. By default this will
   be a patch bump because of the `fix:` default above.
6. After the Release PR merges, watch `publish.yml` and `publish-apt.yml`
   to confirm the `.deb`s and APT repository update succeed. If either
   fails, re-dispatch it (`gh workflow run publish.yml -f tag=vX.Y.Z
   -f draft=false`) or open a follow-up fix PR.

Only consider "ship it" done once steps 1-6 are complete — not when the
feature PR merges.

### release-please invariants (do not break these)

The release pipeline is brittle because release-please silently no-ops
when its config and the Release PR title disagree. Past regressions
(notably the missing v0.3.1 cut) traced back to violating one of these:

- **Never add `component` or `package-name` to the single root package
  in `.release-please-config.json`** while `separate-pull-requests` is
  `false`. With grouped manifest PRs the title is
  `chore: release main` (no component, no version). If the package
  declares a component, release-please parses the merged PR's
  component as `undefined`, doesn't match the configured value, logs
  `⚠ There are untagged, merged release PRs outstanding - aborting`,
  exits 0, and **no tag is created**. The downstream
  `Dispatch publish workflow` step is skipped silently — CI shows
  green but nothing ships.
- **Never set a `pull-request-title-pattern` that references
  `${component}` or `${version}`** for this repo. The grouped-PR title
  ignores per-package patterns; warnings like
  `⚠ pullRequestTitlePattern miss the part of '${component}'` are a
  symptom that someone added one back. If you genuinely need to
  customise the grouped title, use `group-pull-request-title-pattern`
  (default: `chore: release ${branch}`).
- **Keep the `# x-release-please-version` marker on the workspace
  `version = "..."` line in the root `Cargo.toml`.** Without
  `package-name` set, release-please updates `extra-files` via that
  generic marker. Removing the marker silently stops version bumps in
  `Cargo.toml`.
- **`release-type` stays `simple` per ADR 0008.** Switching to `rust`
  is a separate, deliberate change with its own ADR — do not bundle it
  with an unrelated fix.
- **Confirm a release actually shipped before declaring "ship it"
  done.** A successful `release-please` workflow run is **not**
  sufficient evidence. Check `gh release list` for the new tag and
  watch `publish.yml` + `publish-apt.yml` complete. If the
  release-please run logs include `untagged, merged release PRs
  outstanding - aborting`, the release did **not** happen — recover
  by either (a) merging a config fix and letting the next push
  re-trigger tagging, or (b) manually creating the tag/release on the
  merged Release PR's squash commit and dispatching `publish.yml`
  yourself, then relabel the stuck Release PR from
  `autorelease: pending` to `autorelease: tagged` so future runs
  don't keep aborting.

## Documentation

- Markdown is linted with `markdownlint-cli2` per `.markdownlint-cli2.yaml`.
  Key rules that bite often:
  - **MD040**: fenced code blocks need a language. Use `text` for ASCII art
    or tree diagrams that don't have a real language.
  - **MD032**: lists need a blank line above and below — including when the
    preceding line is a bold "heading" like `**Good:**`.
  - **MD013**: line length 120 in prose; relaxed inside tables and code.
- Rustdoc is built with `-D warnings`. Intra-doc links must resolve. When
  referencing a type that lives in a different crate, use the fully-qualified
  path (`booth_core::Effect`) rather than `crate::Effect`.
- `docs/README.md` is the index. `just docs-index` rebuilds it; CI fails if
  it's out of date.
- Lychee runs in `--offline` mode — relative links must point to files that
  actually exist on disk.

## Security & secrets

- Never commit tokens, certs, or `.env` values. `.gitignore` already excludes
  `*.debug-token`, `*.debug-cert.*`, and `.env*` (except `.env.example`).
- Bearer-token authentication applies equally to the Tailscale-served HTTPS
  listener and the LAN self-signed listener (`booth-debug`). Don't add a
  code path that skips auth on one and not the other.
- Use `subtle::ConstantTimeEq` (already in workspace deps) for comparing
  secrets / tokens — never `==`.

## Audio & hardware specifics

- Recording format is **FLAC** (`flacenc` for encode, `claxon` /
  `symphonia` for decode). Don't add MP3 / Opus / WAV pipelines without an
  ADR. See ADR 0003.
- Rotary pulses: 10 pulses = digit 0. More than 10 in a single group resets
  to `DialTone`. Pulse-group timeout is 350 ms (`PULSE_GROUP_TIMEOUT_MS`).
- Digit 1 → random question. Digit 2 → random message. Digits 0, 3–9 → the
  operator-recorded `Instructions` clip. Keep these mappings stable; UX is
  documented to operators.

## Tooling cheat sheet

| Want to…                                  | Run                                                                |
| ----------------------------------------- | ------------------------------------------------------------------ |
| Set up the toolchain                      | `mise install`                                                     |
| Full local check (what CI runs)           | `just check`                                                       |
| Format Rust code                          | `cargo fmt --all` (or `just fmt`)                                  |
| Lint Rust                                 | `cargo clippy --workspace --all-targets --all-features -- -D warnings` |
| Run tests                                 | `cargo nextest run --workspace --all-features`                     |
| Run the simulator TUI                     | `just tui`                                                         |
| Run on real Pi hardware                   | `just run-pi`                                                      |
| Cross-compile for Pi 4 / 5                | `just cross-build aarch64-unknown-linux-gnu`                       |
| Cross-compile for Pi 3 / Zero 2           | `just cross-build armv7-unknown-linux-gnueabihf`                   |
| Build the `.deb`                          | `just deb`                                                         |
| Build & lint rustdoc                      | `just docs`                                                        |
| Lint markdown + check links               | `just docs-check`                                                  |
| Audit deps (licenses + advisories)        | `just audit`                                                       |

## When in doubt

- Read the relevant ADR in `docs/adr/`.
- Read `docs/architecture.md` for the big picture.
- Read `docs/contributing.md` for the human-facing version of these rules.
- Open a draft PR early and let CI tell you what you missed.
