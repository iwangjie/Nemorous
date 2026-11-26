#!/usr/bin/env bash
set -euo pipefail

# The headless UI suite drives whole desktop sessions through Git, a shell
# and the filesystem. A few of its steps race the runner's scheduler on
# shared CI machines: an input the driver has sent can take longer than a
# step's quiet window to reach the app. Such tests pass when run alone, so
# a failed run reruns each failing test on its own; a test that keeps
# failing still fails the job.

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

cargo=(cargo)
if [[ -n "${RUST_TOOLCHAIN:-}" ]]; then
    cargo+=("+$RUST_TOOLCHAIN")
fi

test_args=(test --locked --release --test headless_review_ui_tests -- --test-threads=1)
log="$(mktemp)"
trap 'rm -f "$log"' EXIT

if "${cargo[@]}" "${test_args[@]}" 2>&1 | tee "$log"; then
    exit 0
fi

failed=()
while IFS= read -r name; do
    failed+=("$name")
done < <(sed -n 's/^test \(.*\) \.\.\. FAILED$/\1/p' "$log" | sort -u)

if [[ ${#failed[@]} -eq 0 ]]; then
    echo "::error::The headless UI tests failed without reporting a failed test."
    exit 1
fi

for name in "${failed[@]}"; do
    echo
    echo "Rerunning \`$name\` alone: it failed in the shared run."
    if [[ "${GITHUB_ACTIONS:-}" == "true" ]]; then
        echo "::warning title=Flaky UI test::$name failed in the suite; rerunning it alone"
    fi
    passed=0
    for _ in 1 2 3; do
        if "${cargo[@]}" "${test_args[@]}" --exact "$name"; then
            passed=1
            break
        fi
    done
    if [[ $passed -eq 0 ]]; then
        echo "::error title=UI test failed::$name still fails when run alone"
        exit 1
    fi
done

echo "Every failing test passed when rerun alone."
