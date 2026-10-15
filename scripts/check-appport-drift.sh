#!/usr/bin/env bash
# Drift check for the AppPort knowledge module.
#
#   scripts/check-appport-drift.sh            pinned: the checked-in surface and fixtures must match
#                                             the exact @appport/* versions in tools/appport-inspect
#                                             (reproducible; run on every change)
#   scripts/check-appport-drift.sh --latest   latest: re-inspect the newest published @appport/*
#                                             packages and fail if anything the Rust model mirrors
#                                             changed (run on a schedule)
#
# APPPORT_SPECS overrides the package specs used by --latest, e.g. to rehearse an upgrade:
#   APPPORT_SPECS="@appport/protocol@1.2.0 @appport/core@1.3.0" scripts/check-appport-drift.sh --latest
#
# Both modes then run the Rust side of the check (cargo tests that compare the Rust
# types and primitives with the surface and fixtures).

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tool="$root/tools/appport-inspect"
crate="$root/crates/agent-core"
mode="${1:---pinned}"

case "$mode" in
  --pinned)
    (cd "$tool" && npm ci --no-audit --no-fund --silent && node inspect.mjs --check "$crate")
    ;;
  --latest)
    work="$(mktemp -d)"
    trap 'rm -rf "$work"' EXIT
    cp -R "$tool/package.json" "$tool/inspect.mjs" "$tool/fixtures-src" "$work/"
    specs="${APPPORT_SPECS:-@appport/core@latest @appport/protocol@latest @appport/schema@latest @appport/testing@latest}"
    echo "Inspecting: $specs"
    # shellcheck disable=SC2086
    (cd "$work" && npm install --no-audit --no-fund --silent $specs && node inspect.mjs --check "$crate")
    ;;
  *)
    echo "usage: $0 [--pinned|--latest]" >&2
    exit 2
    ;;
esac

cargo test --manifest-path "$root/Cargo.toml" -p agent-core --test surface --test fixtures --test parity --quiet
