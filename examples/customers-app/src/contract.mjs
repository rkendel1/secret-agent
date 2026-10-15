// The application's contract: what each capability promises, independent of how
// it is implemented. Implementations live in ./handlers.
import { defineCapability, defineEvent } from "@appport/core";
import { annotate, s, semantic } from "@appport/schema";

import { billingContact } from "./handlers/billing.contact.mjs";
import { listCustomers } from "./handlers/customers.list.mjs";
import { lookupCustomer } from "./handlers/customers.lookup.mjs";
import { registerCustomer } from "./handlers/customers.register.mjs";

export const CUSTOMER = "https://schema.example.com/crm/Customer";
export const CUSTOMER_RESOURCE = "urn:example:customers:customer";
const AUTHORITY = "https://schema.example.com/authority/crm";

const customerId = annotate(s.string({ minLength: 1 }), semantic({ concept: CUSTOMER }));
const email = annotate(s.string({ format: "email" }), semantic({ concept: "https://schema.example.com/crm/Email" }));
const tier = annotate(
  s.enum(["standard", "gold", "platinum"]),
  semantic({
    concept: "https://schema.example.com/crm/Tier",
    enumConcepts: {
      standard: "https://schema.example.com/crm/Tier#standard",
      gold: "https://schema.example.com/crm/Tier#gold",
      platinum: "https://schema.example.com/crm/Tier#platinum"
    }
  })
);
const since = annotate(s.timestamp(), semantic({ concept: "https://schema.example.com/crm/customerSince", encoding: "iso8601" }));
const name = s.string({ minLength: 1 });

const customer = s.object({ customerId, name, email, tier, since });

export const registered = defineEvent({
  name: "customers.registered",
  version: 1,
  description: "A customer was registered.",
  payload: s.object({ customerId, email }),
  authorization: ["customers.read"]
});

export const register = defineCapability({
  name: "customers.register",
  version: 1,
  description: "Registers a new customer.",
  input: s.object({ name, email, tier }),
  output: customer,
  authorization: ["customers.write"],
  effect: "consequential",
  idempotency: "unsupported",
  emits: ["customers.registered"],
  integration: {
    purpose: "Create a customer record.",
    authority: AUTHORITY,
    effects: [{ type: "https://schema.example.com/effects/customer-created" }],
    verify: [
      {
        id: "customer-readable",
        kind: "state",
        via: { capability: "customers.lookup", version: 1, input: { customerId: { $output: "/customerId" } } },
        expect: [
          { at: "/email", equals: { $input: "/email" } },
          { at: "/tier", equals: { $input: "/tier" } },
          { at: "/since", exists: true }
        ]
      },
      {
        id: "registered-event",
        kind: "event",
        event: "customers.registered",
        match: { "/customerId": { $output: "/customerId" } },
        strength: "corroborating"
      }
    ],
    access: [{ resource: CUSTOMER_RESOURCE, mode: "changes", operations: ["create"] }],
    failures: [{ code: "CONFLICT", retry: "unsafe", effect: "none", description: "The email is already registered." }],
    coverage: { access: "complete", failures: "complete" },
    approval: { required: false }
  },
  handler: registerCustomer
});

export const lookup = defineCapability({
  name: "customers.lookup",
  version: 1,
  description: "Reads one customer.",
  input: s.object({ customerId }),
  output: customer,
  authorization: ["customers.read"],
  effect: "observation",
  idempotency: "supported",
  integration: {
    purpose: "Look up the current state of a customer.",
    authority: AUTHORITY,
    access: [{ resource: CUSTOMER_RESOURCE, mode: "reads" }],
    failures: [{ code: "NOT_FOUND", retry: "safe", effect: "none", description: "No such customer." }],
    coverage: { requires: "complete", access: "complete", failures: "complete" }
  },
  handler: lookupCustomer
});

export const list = defineCapability({
  name: "customers.list",
  version: 1,
  description: "Lists every customer.",
  input: s.object({}),
  output: s.object({ customers: s.array(s.object({ customerId, name })) }),
  authorization: ["customers.read"],
  effect: "observation",
  idempotency: "supported",
  integration: {
    authority: AUTHORITY,
    access: [{ resource: CUSTOMER_RESOURCE, mode: "reads" }],
    coverage: { requires: "complete", access: "complete" }
  },
  handler: listCustomers
});

export const contact = defineCapability({
  name: "billing.contact",
  version: 1,
  description: "Who to send a customer's invoices to.",
  input: s.object({ customerId }),
  output: s.object({ customerId, name, email }),
  authorization: ["customers.read"],
  effect: "observation",
  idempotency: "supported",
  integration: {
    requires: [{ id: "customer", relationship: "observes", target: { capability: "customers.lookup@1" }, required: true }],
    failures: [{ code: "NOT_FOUND", retry: "safe", effect: "none" }],
    coverage: { requires: "complete", failures: "complete" }
  },
  handler: billingContact
});
