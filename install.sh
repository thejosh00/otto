#!/usr/bin/env bash
# Installer for otto: builds the Rust binary, places it on PATH, installs otto's Claude
# Code skills, and registers the launchd reviver agent.
#
#   ./install.sh
#
# Each step is also available on its own once otto is installed:
#
#   otto install   # (re-)symlink the skills otto ships into ~/.claude/skills
#   otto agent start   # (re-)register and start the launchd reviver — installs it if missing
#   otto agent stop    # unregister the launchd reviver
set -euo pipefail

repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
bin_dir="${OTTO_BIN_DIR:-$HOME/.local/bin}"

cd "$repo_dir"

echo "==> building otto (cargo build --release)"
cargo build --release

mkdir -p "$bin_dir"
install -m 755 target/release/otto "$bin_dir/otto"
echo "==> installed $bin_dir/otto"

if ! command -v otto >/dev/null 2>&1; then
	echo "note: $bin_dir is not on your PATH — add it to your shell profile to run otto directly." >&2
fi

echo "==> otto install (skills)"
"$bin_dir/otto" install --repo "$repo_dir"

echo "==> otto agent start (launchd reviver)"
"$bin_dir/otto" agent start

echo "==> done"
