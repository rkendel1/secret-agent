#!/usr/bin/env bash
# Reproducible demo of the autonomous loop (PR 2).
#
# Points the agent at examples/customers-app, an application built with the
# published @appport/* packages, and asks it to replace customers.lookup@1 while
# preserving its contract. The agent observes the project, reads the contract the
# application's own runtime publishes, plans the change, asks a model only for
# the implementation, and accepts the result only if the running application
# satisfies every check derived from the contract.
#
#   scripts/demo.sh                      # scripted completions: no model, no key, reproducible
#   scripts/demo.sh --provider openai-compatible --model qwen2.5-coder:14b      # local LLM (Ollama)
#   scripts/demo.sh --provider openai-compatible --base-url http://localhost:8080/v1 --model local   # llama.cpp
#   scripts/demo.sh --provider anthropic --model claude-sonnet-5-5               # remote; needs ANTHROPIC_API_KEY
#
# Needs Node.js 20+ and a Rust toolchain. Writes target/demo/: the changed
# project copy, plan.json, report.json (metrics) and run.jsonl (every step,
# including the exact prompts sent to the model).
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo"
out="target/demo"
app="examples/customers-app"
intent="replace customers.lookup@1: read customers from the application's customer store instead of the legacy CRM export"

if [ "$#" -eq 0 ]; then
  set -- --provider scripted --responses examples/scripted/replace-customer-lookup
fi

if [ ! -d "$app/node_modules/@appport/server" ]; then
  echo "== installing $app dependencies"
  npm ci --prefix "$app" --no-audit --no-fund
fi

echo "== building appport-agent"
cargo build --quiet -p appport-agent
agent="target/debug/appport-agent"

echo "== fresh copy of the project in $out/project"
rm -rf "$out"
mkdir -p "$out/project"
cp -R "$app/src" "$app/data" "$app/package.json" "$out/project/"
ln -s "$repo/$app/node_modules" "$out/project/node_modules"

echo "== plan (no model involved)"
"$agent" plan --project "$out/project" --intent "$intent" --json > "$out/plan.json"
echo "   $out/plan.json"

echo "== run"
status=0
"$agent" run --project "$out/project" --intent "$intent" \
  --report "$out/report.json" --log "$out/run.jsonl" "$@" || status=$?

echo
echo "== change made"
diff -u "$app/src/handlers/customers.lookup.mjs" "$out/project/src/handlers/customers.lookup.mjs" || true
echo
echo "== metrics ($out/report.json)"
node -e '
const r = require(process.argv[1]);
const t = r.totals, d = r.deterministic, c = r.context;
console.log(`outcome                        ${r.outcome}`);
console.log(`model                          ${r.model}`);
console.log(`model calls                    ${t.modelCalls} (${t.rejectedAttempts} rejected by verification)`);
console.log(`tokens in / out                ${t.inputTokens} / ${t.outputTokens}${t.tokensEstimated ? " (estimated)" : ""}`);
console.log(`model calls for discovery      ${d.modelCallsForDiscovery}`);
console.log(`model calls for verification   ${d.modelCallsForVerification}`);
console.log(`facts established without it  ${d.capabilitiesInContract} capabilities, ${d.graphNodes} graph nodes / ${d.graphEdges} edges, ${d.dependentsOfTarget} dependents, ${d.replacementConstraints} constraints, ${d.acceptanceChecks} checks, ${d.decisions} decisions`);
console.log(`application runs / envelopes   ${d.applicationRuns} / ${d.envelopesExchanged}`);
console.log(`verification methods evaluated ${d.verificationMethodsEvaluated}`);
console.log(`context sent to the model      ${c.filesSent}/${c.projectSourceFiles} files, ${c.sourceBytesSent}/${c.projectSourceBytes} source bytes + ${c.knowledgeBytesSent} bytes of contract`);
for (const a of r.attempts) {
  const failed = a.verdict ? a.verdict.checks.filter((k) => k.status === "fail").map((k) => k.code) : [];
  console.log(`attempt ${a.attempt}                      ${a.verdict?.accepted ? "accepted" : "rejected"}${failed.length ? ": " + failed.join(", ") : ""}${a.unusable ? ": " + a.unusable : ""}`);
}
' "$repo/$out/report.json"
exit "$status"
