#!/bin/bash
# Double-click to start kvm-mac. Asks for the Windows laptop's IP the first time
# and remembers it in ~/.kvm-mac-host. To change it: rm ~/.kvm-mac-host
cd "$(dirname "$0")" || exit 1

# Files downloaded through a browser are quarantined; clear it for this folder.
xattr -dr com.apple.quarantine . 2>/dev/null
chmod +x ./kvm-mac 2>/dev/null

CONF="$HOME/.kvm-mac-host"
HOST="$(cat "$CONF" 2>/dev/null)"
if [ -z "$HOST" ]; then
    echo "Enter the Windows laptop's IP address (kvm-win prints it when it starts):"
    read -r HOST
    [ -z "$HOST" ] && { echo "No address given."; exit 1; }
    echo "$HOST" > "$CONF"
fi

echo "Connecting to $HOST  (Ctrl+C to stop)"
exec ./kvm-mac "$HOST" "$@"
