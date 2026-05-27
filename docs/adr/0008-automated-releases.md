# ADR 0008 — Automated releases via release-please

**Status:** accepted.

## Context

ADR 0007 makes the APT repository the canonical upgrade path. That only
delivers on its promise if cutting a release is friction-free; otherwise
the Pi-side `apt upgrade` story is gated on the maintainer remembering to
manually tag, run `workflow_dispatch`, and write release notes.

The current state already makes Conventional Commits a strong norm
(`feat(audio): …`, `fix(pi): …`, `docs: …` — see git history), so
deriving the next semver bump and a changelog from commit messages is
straightforward.

We considered three tools:

1. **release-please (Google).** Opens / maintains a single "Release PR"
   that previews the next version + changelog. Merging the PR creates the
   tag and the GitHub Release. Has explicit Rust workspace support.
2. **cargo-smart-release (Byron / gitoxide).** Rust-native; runs from CI
   and produces tags directly per merge. Less popular for workspaces with
   `version.workspace = true`.
3. **Custom shell script.** Maximum control; maximum maintenance.

## Decision

Use **release-please** with the **release-PR model** as the default,
configured for a single-component Rust workspace.

- Workflow: `.github/workflows/release-please.yml`, triggered on
  `push: branches: [main]`. Uses `googleapis/release-please-action` pinned
  by commit SHA.
- Config: `.release-please-config.json`. Single component (`.`) tracking
  the workspace `version`. `include-component-in-tag: false` so tags are
  `vX.Y.Z` not `telephone-booth-vX.Y.Z`. Bumps follow Conventional
  Commits: `feat` → minor, `fix`/`perf` → patch, `feat!` or `BREAKING
  CHANGE:` → major. `bump-minor-pre-major` is true so `feat:` lands as
  `0.x` minors until we cut `1.0.0`.
- **Do not** set `component`, `package-name`, or a
  `pull-request-title-pattern` containing `${component}` / `${version}`
  on the root package. With `separate-pull-requests: false`,
  release-please emits grouped Release PR titles
  (`chore: release main`); declaring a component makes the action
  refuse to tag the merged PR ("untagged, merged release PRs
  outstanding - aborting") and the release silently does not ship. See
  the release-please invariants in `.github/copilot-instructions.md`.
- Manifest: `.release-please-manifest.json` seeded with `0.1.0`.
- Changelog: `CHANGELOG.md` at the repo root. `feat`, `fix`, `perf`,
  `docs`, `build`, `deps`, `revert` are user-visible; `chore`, `refactor`,
  `test`, `ci` are hidden by default (still consumed for version
  decisions).

### How a release happens, end-to-end

1. PRs land on `main` with Conventional Commit titles (squash-merge
   recommended; the squash commit subject becomes the conventional commit).
2. `release-please.yml` re-runs; it opens (or updates) a single PR titled
   `chore: release X.Y.Z`. The PR body shows the proposed bumps and
   `CHANGELOG.md` entries.
3. Maintainer reviews and merges the Release PR.
4. release-please-action observes the merge, creates tag `vX.Y.Z`, and
   creates the GitHub Release with auto-generated notes.
5. release-please-action then `workflow_dispatch`-es `publish.yml` via
   `gh workflow run` with the new tag as input (see Token scope below for
   why we can't just rely on `push: tags`). `publish.yml` builds the
   `.deb`s + macOS tarball and attaches them to the Release that
   release-please just created. `softprops/action-gh-release` updates
   the existing Release rather than failing on the duplicate tag.
6. On successful completion of `publish.yml`, `publish-apt.yml` fires
   via `workflow_run`, regenerates the APT indexes, and pushes
   `gh-pages`.
7. Pis pick up the new version on their next `apt update` (or
   automatically via `unattended-upgrades`).

### Token scope

A vanilla `GITHUB_TOKEN` is sufficient, but the trigger chain has to be
arranged carefully. GitHub Actions' anti-recursion rule says that events
triggered by `GITHUB_TOKEN` do **not** spawn new workflow runs, with two
exceptions: `workflow_dispatch` and `repository_dispatch`. That means a
naive `push: tags: ['v*']` trigger on `publish.yml` would **not** fire
when release-please pushes the tag, because release-please uses
`GITHUB_TOKEN`.

To work around this without introducing a PAT or a GitHub App, the
`release-please.yml` workflow itself dispatches `publish.yml` via
`gh workflow run publish.yml -f tag=vX.Y.Z -f draft=false` after
release-please-action reports `release_created == true`. `publish.yml`
still keeps its `push: tags: ['v*']` trigger so that human-pushed tags
(rare, but useful for hotfixes) also build. `workflow_dispatch` is
allowed under the anti-recursion rule, so the chain works end-to-end
with only `GITHUB_TOKEN` and no special tokens to rotate.

The downstream `publish-apt.yml` is `workflow_run`-triggered, which is
also unaffected by the recursion rule (it observes another workflow's
completion rather than a repository event), so no special handling is
needed there.

If we ever want a Release PR merge to also re-run other branch-watching
workflows (e.g. `ci.yml`), we will need to switch to a GitHub App token
or PAT; see <https://github.com/peter-evans/create-pull-request/blob/main/docs/concepts-guidelines.md#triggering-further-workflow-runs>
for the standard recipe.

## Alternatives considered

- **`cargo-smart-release`.** Powerful, but its workspace-version inference
  fights `version.workspace = true` and there is no preview-PR equivalent
  to release-please's Release PR.
- **`semantic-release` (JS).** Adds a Node toolchain dependency to a Rust
  project for no win.
- **Custom script.** Implementable in ~50 lines but offers nothing over
  release-please beyond avoiding a third-party action. The shipped
  `auto-tag-on-merge.yml.disabled` file is exactly this script, kept as
  an opt-in alternative.
- **Tag-on-every-merge instead of release-PR.** Considered. Pro: zero
  manual step. Con: a noisy day of merges becomes a noisy day of
  releases, each rebuilding `.deb`s and pushing to `gh-pages`. The
  release-PR model batches multiple merges into one release, which
  matches our pace of work. The alternative is shipped as
  `.github/workflows/auto-tag-on-merge.yml.disabled` — rename to enable.

## Consequences

**Good:**

- Releases are a one-click action (merge a PR) with auditable diffs.
- `CHANGELOG.md` is always current.
- Combined with ADR 0007, the maintainer never SSHes into a Pi to upgrade.

**Trade-offs:**

- Adds a third-party GitHub Action to the workflow set. Pinned by commit
  SHA per repo policy; we control rollover.
- Forces strict adherence to Conventional Commits in PR titles (already
  the norm).
- `package.metadata.deb` in `crates/booth-bin/Cargo.toml` references the
  workspace `version` indirectly via `version.workspace = true`; the
  Rust release-type updates the workspace Cargo.toml automatically so
  `cargo deb` always packages the right version.

## Switching to the tag-on-every-merge model

If batching ever becomes painful, switch by:

1. Disable release-please by renaming
   `.github/workflows/release-please.yml` → `.disabled` (and deleting
   `.release-please-config.json` + `.release-please-manifest.json`).
2. Enable the alternative by renaming
   `.github/workflows/auto-tag-on-merge.yml.disabled` →
   `.github/workflows/auto-tag-on-merge.yml`.
3. Document the new flow in `docs/contributing.md`.

The CHANGELOG is not maintained automatically under the alternative; the
GitHub Release notes (auto-generated by `softprops/action-gh-release`)
become the canonical record.

## Initial bootstrap

The seeded manifest is `0.1.0`. The first Release PR after this ADR
merges will propose `0.1.1` (if all commits are `fix`/`docs`) or `0.2.0`
(if a `feat` lands). Use a `Release-As: 0.1.0` footer in a commit message
if you want the very first auto-tag to be `v0.1.0` exactly.
