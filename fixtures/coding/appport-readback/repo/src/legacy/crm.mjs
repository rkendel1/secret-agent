// The legacy CRM's nightly export. It is regenerated once a day, so customers
// registered since the last export are not in it.
import { readFileSync } from "node:fs";

const EXPORT = new URL("../../data/legacy-crm.json", import.meta.url);

export function readExport() {
  return JSON.parse(readFileSync(EXPORT, "utf8"));
}

export const TIERS = { standard: "standard", gold: "gold", platinum: "platinum" };
