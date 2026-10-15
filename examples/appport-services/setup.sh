#!/usr/bin/env bash
# Prepares rkendel1/appport-services for the closed-loop test.
#
#   examples/appport-services/setup.sh <dir> [stage]
#
# stages (cumulative):
#   clone      the repository at the pinned commit, untouched
#   init       + `npx appport init` and the runtime packages it needs (@appport/sdk)
#   declared   + the contract declarations in declare-contract.patch (default)
#
# The repository is never modified upstream; everything happens in <dir>.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dir="${1:?usage: setup.sh <dir> [clone|init|declared]}"
stage="${2:-declared}"
repo="https://github.com/rkendel1/appport-services"
commit="81426d60e3368cd834a409007beb2d9630dc146b"
cli="appport@1.1.20"

rm -rf "$dir"
mkdir -p "$dir"
git -C "$dir" init -q
git -C "$dir" fetch -q --depth 1 "$repo" "$commit"
git -C "$dir" -c advice.detachedHead=false checkout -q FETCH_HEAD
echo "appport-services @ $commit in $dir"
[ "$stage" = clone ] && exit 0

(cd "$dir" && npx --yes "$cli" init)
npm install --prefix "$dir" --no-audit --no-fund --save-exact @appport/sdk@1.2.4 @appport/schema@1.1.0
[ "$stage" = init ] && exit 0

patch -s -d "$dir" -p1 < "$here/declare-contract.patch"
echo "contract declarations applied"
