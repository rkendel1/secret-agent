import { errors } from "@appport/protocol";
import { findByEmail, nextCustomerId, putCustomer } from "../store.mjs";

export async function registerCustomer(input, context) {
  if (findByEmail(input.email)) {
    throw errors.conflict(`a customer with email ${input.email} already exists`);
  }
  const customer = {
    customerId: nextCustomerId(),
    name: input.name,
    email: input.email,
    tier: input.tier,
    since: new Date().toISOString()
  };
  putCustomer(customer);
  await context.emit({ name: "customers.registered", version: 1, payload: { customerId: customer.customerId, email: customer.email } });
  return customer;
}
