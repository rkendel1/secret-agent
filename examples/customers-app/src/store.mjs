// The application's customer store: the system of record for
// `urn:example:customers:customer`. Seeded from the legacy export at start-up
// (the migration), then written by `customers.register`.
import { readExport, TIERS } from "./legacy/crm.mjs";

const customers = new Map();

for (const row of readExport()) {
  customers.set(row.cust_id, {
    customerId: row.cust_id,
    name: row.full_name,
    email: row.mail,
    tier: TIERS[row.level],
    since: row.created
  });
}

let next = 2001;

export function nextCustomerId() {
  return `cus_${next++}`;
}

export function getCustomer(customerId) {
  return customers.get(customerId);
}

export function findByEmail(email) {
  for (const customer of customers.values()) {
    if (customer.email === email) return customer;
  }
  return undefined;
}

export function putCustomer(customer) {
  customers.set(customer.customerId, customer);
}

export function allCustomers() {
  return [...customers.values()];
}
