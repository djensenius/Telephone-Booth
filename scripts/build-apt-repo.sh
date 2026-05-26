#!/usr/bin/env bash
# build-apt-repo.sh — assemble / refresh a static Debian APT repository.
#
# Run from CI (`publish-apt.yml`) or locally for debugging. Idempotent:
# repeated runs over the same input produce the same output.
#
# The signing key must already be present in the active GnuPG keyring;
# pass its fingerprint with --signing-key-fpr. If your key has a passphrase,
# export it in $APT_SIGNING_KEY_PASSPHRASE.
#
# Repository layout produced:
#
#   <pages-dir>/
#     .nojekyll
#     pool/main/t/telephone-booth/<file>.deb
#     dists/stable/Release
#     dists/stable/Release.gpg
#     dists/stable/InRelease
#     dists/stable/main/binary-arm64/Packages{,.gz}
#     dists/stable/main/binary-armhf/Packages{,.gz}
#     telephone-booth-archive-keyring.gpg   (optional, --pubkey-output)
#     index.html                            (only if missing)

set -euo pipefail

PAGES_DIR=""
DEBS_DIR=""
SIGNING_KEY_FPR=""
PUBKEY_OUTPUT=""
ORIGIN="Telephone-Booth"
LABEL="Telephone-Booth"
SUITE="stable"
CODENAME="stable"
COMPONENT="main"
ARCHES=("arm64" "armhf")
DESCRIPTION="Telephone-Booth phone-side client packages"

usage() {
    cat <<USAGE
Usage: $0 --pages-dir PATH --debs-dir PATH --signing-key-fpr FPR [options]

Required:
  --pages-dir PATH         gh-pages worktree to mutate
  --debs-dir PATH          directory containing new .deb files to ingest
  --signing-key-fpr FPR    GPG fingerprint of the (already imported) signing key

Optional:
  --pubkey-output PATH     also export the public key here (e.g. inside pages-dir)
  --origin NAME            APT Origin field (default: $ORIGIN)
  --label NAME             APT Label field (default: $LABEL)
  --suite NAME             APT Suite field (default: $SUITE)
  --codename NAME          APT Codename field (default: $CODENAME)
  --component NAME         APT component (default: $COMPONENT)
  --arch ARCH              Additional architecture (repeatable; default: arm64 armhf)
USAGE
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --pages-dir) PAGES_DIR="$2"; shift 2 ;;
        --debs-dir) DEBS_DIR="$2"; shift 2 ;;
        --signing-key-fpr) SIGNING_KEY_FPR="$2"; shift 2 ;;
        --pubkey-output) PUBKEY_OUTPUT="$2"; shift 2 ;;
        --origin) ORIGIN="$2"; shift 2 ;;
        --label) LABEL="$2"; shift 2 ;;
        --suite) SUITE="$2"; shift 2 ;;
        --codename) CODENAME="$2"; shift 2 ;;
        --component) COMPONENT="$2"; shift 2 ;;
        --arch) ARCHES+=("$2"); shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; usage; exit 2 ;;
    esac
done

if [[ -z "$PAGES_DIR" || -z "$DEBS_DIR" || -z "$SIGNING_KEY_FPR" ]]; then
    echo "Missing required argument." >&2
    usage
    exit 2
fi

if [[ ! -d "$PAGES_DIR" ]]; then
    echo "pages dir does not exist: $PAGES_DIR" >&2
    exit 1
fi

if ! command -v apt-ftparchive >/dev/null 2>&1; then
    echo "apt-ftparchive not found; install the apt-utils package" >&2
    exit 1
fi

mkdir -p "$PAGES_DIR/pool/main/t/telephone-booth"
touch "$PAGES_DIR/.nojekyll"

# Resolve PUBKEY_OUTPUT to an absolute path now, before we 'cd' into
# PAGES_DIR. Otherwise a relative path like 'pages/foo.gpg' breaks after
# the cd (it would resolve to '<pages-dir>/pages/foo.gpg').
if [[ -n "$PUBKEY_OUTPUT" && "$PUBKEY_OUTPUT" != /* ]]; then
    PUBKEY_OUTPUT="$PWD/$PUBKEY_OUTPUT"
fi

# Copy any newly-built .debs into the pool. Skip if no new files; that allows
# rerunning purely to regenerate metadata.
shopt -s nullglob
for deb in "$DEBS_DIR"/*.deb; do
    cp -v "$deb" "$PAGES_DIR/pool/main/t/telephone-booth/"
done
shopt -u nullglob

# Run apt-ftparchive from inside the pages dir so `Filename:` entries are
# repo-relative (e.g. `pool/main/t/telephone-booth/foo.deb`), not absolute.
cd "$PAGES_DIR"

# Track every per-iteration scratch dir in an array and clean them up with a
# single EXIT trap. Using `trap … EXIT` inside the loop would overwrite any
# outer trap on every iteration (bash only supports one handler per signal),
# which is fragile if cleanup is added later.
TMP_DIRS=()
cleanup() {
    local d
    for d in "${TMP_DIRS[@]:-}"; do
        [ -n "$d" ] && rm -rf "$d"
    done
}
trap cleanup EXIT

# Per-arch Packages indexes. We filter the pool by the .deb name suffix so a
# stray arch doesn't leak into the wrong index.
for arch in "${ARCHES[@]}"; do
    arch_dir="dists/$CODENAME/$COMPONENT/binary-$arch"
    mkdir -p "$arch_dir"

    # Build a temporary directory of symlinks to just the .debs for this arch.
    tmp_pool="$(mktemp -d)"
    TMP_DIRS+=("$tmp_pool")

    # Match Debian's `_<arch>.deb` filename convention.
    pool="pool/main/t/telephone-booth"
    matched=0
    for deb in "$pool"/*_"$arch".deb; do
        [[ -e "$deb" ]] || continue
        ln -sf "$PWD/$deb" "$tmp_pool/$(basename "$deb")"
        matched=$((matched + 1))
    done
    echo "arch=$arch: ${matched} package(s) in index"

    # apt-ftparchive packages takes a directory; we point it at the symlink
    # farm but rewrite Filename: back to the canonical pool path with sed.
    apt-ftparchive packages "$tmp_pool" \
        | sed -E "s|^Filename: .*/|Filename: ${pool}/|" \
        > "$arch_dir/Packages"
    gzip -9kf "$arch_dir/Packages"
done

# Release file. Arch list reflects what we actually built.
arch_list="$(printf '%s ' "${ARCHES[@]}" | sed 's/ $//')"

cat > apt-ftparchive-release.conf <<EOF
APT::FTPArchive::Release::Origin "$ORIGIN";
APT::FTPArchive::Release::Label "$LABEL";
APT::FTPArchive::Release::Suite "$SUITE";
APT::FTPArchive::Release::Codename "$CODENAME";
APT::FTPArchive::Release::Components "$COMPONENT";
APT::FTPArchive::Release::Architectures "$arch_list";
APT::FTPArchive::Release::Description "$DESCRIPTION";
EOF

apt-ftparchive -c apt-ftparchive-release.conf release "dists/$CODENAME" \
    > "dists/$CODENAME/Release"
rm -f apt-ftparchive-release.conf

# Sign Release. --batch + --yes overwrite previous signatures from a prior run.
GPG_BASE_ARGS=(--batch --yes --pinentry-mode loopback --local-user "$SIGNING_KEY_FPR")
if [[ -n "${APT_SIGNING_KEY_PASSPHRASE:-}" ]]; then
    GPG_BASE_ARGS+=(--passphrase "$APT_SIGNING_KEY_PASSPHRASE")
fi

gpg "${GPG_BASE_ARGS[@]}" -abs -o "dists/$CODENAME/Release.gpg" "dists/$CODENAME/Release"
gpg "${GPG_BASE_ARGS[@]}" --clearsign -o "dists/$CODENAME/InRelease" "dists/$CODENAME/Release"

# Optionally export the public key alongside the repo for fresh installs.
if [[ -n "$PUBKEY_OUTPUT" ]]; then
    gpg --batch --yes --export "$SIGNING_KEY_FPR" > "$PUBKEY_OUTPUT"
    echo "Exported pubkey to $PUBKEY_OUTPUT"
fi

# Static install-instructions index page (only if missing — never overwrite
# a customised landing page).
if [[ ! -f index.html ]]; then
    cat > index.html <<'HTML'
<!doctype html>
<meta charset="utf-8">
<title>Telephone-Booth APT repository</title>
<style>
  body { font-family: -apple-system, BlinkMacSystemFont, sans-serif; max-width: 48rem; margin: 2rem auto; padding: 0 1rem; color: #222; line-height: 1.45; }
  pre { background: #f3f3f3; padding: 1rem; overflow-x: auto; border-radius: 6px; }
  code { background: #f3f3f3; padding: 0.1rem 0.3rem; border-radius: 4px; }
</style>
<h1>Telephone-Booth APT repository</h1>
<p>This is the APT repository for the
  <a href="https://github.com/djensenius/Telephone-Booth">Telephone-Booth</a>
  Raspberry Pi client. To install or upgrade on a Pi:</p>
<pre><code>curl -fsSL https://djensenius.github.io/Telephone-Booth/telephone-booth-archive-keyring.gpg \
  | sudo install -m 0644 /dev/stdin /usr/share/keyrings/telephone-booth-archive-keyring.gpg

echo "deb [signed-by=/usr/share/keyrings/telephone-booth-archive-keyring.gpg] https://djensenius.github.io/Telephone-Booth stable main" \
  | sudo tee /etc/apt/sources.list.d/telephone-booth.list

sudo apt update
sudo apt install telephone-booth
</code></pre>
<p>The <code>telephone-booth</code> package itself ships the keyring and a
  matching <code>/etc/apt/sources.list.d/telephone-booth.list</code>, so future
  upgrades flow through <code>apt upgrade</code> automatically.</p>
HTML
fi

echo "APT repo build complete in $PAGES_DIR"
