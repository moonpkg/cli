#!/usr/bin/env bash
set -euo pipefail

REPO="moonpkg/cli"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/share/moon}"
BIN_DIR="${BIN_DIR:-$HOME/.local/bin}"

OS="$(uname -s)"
ARCH="$(uname -m)"

case "$ARCH" in
  x86_64|amd64) ARCH="x86_64" ;;
  aarch64|arm64) ARCH="aarch64" ;;
  *) echo "Unsupported architecture: $ARCH" >&2; exit 1 ;;
esac

case "$OS" in
  Linux) OS="linux" ;;
  *) echo "Unsupported OS: $OS" >&2; exit 1 ;;
esac

VERSION="${MOON_VERSION:-latest}"
if [ "$VERSION" = "latest" ]; then
  VERSION=$(curl -s "https://api.github.com/repos/${REPO}/releases/latest" 2>/dev/null | grep '"tag_name"' | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/' || true)
  if [ -z "$VERSION" ]; then
    echo "Could not determine latest version, falling back to main branch download not supported" >&2
    exit 1
  fi
fi

ASSET="moon-${VERSION}-${OS}-${ARCH}.tar.gz"
ALT_ASSET="moon-${OS}-${ARCH}.tar.gz"

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

echo "==> Downloading moon ${VERSION} for ${OS}/${ARCH}"
if curl -fsSL "https://github.com/${REPO}/releases/download/${VERSION}/${ASSET}" -o "$TMPDIR/${ASSET}" 2>/dev/null; then
  ARCHIVE="$TMPDIR/${ASSET}"
elif curl -fsSL "https://github.com/${REPO}/releases/download/${VERSION}/${ALT_ASSET}" -o "$TMPDIR/${ALT_ASSET}" 2>/dev/null; then
  ARCHIVE="$TMPDIR/${ALT_ASSET}"
else
  echo "Could not find release asset for ${OS}/${ARCH} at ${VERSION}" >&2
  echo "Check https://github.com/${REPO}/releases for available assets" >&2
  exit 1
fi

mkdir -p "$INSTALL_DIR"
echo "==> Extracting to $INSTALL_DIR"
tar -xzf "$ARCHIVE" -C "$INSTALL_DIR" --strip-components=1 2>/dev/null || tar -xzf "$ARCHIVE" -C "$INSTALL_DIR"

mkdir -p "$BIN_DIR"
echo "==> Installing binary to $BIN_DIR/moon"
install -m 755 "$INSTALL_DIR/moon" "$BIN_DIR/moon" 2>/dev/null || cp "$INSTALL_DIR/moon" "$BIN_DIR/moon" && chmod +x "$BIN_DIR/moon"

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *)
    echo
    echo "warning: $BIN_DIR is not in your PATH. Add this to your shell config:"
    echo "    export PATH=\"$BIN_DIR:\$PATH\""
    ;;
esac

echo
echo "Done. Try: moon --version"
