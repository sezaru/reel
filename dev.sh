#!/usr/bin/env bash
# Development: sync this tree to a build host and run cargo there (inside shell.nix).
#   ./dev.sh [host] [cargo args…]      (default: charmander, build --release)
set -euo pipefail
host=${1:-charmander}
shift || true
args=${*:-build --release}
here=$(cd "$(dirname "$0")" && pwd)
ssh "$host" mkdir -p .cache/agents/remote/reel
rsync -a --delete --exclude target --exclude result --exclude .agent-tmp "$here/" "$host:.cache/agents/remote/reel/"
ssh "$host" bash -c "'. /etc/agents/env.sh; cd ~/.cache/agents/remote/reel && nix-shell --run \"cargo $args\"'"
