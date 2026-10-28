#!/bin/sh
# sphinx-ultra installer for Linux and macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/salioglu/sphinx-ultra/main/scripts/install.sh | sh
#
# Downloads the release archive for this platform from GitHub, verifies its
# SHA-256 checksum against the one published next to it, and installs the
# binary into $INSTALL_DIR (default: ~/.local/bin). It does not modify your
# shell configuration; add the directory to PATH yourself if needed.
#
# Environment:
#   INSTALL_DIR       where to put the binary            (default: $HOME/.local/bin)
#   SPHINX_ULTRA_VERSION   a tag to install, e.g. v0.6.0     (default: latest release)
#   SPHINX_ULTRA_MUSL      set to 1 to prefer the static musl build on Linux x86_64
#
# POSIX sh, no bashisms: it must run under dash, busybox sh and macOS /bin/sh.

set -eu

REPO="salioglu/sphinx-ultra"
BINARY="sphinx-ultra"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${SPHINX_ULTRA_VERSION:-}"

log()  { printf '%s\n' "$*" >&2; }
die()  { log "error: $*"; exit 1; }

usage() {
    sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'
}

case "${1:-}" in
    -h|--help) usage; exit 0 ;;
    "") ;;
    *) die "unknown argument: $1 (try --help)" ;;
esac

# --- helpers -----------------------------------------------------------------

fetch() {
    # fetch URL [OUTPUT]; prints to stdout when OUTPUT is omitted
    if command -v curl >/dev/null 2>&1; then
        if [ $# -ge 2 ]; then curl -fsSL "$1" -o "$2"; else curl -fsSL "$1"; fi
    elif command -v wget >/dev/null 2>&1; then
        if [ $# -ge 2 ]; then wget -q "$1" -O "$2"; else wget -qO- "$1"; fi
    else
        die "curl or wget is required"
    fi
}

detect_platform() {
    os=$(uname -s)
    arch=$(uname -m)
    case "$os" in
        Linux)
            case "$arch" in
                x86_64|amd64)
                    if [ "${SPHINX_ULTRA_MUSL:-0}" = "1" ]; then echo "linux-x86_64-musl"; else echo "linux-x86_64"; fi ;;
                aarch64|arm64) echo "linux-aarch64" ;;
                *) die "unsupported Linux architecture: $arch" ;;
            esac ;;
        Darwin)
            case "$arch" in
                x86_64) echo "macos-x86_64" ;;
                arm64)  echo "macos-aarch64" ;;
                *) die "unsupported macOS architecture: $arch" ;;
            esac ;;
        *) die "unsupported operating system: $os (Windows: download the .zip from https://github.com/$REPO/releases)" ;;
    esac
}

latest_version() {
    # The GitHub API answers with a JSON document; the tag is the only
    # "tag_name" key in it.
    fetch "https://api.github.com/repos/$REPO/releases/latest" \
        | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1
}

verify_checksum() {
    # verify_checksum DIR ARCHIVE
    dir=$1; archive=$2
    if [ ! -f "$dir/$archive.sha256" ]; then
        die "checksum file missing; refusing to install an unverified archive"
    fi
    if command -v sha256sum >/dev/null 2>&1; then
        (cd "$dir" && sha256sum -c "$archive.sha256" >/dev/null) || die "checksum mismatch for $archive"
    elif command -v shasum >/dev/null 2>&1; then
        (cd "$dir" && shasum -a 256 -c "$archive.sha256" >/dev/null) || die "checksum mismatch for $archive"
    else
        die "sha256sum or shasum is required to verify the download"
    fi
    log "checksum verified"
}

# --- main --------------------------------------------------------------------

command -v tar >/dev/null 2>&1 || die "tar is required"

platform=$(detect_platform)
log "platform: $platform"

if [ -z "$VERSION" ]; then
    VERSION=$(latest_version)
    [ -n "$VERSION" ] || die "could not determine the latest release (set SPHINX_ULTRA_VERSION=vX.Y.Z)"
fi
version_num="${VERSION#v}"
log "version: $VERSION"

archive="sphinx-ultra-${version_num}-${platform}.tar.gz"
base="https://github.com/$REPO/releases/download/$VERSION"

tmp=$(mktemp -d 2>/dev/null || mktemp -d -t sphinx-ultra)
trap 'rm -rf "$tmp"' EXIT INT TERM

log "downloading $base/$archive"
fetch "$base/$archive" "$tmp/$archive"
fetch "$base/$archive.sha256" "$tmp/$archive.sha256" || true
verify_checksum "$tmp" "$archive"

tar -xzf "$tmp/$archive" -C "$tmp"
src="$tmp/sphinx-ultra-${version_num}-${platform}/$BINARY"
[ -f "$src" ] || die "archive did not contain $BINARY"

mkdir -p "$INSTALL_DIR"
cp "$src" "$INSTALL_DIR/$BINARY"
chmod 755 "$INSTALL_DIR/$BINARY"
log "installed $INSTALL_DIR/$BINARY"

"$INSTALL_DIR/$BINARY" --version >&2 || true

case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *)
        log ""
        log "$INSTALL_DIR is not on your PATH. Add it, for example:"
        log "  export PATH=\"$INSTALL_DIR:\$PATH\""
        ;;
esac
log ""
log "Get started: $BINARY --help  (docs: https://github.com/$REPO/tree/main/docs)"
