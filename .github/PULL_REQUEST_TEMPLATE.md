<!--
Reminders before opening this PR:

- Do NOT include a `Co-authored-by: Copilot ...` trailer in commits.
- Branch name follows `<github-username>/<topic>` for personal work or
  `feat|fix|chore|docs/<topic>` for shared branches.
- Conventional Commit style for commit subjects is preferred. Default to
  `fix:` (patch bump) unless the change is genuinely a `feat:` (new
  user-visible functionality → minor) or breaking (`feat!:` /
  `BREAKING CHANGE:` → major).
- See `.github/copilot-instructions.md` for the full conventions, including
  the "ship it" workflow.
-->

## Summary

<!-- What does this change do, and *why*? The diff already shows the what. -->

## How was this tested?

<!-- Which `just` recipe(s) did you run, on which OS? -->

- [ ] `just check` passed locally on …
- [ ] `just docs-check` passed locally
- [ ] Manual smoke test, if applicable (`just dev` / `just tui` / on-Pi)

## New external dependencies

<!--
List any new crates pulled in by this PR, their license (must be on
`deny.toml`'s allow-list), and a one-line justification. Delete this
section if there are none.
-->

- _(none)_

## Feature flags / config changes

<!--
Mention any new or renamed Cargo features, env vars, or config keys.
Delete this section if there are none.
-->

- _(none)_

## ADRs

<!--
Significant architecture changes ship with an ADR in `docs/adr/`.
Link it here, or explain why one isn't needed.
-->

## Checklist

- [ ] Commits do **not** include a `Co-authored-by: Copilot` trailer.
- [ ] Copilot PR review completed and all actionable feedback is addressed.
- [ ] Public items in `booth-core` / `booth-hal` have rustdoc.
- [ ] Tests added or updated alongside behavior changes.
- [ ] `docs/` updated if user-facing behavior changed.
- [ ] All CI jobs are expected to pass (clippy, tests on Linux + macOS,
      cross-build for aarch64 + armv7 Linux, rustdoc + markdownlint + lychee).
