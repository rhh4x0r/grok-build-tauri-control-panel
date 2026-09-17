#!/usr/bin/env bash
# Launch Bomb Code (dev build or installed app).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# Ensure CLI tools are on PATH for this process tree.
export PATH="${HOME}/.grok/bin:${HOME}/.cargo/bin:${HOME}/.local/bin:/opt/homebrew/bin:/usr/local/bin:${PATH}"

APP_BUNDLE="${ROOT}/target/release/bundle/Bomb Code.app"
INSTALLED="/Applications/Bomb Code.app"
BIN="${ROOT}/target/release/bomb_app"

if [[ "${1:-}" == "--dev" ]]; then
  exec cargo run -p bomb_app
fi

if [[ -d "$INSTALLED" ]]; then
  echo "Opening installed app: $INSTALLED"
  exec open "$INSTALLED"
fi

if [[ -d "$APP_BUNDLE" ]]; then
  echo "Opening built app: $APP_BUNDLE"
  exec open "$APP_BUNDLE"
fi

if [[ -x "$BIN" ]]; then
  echo "Running release binary: $BIN"
  exec "$BIN"
fi

echo "No build found. Building release binary…"
cargo build --release -p bomb_app
exec "$BIN"
