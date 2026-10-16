#!/usr/bin/env bash
# Checks every coding fixture is sound: the visible tests fail before any change,
# and the golden solution passes both the visible and the hidden tests.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
status=0
for task in "$root"/fixtures/coding/*/; do
  name="$(basename "$task")"
  work="$(mktemp -d)"
  cp -R "$task/repo/." "$work/"
  # A fixture may link shared dependencies; point the link at the same place from the copy.
  if [ -L "$task/repo/node_modules" ]; then rm "$work/node_modules"; ln -s "$(cd "$task/repo/node_modules" && pwd -P)" "$work/node_modules"; fi
  test_cmd="$(node -e 'console.log(require(process.argv[1]).test.join(" "))' "$task/task.json")"
  hidden_cmd="$(node -e 'console.log(require(process.argv[1]).hidden.join(" "))' "$task/task.json")"
  if (cd "$work" && eval "$test_cmd" > /dev/null 2>&1); then echo "FAILED: $name: the visible tests already pass"; status=1; fi
  cp -R "$task/golden/." "$work/"
  (cd "$work" && eval "$test_cmd" > /dev/null 2>&1) || { echo "FAILED: $name: golden fails the visible tests"; status=1; }
  cp -R "$task/hidden" "$work/hidden"
  (cd "$work" && eval "$hidden_cmd" > /dev/null 2>&1) || { echo "FAILED: $name: golden fails the hidden tests"; status=1; }
  rm -rf "$work"
  [ "$status" = 0 ] && echo "ok: $name"
done
exit "$status"
