---
applyTo: "**/*.md"
---

# Markdown / docs instructions

These tighten the workspace-wide rules in `.github/copilot-instructions.md`
for Markdown files. Read that first.

## Linter

`markdownlint-cli2` runs in CI against `docs/**/*.md` and `README.md`. The
shared config lives in `.markdownlint-cli2.yaml`. Lints that fire most often
in this repo:

- **MD040** (`fenced-code-language`) — every triple-backtick code block needs
  a language. If the contents are ASCII art, a directory tree, or shell
  output without a real language, tag it `text` (or `sh`, `console`, `bash`,
  `rust`, `toml`, `yaml`, `mermaid` etc. when applicable).
- **MD032** (`blanks-around-lists`) — lists must have a blank line above and
  below. This bites when a list immediately follows a bold "heading" like
  `**Good:**` — insert a blank line between the bold line and the first
  bullet.
- **MD013** (`line-length`) — wrap prose at 120 characters. Tables and code
  blocks are exempt.
- **MD024** (`no-duplicate-heading`) — siblings only. Reusing a heading
  (e.g. "Authentik") across sibling sections within one file requires
  rewording.
- **MD041** is **off** — the first line does not need to be a top-level
  heading.
- **MD033** — raw HTML is restricted to `<details>`, `<summary>`, `<br>`,
  `<sub>`, `<sup>`. Don't reach for raw `<div>` / `<table>`.

## Links

- Lychee runs in `--offline` mode against `docs/**/*.md` and `README.md`.
  Internal links must resolve to files that actually exist on disk.
- Relative links (`./docs/foo.md`, `../adr/0003-…`) are preferred over
  absolute GitHub URLs for in-repo references — they survive renames if
  the renaming PR updates them.
- When linking to a heading, generate the anchor by lower-casing,
  replacing spaces with `-`, and dropping punctuation
  (`## Why FLAC?` → `#why-flac`).

## Structure

- `docs/README.md` is the index. When you add a new doc, list it there and
  re-run `just docs-index` so the generator's output matches what you wrote.
- ADRs go in `docs/adr/`, numbered sequentially. The template lives in
  `docs/adr/0001-hexagonal-architecture.md` — follow its
  `Context / Decision / Consequences (Good / Trade-offs)` structure.
- The README's "Quickstart" section is sacred. Update it whenever a `just`
  recipe is added or renamed.

## Style

- Sentence case in headings (`## Why FLAC?`, not `## Why Flac`).
- Code identifiers in `backticks` whenever they appear in prose
  (`booth-core`, `GpioPort`, `Effect::Play`).
- Cross-crate references in prose use the crate-name form (`booth-pi`) so a
  search lands on the right place; in rustdoc, use the underscored Rust path
  (`booth_pi`).
- Mermaid diagrams (` ```mermaid ... ``` `) are fine; keep them small and
  resilient — GitHub's renderer is conservative.

## When updating the workspace-wide instructions

If a long-form doc in `docs/` disagrees with anything in
`.github/copilot-instructions.md`, update both in the same PR so future
agents see consistent guidance.
