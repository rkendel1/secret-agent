import { errors } from "@appport/protocol";
import { readExport, TIERS } from "../legacy/crm.mjs";

// Reads the legacy CRM export.
export async function lookupCustomer(input) {
  const row = readExport().find((r) => r.cust_id === input.customerId);
  if (!row) {
    throw errors.notFound(`no customer ${input.customerId}`);
  }
  return {
    customerId: row.cust_id,
    name: row.full_name,
    email: row.mail,
    tier: TIERS[row.level],
    since: row.created
  };
}
