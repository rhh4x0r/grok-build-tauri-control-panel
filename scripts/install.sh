#!/usr/bin/env bash
# Build production .app and install to /Applications.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export PATH="${HOME}/.grok/bin:${HOME}/.cargo/bin:${HOME}/.local/bin:/opt/homebrew/bin:/usr/local/bin:${PATH}"

echo "==> Checking grok CLI"
if ! command -v grok >/dev/null 2>&1; then
  if [[ -x "${HOME}/.grok/bin/grok" ]]; then
    export PATH="${HOME}/.grok/bin:${PATH}"
  else
    echo "ERROR: grok CLI not found. Install Grok Build first."
    exit 1
  fi
fi
grok version || true

echo "==> Building app bundle (release)"
"${ROOT}/scripts/bundle.sh"

SRC="${ROOT}/target/release/bundle/Bomb Code.app"
DEST="/Applications/Bomb Code.app"

if [[ ! -d "$SRC" ]]; then
  echo "ERROR: app bundle not found at $SRC"
  exit 1
fi

echo "==> Installing to $DEST"
rm -rf "$DEST"
cp -R "$SRC" "$DEST"

# Ad-hoc sign so Gatekeeper is less noisy for local builds (optional).
if command -v codesign >/dev/null 2>&1; then
  codesign --force --deep --sign - "$DEST" 2>/dev/null || true
fi

echo ""
echo "Installed: $DEST"
echo "Launch with: open \"$DEST\""
echo "Or:          ./scripts/run.sh"
echo ""
echo "Panel config:  ~/.grok/control-panel/config.toml"
echo "Grok CLI cfg:  ~/.grok/config.toml  (unchanged by panel)"
echo "Credentials:   ~/.grok/mcp_credentials.json"
open "$DEST"
