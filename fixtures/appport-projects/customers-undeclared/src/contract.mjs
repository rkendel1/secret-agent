// The application's contract, without effects or verification declarations
// (a contract the agent cannot verify a change against): what each capability promises, independent of how
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
  emits: ["customers.registered"],
  handler: registerCustomer
});

export const lookup = defineCapability({
  name: "customers.lookup",
  version: 1,
  description: "Reads one customer.",
  input: s.object({ customerId }),
  output: customer,
  authorization: ["customers.read"],
  handler: lookupCustomer
});

export const list = defineCapability({
  name: "customers.list",
  version: 1,
  description: "Lists every customer.",
  input: s.object({}),
  output: s.object({ customers: s.array(s.object({ customerId, name })) }),
  authorization: ["customers.read"],
  handler: listCustomers
});

export const contact = defineCapability({
  name: "billing.contact",
  version: 1,
  description: "Who to send a customer's invoices to.",
  input: s.object({ customerId }),
  output: s.object({ customerId, name, email }),
  authorization: ["customers.read"],
  handler: billingContact
});
