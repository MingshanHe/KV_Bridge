#!/bin/bash
# Installs kvm-mac into ~/kvm-mac. Run in Terminal:
#   curl -fsSL https://raw.githubusercontent.com/MingshanHe/KV_Bridge/main/install-mac.sh | bash
set -euo pipefail

URL="https://github.com/MingshanHe/KV_Bridge/releases/download/latest/kvm-mac.dmg"
DEST="$HOME/kvm-mac"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "Downloading kvm-mac..."
curl -fsSL -o "$TMP/kvm-mac.dmg" "$URL"
hdiutil attach -quiet -nobrowse -readonly -mountpoint "$TMP/mnt" "$TMP/kvm-mac.dmg"

rm -rf "$DEST"
cp -R "$TMP/mnt/kvm-mac" "$DEST"
hdiutil detach -quiet "$TMP/mnt"
chmod +x "$DEST/kvm-mac" "$DEST/Start kvm-mac.command"
xattr -dr com.apple.quarantine "$DEST" 2>/dev/null || true

echo
echo "Installed to $DEST"
echo
echo "Next:"
echo "  1. Check typing works:   $DEST/kvm-mac --test-type"
echo "     (allow Terminal under System Settings > Privacy & Security > Accessibility)"
echo "  2. Connect to Windows:   $DEST/kvm-mac <Windows IP>"
echo "     or double-click 'Start kvm-mac' in the $DEST folder"
