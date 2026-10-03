#!/usr/bin/env bash
# Build moon from source and install it to ~/.local/bin (override with PREFIX=...).
set -euo pipefail

cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")"

PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"

if ! command -v cargo >/dev/null 2>&1 && [ -x "$HOME/.cargo/bin/cargo" ]; then
  export PATH="$HOME/.cargo/bin:$PATH"
fi

if ! command -v cargo >/dev/null 2>&1; then
  cat >&2 <<'MSG'
error: cargo (Rust) not found.
  Install it with your package manager (e.g. `sudo pacman -S rust`,
  `sudo apt install cargo`, `sudo dnf install cargo`) or via https://rustup.rs
MSG
  exit 1
fi

echo "==> Building moon (release)"
cargo build --release

echo "==> Installing to $BIN_DIR/moon"
mkdir -p "$BIN_DIR"
install -m 755 target/release/moon "$BIN_DIR/moon"

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *)
    echo
    echo "warning: $BIN_DIR is not in your PATH. Add this to ~/.bashrc / ~/.zshrc:"
    echo "    export PATH=\"$BIN_DIR:\$PATH\""
    echo "(then open a new terminal)"
    ;;
esac

echo
echo "Done. Try:  moon help"
