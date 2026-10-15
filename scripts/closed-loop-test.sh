#!/usr/bin/env bash
# The closed-loop autonomy test (PR 3).
#
#   scripts/closed-loop-test.sh                         scripted completions; deterministic; what CI runs
#   scripts/closed-loop-test.sh --config <model.json>   the same, then a real-model evaluation
#                                                       (knowledge vs bare, TRIALS per arm, default 5)
#
# Stages, each asserted:
#   1. appport-services as published: no AppPort runtime -> the agent refuses, no model call
#   2. appport-services after `npx appport init`: a contract that declares no effects or
#      verification -> the agent refuses to accept changes it cannot verify, no model call
#   3. appport-services with its contract declared: replace documents.open@1 -> one proposal
#      rejected and rolled back, the retry verified and accepted
#   4. examples/customers-app: replace customers.lookup@1 (scripts/demo.sh), same assertions
#   5. with --config: the evaluation on both projects; results in target/closed-loop/eval-*
#
# Needs git, Node.js 20+, npm and a Rust toolchain. Writes target/closed-loop/.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo"
out="target/closed-loop"
setup="examples/appport-services/setup.sh"
open_intent="replace documents.open@1: return a copy of the stored document instead of the live record"
lookup_intent="replace customers.lookup@1: read customers from the application's customer store instead of the legacy CRM export"
trials="${TRIALS:-5}"
rm -rf "$out"
mkdir -p "$out"

cargo build --quiet -p appport-agent
agent="$repo/target/debug/appport-agent"

expect_refusal() { # <label> <project> <intent> <code>
  local status=0
  "$agent" plan --project "$2" --intent "$3" > "$out/$1.txt" 2>&1 || status=$?
  if [ "$status" -ne 2 ] || ! grep -q "$4" "$out/$1.txt"; then
    echo "FAILED: $1: expected a refusal naming $4" >&2
    cat "$out/$1.txt" >&2
    exit 1
  fi
  echo "ok: $1: refused ($4), no model call"
}

echo "== 1. appport-services as published"
"$setup" "$out/services-clone" clone
expect_refusal 1-services-clone "$out/services-clone" "replace documents.open@1" "appport.runtime-absent"

echo "== 2. appport-services after npx appport init"
"$setup" "$out/services-init" init
expect_refusal 2-services-init "$out/services-init" "$open_intent" "contract.unverifiable"

echo "== 3. appport-services with its contract declared"
"$setup" "$out/services" declared
cp -R "$out/services" "$out/services-pristine"
"$agent" run --project "$out/services" --intent "$open_intent" \
  --provider scripted --responses examples/appport-services/scripted/replace-documents-open \
  --report "$out/3-services.report.json" --log "$out/3-services.run.jsonl" 2> "$out/3-services.log.txt" || true
node scripts/check-report.mjs "$out/3-services.report.json" --min-rejected 1 --min-rollbacks 1

echo "== 4. examples/customers-app"
scripts/demo.sh > "$out/4-customers.txt" 2>&1 || { cat "$out/4-customers.txt" >&2; exit 1; }
cp target/demo/report.json "$out/4-customers.report.json"
node scripts/check-report.mjs "$out/4-customers.report.json" --min-rejected 1 --min-rollbacks 1

if [ "$#" -gt 0 ]; then
  echo "== 5. evaluation with a real model ($trials trial(s) per arm)"
  status=0
  "$agent" eval --project "$out/services-pristine" --intent "$open_intent" --trials "$trials" \
    --out "$out/eval-appport-services" "$@" || status=$?
  "$agent" eval --project examples/customers-app --intent "$lookup_intent" --trials "$trials" \
    --out "$out/eval-customers-app" "$@" || status=$?
  echo "results: $out/eval-appport-services/eval.md, $out/eval-customers-app/eval.md"
  exit "$status"
fi
echo "closed loop: all stages passed"
