#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
source_dir="$root/build/nsh-v0.6.1"
patch="$root/scripts/nsh-editor-syntax-colors.patch"
revision=2f51d8611677c8a6bbad937b39756e8d0cc45350

if [[ ! -e "$source_dir" ]]; then
    git clone --depth 1 --branch v0.6.1 https://github.com/longbridge/nsh.git "$source_dir"
fi

if [[ "$(git -C "$source_dir" rev-parse HEAD)" != "$revision" ]]; then
    echo "NSH must be checked out at $revision in $source_dir" >&2
    exit 1
fi

if ! git -C "$source_dir" apply --reverse --check "$patch" 2>/dev/null; then
    git -C "$source_dir" apply --check "$patch"
    git -C "$source_dir" apply "$patch"
fi
