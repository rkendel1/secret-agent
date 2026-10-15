#!/usr/bin/env node
// Fails unless a run report shows a verified, accepted change, and optionally
// that the verifier rejected and rolled back at least N proposals first.
//
//   node scripts/check-report.mjs <report.json> [--min-rejected N] [--min-rollbacks N]
import { readFileSync } from "node:fs";

const [path, ...args] = process.argv.slice(2);
const flag = (name) => {
  const i = args.indexOf(name);
  return i < 0 ? 0 : Number(args[i + 1]);
};
const report = JSON.parse(readFileSync(path, "utf8"));
const last = report.attempts.at(-1);
const problems = [];
if (report.outcome !== "accepted") problems.push(`outcome is ${report.outcome}`);
if (!last?.verdict?.accepted || last.verdict.checks.some((c) => c.status !== "pass")) problems.push("the last attempt did not pass every check");
if (!(last?.evidence ?? []).some((e) => e.endsWith(": verified"))) problems.push("no declared verification was established on the accepted change");
if (report.totals.rejectedAttempts < flag("--min-rejected")) problems.push(`${report.totals.rejectedAttempts} rejected attempt(s), expected at least ${flag("--min-rejected")}`);
if (report.totals.rollbacks < flag("--min-rollbacks")) problems.push(`${report.totals.rollbacks} rollback(s), expected at least ${flag("--min-rollbacks")}`);
if (report.deterministic.modelCallsForDiscovery !== 0 || report.deterministic.modelCallsForVerification !== 0) problems.push("the model was used for discovery or verification");
if (problems.length) {
  console.error(`${path}: FAILED\n  - ${problems.join("\n  - ")}`);
  process.exit(1);
}
console.log(`${path}: accepted after ${report.attempts.length} attempt(s) (${report.totals.rejectedAttempts} rejected, ${report.totals.rollbacks} rolled back), ${report.totals.modelCalls} model call(s), ${report.totals.wallMs} ms`);
