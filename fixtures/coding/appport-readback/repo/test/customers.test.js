import { test } from "node:test";
import assert from "node:assert/strict";

import { registerCustomer } from "../src/handlers/customers.register.mjs";
import { lookupCustomer } from "../src/handlers/customers.lookup.mjs";

const context = { emit: async () => {} };

test("a customer can be looked up right after registering", async () => {
  const created = await registerCustomer({ name: "Ada Lovelace", email: "first.registration@new.example", tier: "gold" }, context);
  const found = await lookupCustomer({ customerId: created.customerId });
  assert.equal(found.email, "first.registration@new.example");
  assert.equal(found.tier, "gold");
});
