#!/bin/sh
# Install the lam CLI from a GitHub release.
#   curl -fsSL https://raw.githubusercontent.com/carlosarraes/lam/main/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- 0.1.3      (or LAM_VERSION=0.1.3)
# LAM_INSTALL_DIR overrides ~/.local/bin; LAM_RELEASE_URL overrides the download base.
set -eu

repo_url="${LAM_RELEASE_URL:-https://github.com/carlosarraes/lam/releases}"
install_dir="${LAM_INSTALL_DIR:-$HOME/.local/bin}"
version="${1:-${LAM_VERSION:-latest}}"

die() {
    echo "error: $*" >&2
    exit 1
}

case "$(uname -s)/$(uname -m)" in
    Darwin/arm64 | Darwin/aarch64) target=aarch64-apple-darwin ;;
    Darwin/x86_64) target=x86_64-apple-darwin ;;
    Linux/x86_64 | Linux/amd64) target=x86_64-unknown-linux-gnu ;;
    *) die "no prebuilt lam for $(uname -s) $(uname -m); build from source with 'just build'" ;;
esac

if [ "$version" = latest ]; then
    base="$repo_url/latest/download"
else
    version="${version#v}"
    base="$repo_url/download/v$version"
fi

if command -v sha256sum >/dev/null 2>&1; then
    sha256() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
    sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
    die 'need sha256sum or shasum'
fi

tmp=$(mktemp -d)
pending=''
trap 'rm -rf "$tmp"; [ -z "$pending" ] || rm -f "$pending"' EXIT

asset="lam-$target.tar.gz"
echo "Downloading $asset ($version)"
curl -fsSL -o "$tmp/$asset" "$base/$asset" || die "download failed: $base/$asset"
curl -fsSL -o "$tmp/SHA256SUMS" "$base/SHA256SUMS" || die "download failed: $base/SHA256SUMS"

expected=$(awk -v f="$asset" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS")
[ -n "$expected" ] || die "$asset missing from SHA256SUMS"
[ "$(sha256 "$tmp/$asset")" = "$expected" ] || die "checksum mismatch for $asset"

tar -xzf "$tmp/$asset" -C "$tmp" lam

# Stage next to the destination and rename: replacing the inode keeps running copies
# intact and avoids macOS killing a signed binary that was rewritten in place.
mkdir -p "$install_dir"
pending=$(mktemp "$install_dir/.lam-install.XXXXXX")
cp "$tmp/lam" "$pending"
chmod 755 "$pending"
"$pending" --version >/dev/null || die 'downloaded binary does not run'
mv -f "$pending" "$install_dir/lam"
pending=''

echo "Installed $("$install_dir/lam" --version) -> $install_dir/lam"
case ":$PATH:" in
    *":$install_dir:"*) ;;
    *) echo "note: $install_dir is not on PATH" ;;
esac
