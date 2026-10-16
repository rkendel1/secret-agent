import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { registerCustomer } from "../src/handlers/customers.register.mjs";
import { lookupCustomer } from "../src/handlers/customers.lookup.mjs";
import { billingContact } from "../src/handlers/billing.contact.mjs";

const context = { emit: async () => {} };
const legacy = JSON.parse(readFileSync(new URL("../data/legacy-crm.json", import.meta.url), "utf8"));

test("every customer from the legacy export is still found, unchanged", async () => {
  for (const row of legacy) {
    const found = await lookupCustomer({ customerId: row.cust_id });
    assert.deepEqual(
      { customerId: found.customerId, name: found.name, email: found.email, since: found.since },
      { customerId: row.cust_id, name: row.full_name, email: row.mail, since: row.created }
    );
  }
});

test("an unknown customer is NOT_FOUND, as the contract declares", async () => {
  await assert.rejects(lookupCustomer({ customerId: "cus_does_not_exist" }), (e) => e.code === "NOT_FOUND");
});

test("billing contacts work for newly registered customers too", async () => {
  const created = await registerCustomer({ name: "Grace Hopper", email: "second.registration@new.example", tier: "platinum" }, context);
  const contact = await billingContact({ customerId: created.customerId }, context);
  assert.deepEqual(contact, { customerId: created.customerId, name: "Grace Hopper", email: "second.registration@new.example" });
});
