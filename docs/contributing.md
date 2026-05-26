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

**Default to `fix:` (patch bump)** unless the change genuinely introduces
new user-visible functionality (`feat:` → minor) or a breaking change
(`feat!:` / `BREAKING CHANGE:` → major). When in doubt, prefer `fix:` so
release-please proposes a patch release. `docs:`, `refactor:`, `chore:`,
`ci:`, and `test:` do not bump the version at all.

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

## Releasing

Releases are fully automated by
[release-please](https://github.com/googleapis/release-please) — see
[ADR 0008](adr/0008-automated-releases.md) for the full design.

The maintainer never tags or pushes a release by hand. The flow is:

1. PRs land on `main` with **Conventional Commit titles**
   (`feat(core): …`, `fix(pi): …`, `docs: …`). Squash-merge is
   recommended so the squash commit subject becomes the release-please
   input.
2. `release-please.yml` re-runs on every `main` push and maintains a
   single open PR titled `chore: release X.Y.Z`. The PR previews the
   proposed `Cargo.toml` bump and the new `CHANGELOG.md` entries derived
   from the conventional commits since the last release.
3. When you are ready to cut a release, **merge the Release PR**. That
   one click:
   - Tags `vX.Y.Z`,
   - Creates the GitHub Release with auto-generated notes,
   - Triggers `publish.yml` to build the `.deb`s (arm64 + armhf) and the
     macOS tarball, which `softprops/action-gh-release` attaches to the
     same Release,
   - Triggers `publish-apt.yml` to refresh the signed APT repository on
     the `gh-pages` branch.
4. Within ~5 minutes of merging the Release PR, `apt update` on any Pi
   subscribed to the project's APT repository (or any Pi running
   `unattended-upgrades`) sees the new version. See
   [packaging.md](packaging.md) for the Pi-side install/upgrade flow.

### Bump rules

`feat:` → minor (`0.1.0 → 0.2.0`); `fix:` / `perf:` → patch
(`0.1.0 → 0.1.1`); `feat!:` or `BREAKING CHANGE:` footer → major
(`0.1.0 → 1.0.0`). `chore:` / `refactor:` / `test:` / `ci:` do not bump
the version and are hidden from the changelog by default.

Override the next version explicitly by adding a
`Release-As: 1.2.3` footer to any commit (typically the squash commit
message). Use this for the very first release, for retraction-style
re-releases, or to bypass the conventional-commit bump rules.

### Manual release (escape hatch)

`publish.yml` is also `workflow_dispatch`-triggered. To force a build
without going through release-please (e.g. to retry a failed publish
job for an existing tag), run `gh workflow run publish.yml -f tag=vX.Y.Z
-f draft=false`.

### Alternative: tag on every merge

A dormant alternative workflow lives at
`.github/workflows/auto-tag-on-merge.yml.disabled`. Switch by renaming
it and deleting `release-please.yml`. See ADR 0008 for the procedure.

## Adding a HAL adapter

To support a new SBC (Pico, ESP32, an industrial controller, …):

1. Read [`docs/porting/overview.md`](porting/overview.md).
2. Add a new crate under `crates/booth-<name>/`.
3. Implement the HAL traits relevant to your target. Anything you can't
   support (e.g. the operator HTTP client on a tiny no_std MCU) should
   return `Err(NotSupported)` so the runtime can compose around it.
4. Add a porting doc, a CI build matrix entry, and one integration test
   using your adapter.
