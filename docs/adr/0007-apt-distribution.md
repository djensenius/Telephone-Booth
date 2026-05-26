# ADR 0007 — APT repository on GitHub Pages

**Status:** accepted.

## Context

Pi-side upgrades have always been a manual two-step:

1. SSH into each booth.
2. `wget`/`scp` the new `.deb` from a GitHub Release, then
   `sudo apt install ./telephone-booth_<v>_arm64.deb`.

This works but does not scale across more than a couple of booths and
makes it impossible to enable unattended upgrades. We want
`sudo apt upgrade telephone-booth` (or even no human step at all) to be the
upgrade path.

We considered three shapes:

1. **`cargo install booth-bin`.** Compile-on-Pi. Requires publishing eight
   workspace crates to crates.io with synchronised versions, dropping
   `rppal` (Linux-only) into platform-conditional features, and accepting
   20–40 minute build times on a Pi 3 / Zero 2. Loses every artefact
   `cargo-deb` already ships (systemd units, sysusers, tmpfiles, conffiles,
   maintainer scripts). Rejected.
2. **Third-party hosted APT repo (Cloudsmith / packagecloud).** Zero infra
   on our side; they handle signing and metadata. Free OSS tier exists.
   Rejected because it adds another account to manage, and our needs are
   modest.
3. **Static APT repo on GitHub Pages.** Sign packages with our own GPG key,
   publish the generated `Packages` / `Release` / `InRelease` files to
   `gh-pages`, point `apt` at it. Free, no third-party dependency, uses the
   same GitHub Release storage we already populate.

## Decision

We adopt option 3: a static, signed APT repository served from the
`gh-pages` branch of this repository, with a single `stable` suite.

### Layout

```text
gh-pages/
├── .nojekyll
├── index.html                                 (install instructions)
├── telephone-booth-archive-keyring.gpg        (public key for manual setup)
├── pool/main/t/telephone-booth/*.deb
└── dists/stable/
    ├── Release
    ├── Release.gpg
    ├── InRelease
    └── main/
        ├── binary-arm64/{Packages,Packages.gz}
        └── binary-armhf/{Packages,Packages.gz}
```

### Trust bootstrap

The `telephone-booth` `.deb` itself ships:

- `/usr/share/keyrings/telephone-booth-archive-keyring.gpg` — the public key.
- `/etc/apt/sources.list.d/telephone-booth.list` — a `signed-by=` source
  line pointing at GitHub Pages.
- `/etc/apt/apt.conf.d/50-telephone-booth-unattended` — an
  `Unattended-Upgrade::Origins-Pattern` matching our repo.

So the very first install (a `wget` + `apt install ./*.deb` from a GitHub
Release) is the only manual step. All future upgrades flow through
`apt upgrade telephone-booth`, including via `unattended-upgrades` when
that package is installed.

The keyring file lives in `packaging/debian/telephone-booth-archive-keyring.gpg`
in this repo. It is a real ed25519 public key but treat it as a
placeholder: the publish workflow (`publish.yml`) overwrites it from the
`APT_SIGNING_KEY` secret before `cargo deb` packages it, so production
`.deb`s ship the pubkey matching the secret.

### CI flow

```text
push: tags: ['v*']           push: tags: ['v*']
  │                            │
  ▼                            ▼
publish.yml builds .deb     publish.yml builds tarball
  │                            │
  └────────┬───────────────────┘
           ▼
publish.yml release job ─► uploads .deb + tarball to the Release
           │
           ▼ workflow_run on completion
publish-apt.yml
  ├── verifies release is not draft
  ├── downloads .deb from the Release (gh release download)
  ├── regenerates Packages + Release with apt-ftparchive
  ├── signs Release + InRelease with the production GPG key
  └── pushes pool/ + dists/ to gh-pages
```

The signing key lives in the `production-apt` GitHub Environment, which
should be configured in repo settings to allow deployments only from `main`
and protected by required reviewers.

## Alternatives considered (and rejected)

- **`reprepro` / `aptly` instead of `apt-ftparchive`.** Both maintain
  state in their own database which would complicate the
  `checkout gh-pages → mutate → push` workflow. `apt-ftparchive` is the
  canonical Debian-provided tool and operates statelessly on a directory.
- **Cloudsmith / packagecloud free tiers.** Equally good technically; we
  prefer not to add another vendor account.
- **`deb-s3` + S3 bucket.** Same idea but needs an AWS account; GH Pages
  is free.
- **`/etc/apt/trusted.gpg.d/` instead of `signed-by=`.** The modern best
  practice is per-repo `signed-by=` keyrings; `trusted.gpg.d` grants
  global trust to a key for every repo on the system.

## Consequences

**Good:**

- One-command upgrades on the Pi: `sudo apt upgrade telephone-booth`.
- `unattended-upgrades` can keep booths current with zero human attention.
- The signing model is auditable: the `gh-pages` push commit references
  the signing key fingerprint, and the `Release` file is fully reproducible
  from `pool/`.

**Trade-offs:**

- We are now responsible for an offline signing key. Lose it and there's
  no automatic recovery (see key rotation below).
- The `gh-pages` branch grows without bound as old `.deb`s accumulate
  in `pool/`. At ~10 MB per `.deb` × two architectures × N releases this
  is not a near-term problem but the runbook should document periodic
  pruning of versions older than the N most recent.
- Every release must use a fresh `version` in `Cargo.toml`. APT will not
  upgrade an installed package if the version string is unchanged, even
  if the `.deb` contents differ. release-please (ADR 0008) handles this
  automatically.

## Operating procedures

### One-time setup (repo owner)

1. Generate the signing keypair locally:

   ```sh
   gpg --quick-generate-key \
     "Telephone-Booth APT signing key <david@example.invalid>" \
     ed25519 sign 0
   gpg --armor --export-secret-keys "<fingerprint>"
   ```

2. Settings → Secrets and variables → Actions → New environment `production-apt`:
   - Add `APT_SIGNING_KEY` (the ASCII-armored secret key from step 1).
   - Optionally add `APT_SIGNING_KEY_PASSPHRASE` if the key has one.
   - Restrict the environment to the `main` branch.

3. Settings → Pages → Source = "Deploy from a branch", branch =
   `gh-pages`, folder = `/ (root)`.

4. Trigger `publish` for `v0.1.0` (workflow_dispatch). On success,
   `publish-apt` runs and creates the initial `gh-pages` content.

### Day-2 install on a Pi

Either:

- `sudo apt install ./telephone-booth_<v>_arm64.deb` (downloaded from
  the Release). All future `apt upgrade` cycles flow through the repo.

Or, without ever touching a Release artefact:

```sh
curl -fsSL https://djensenius.github.io/Telephone-Booth/telephone-booth-archive-keyring.gpg \
  | sudo install -m 0644 /dev/stdin /usr/share/keyrings/telephone-booth-archive-keyring.gpg
echo "deb [signed-by=/usr/share/keyrings/telephone-booth-archive-keyring.gpg] https://djensenius.github.io/Telephone-Booth stable main" \
  | sudo tee /etc/apt/sources.list.d/telephone-booth.list
sudo apt update
sudo apt install telephone-booth
```

### Key rotation

If the key has not been compromised, rotate in this order:

1. Generate a new keypair.
2. Open a PR that updates `packaging/debian/telephone-booth-archive-keyring.gpg`
   to a concatenation of the old + new public keys
   (`gpg --export OLD NEW > telephone-booth-archive-keyring.gpg`).
3. Release that PR (still signed with the old private key). Wait long
   enough for every booth to upgrade.
4. Rotate `APT_SIGNING_KEY` in the `production-apt` environment to the
   new private key. Subsequent releases sign with the new key.
5. One release later, drop the old key from the committed keyring.

If the old key is already compromised, the chain of trust is broken and
booths need a manual `apt-key`-equivalent reinstall (re-fetch the
keyring from GitHub Pages over TLS, or download a fresh `.deb`).

### Adding a new architecture

1. Add the new triple to `publish.yml`'s `build-deb` matrix.
2. Add the matching Debian architecture (e.g. `riscv64`) to
   `scripts/build-apt-repo.sh`'s default `ARCHES` array, or pass `--arch`
   from the workflow.
3. No client-side change is needed; `apt` discovers per-arch indexes from
   the `Architectures:` field in `Release`.

### Version immutability

Once a `telephone-booth_X.Y.Z_arm64.deb` is in `gh-pages/pool/`, it is
immutable. If a release ships broken, do not republish the same version;
ship `X.Y.(Z+1)` instead. APT clients cache by version string and will
not re-fetch a changed file with the same name.
