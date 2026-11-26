#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
source_dir="$root/build/nsh-v0.6.1"
revision=2f51d8611677c8a6bbad937b39756e8d0cc45350

if [[ ! -e "$source_dir" ]]; then
    git clone --depth 1 --branch v0.6.1 https://github.com/longbridge/nsh.git "$source_dir"
fi

if [[ "$(git -C "$source_dir" rev-parse HEAD)" != "$revision" ]]; then
    echo "NSH must be checked out at $revision in $source_dir" >&2
    exit 1
fi

for patch in \
    "$root/scripts/nsh-editor-syntax-colors.patch" \
    "$root/scripts/nsh-shortcut-priority.patch" \
    "$root/scripts/nsh-editor-hover.patch" \
    "$root/scripts/nsh-editor-scroll.patch" \
    "$root/scripts/nsh-editor-undo.patch" \
    "$root/scripts/nsh-editor-caret-timer.patch" \
    "$root/scripts/nsh-superseded-region.patch" \
    "$root/scripts/nsh-background-test-keys.patch"; do
    if ! git -C "$source_dir" apply --reverse --check "$patch" 2>/dev/null; then
        git -C "$source_dir" apply --check "$patch"
        git -C "$source_dir" apply "$patch"
    fi
done
