# Raspberry Pi extras — installing your preferred CLI tools

These are personal quality-of-life command-line tools for a Raspberry Pi. They
are **not** required by the Telephone Booth client — the booth service runs fine
without any of them. This page collects the best way to install the latest
version of each.

> **Assumes 64-bit Raspberry Pi OS (`arm64`)**, which is what
> [the setup guide](raspberry-pi-setup.md) provisions (Pi 4 / 5, Raspberry Pi
> OS Lite 64-bit). Several of these projects publish `aarch64` Linux binaries
> only, so on a 32-bit `armhf` install the mise steps below will fail — use
> apt there instead.

There are two install routes:

- **[mise](https://mise.jdx.dev)** — the default for these tools. One manifest
  keeps everything current, and a ready-made config lives at
  [`packaging/raspberry-pi/mise.toml`](../packaging/raspberry-pi/mise.toml).
- **apt** — for the handful of tools mise cannot install, and where you would
  rather updates ride along with `apt upgrade`.

## Summary

| Tool | Recommended install | apt alternative |
| --- | --- | --- |
| `fish` | mise (`aqua:fish-shell/fish-shell`) | Raspberry Pi OS, but far older |
| `mise` | apt repo below | official mise apt repo |
| `tmux` | mise (`tmux@latest`) | Raspberry Pi OS (Debian) |
| `bat` | mise (`bat@latest`) | Raspberry Pi OS, as `batcat` |
| `eza` | mise (`eza@latest`) | Pi OS 13+, or `deb.gierens.de` |
| `neovim` | mise | Debian's build, but it is stale |
| `starship` | mise | none |
| `atuin` | mise | none |
| `zellij` | mise | none |
| `bottom` (`btm`) | mise | `.deb` on GitHub releases |
| `git-delta` | mise | `.deb` on GitHub releases |
| `herdr` | mise | none |

## Install mise first

[mise](https://mise.jdx.dev) publishes its own apt repository:

```sh
sudo apt update
sudo apt install -y gpg wget
sudo install -dm 755 /etc/apt/keyrings
wget -qO - https://mise.jdx.dev/gpg-key.pub \
  | gpg --dearmor \
  | sudo tee /etc/apt/keyrings/mise-archive-keyring.gpg 1>/dev/null
echo "deb [signed-by=/etc/apt/keyrings/mise-archive-keyring.gpg arch=arm64] https://mise.jdx.dev/deb stable main" \
  | sudo tee /etc/apt/sources.list.d/mise.list
sudo apt update
sudo apt install -y mise
```

Then activate it in your shell (fish shown; see the mise docs for bash/zsh):

```sh
echo 'mise activate fish | source' >> ~/.config/fish/config.fish
```

## Install the tools

Copy the ready-made manifest and let mise do the rest:

```sh
mkdir -p ~/.config/mise
cp packaging/raspberry-pi/mise.toml ~/.config/mise/config.toml
mise trust ~/.config/mise/config.toml
mise install
```

That installs `fish`, `tmux`, `bat`, `eza`, `neovim`, `starship`, `atuin`,
`zellij`, `bottom`, `git-delta`, and `herdr` onto your `PATH` as prebuilt
`aarch64` binaries — no compiling. `mise upgrade` later bumps them to the
newest releases.

Or install them ad hoc:

```sh
mise use -g tmux@latest bat@latest eza@latest neovim@latest starship@latest \
  atuin@latest zellij@latest bottom@latest delta@latest herdr@latest \
  "aqua:fish-shell/fish-shell@latest"
```

> The Pi manifest is intentionally separate from the repo-root
> [`mise.toml`](../mise.toml), which pins the Rust toolchain and build tooling
> the booth itself needs.

## fish

`fish` has no short registry name in mise, so it is referenced by its aqua
backend, `aqua:fish-shell/fish-shell`. It is worth the extra typing: upstream
ships a self-contained static `aarch64` binary from 4.0 onward, so mise gets
you 4.8.x where Raspberry Pi OS 13 packages 4.0.2 (and Pi OS 12 packages
3.6.0, before the Rust rewrite).

The manifest above already includes it. Standalone:

```sh
mise use -g "aqua:fish-shell/fish-shell@latest"
```

### Making it your login shell

A mise-managed fish lives under your home directory rather than `/usr/bin`,
so `chsh` needs the full path, and that path must be listed in `/etc/shells`:

```sh
fish_path="$(mise which fish)"
echo "$fish_path" | sudo tee -a /etc/shells
chsh -s "$fish_path"
```

The path stays stable across `mise upgrade` because the version is pinned to
`latest` rather than a number.

> **Verify before you log out.** Your login shell now depends on mise, so
> removing mise or running `mise prune` would leave you without a working
> shell — awkward on a headless Pi, because a broken login shell also breaks
> `ssh host <command>`. Open a second SSH session and confirm it works before
> closing the first. If you would rather not couple the two, `sudo apt install
> fish` gives you an older fish at `/usr/bin/fish` that nothing can take away.

## apt alternatives

You do not need any of these if you used the mise manifest above — they are
here for when you would rather have apt manage a tool.

### fish, tmux, and bat

```sh
sudo apt install -y fish tmux bat
```

Debian's `fish` is well behind upstream — 4.0.2 on Pi OS 13, 3.6.0 on Pi OS 12
— but it installs to `/usr/bin/fish` and is independent of mise.

Debian installs the `bat` binary as `batcat` (name collision). Add a shim if
you want the upstream command name:

```sh
mkdir -p ~/.local/bin
ln -sf "$(command -v batcat)" ~/.local/bin/bat
```

### eza

Raspberry Pi OS 13 (trixie) and newer ship `eza` directly, though it lags
upstream:

```sh
sudo apt install -y eza
```

On older releases, use `deb.gierens.de` — the Debian route documented in eza's
own [`INSTALL.md`](https://github.com/eza-community/eza/blob/main/INSTALL.md),
run by an eza maintainer with its signing key served from the upstream
repository:

```sh
sudo apt update
sudo apt install -y gpg
sudo mkdir -p /etc/apt/keyrings
wget -qO- https://raw.githubusercontent.com/eza-community/eza/main/deb.asc \
  | sudo gpg --dearmor -o /etc/apt/keyrings/gierens.gpg
echo "deb [arch=arm64 signed-by=/etc/apt/keyrings/gierens.gpg] https://deb.gierens.de stable main" \
  | sudo tee /etc/apt/sources.list.d/gierens.list
sudo chmod 644 /etc/apt/keyrings/gierens.gpg /etc/apt/sources.list.d/gierens.list
sudo apt update
sudo apt install -y eza
```

### neovim

Debian's `neovim` package installs with `apt install -y neovim`, but it lags a
long way behind upstream. Note that upstream publishes no Linux `.deb` at all —
its release assets are AppImages and tarballs — so mise is the practical way to
run a current Neovim.

### bottom and git-delta

Both publish an `arm64` `.deb` on their GitHub releases pages. There is no apt
repo, so you would be updating these by hand:

```sh
# Example: bottom (replace VERSION)
wget https://github.com/ClementTsang/bottom/releases/download/VERSION/bottom_VERSION-1_arm64.deb
sudo dpkg -i bottom_VERSION-1_arm64.deb

# Example: git-delta
wget https://github.com/dandavison/delta/releases/download/VERSION/git-delta_VERSION_arm64.deb
sudo dpkg -i git-delta_VERSION_arm64.deb
```
