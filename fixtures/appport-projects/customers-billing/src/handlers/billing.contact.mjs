import { lookupCustomer } from "./customers.lookup.mjs";

// Who to send a customer's invoices to.
export async function billingContact(input, context) {
  const customer = await lookupCustomer({ customerId: input.customerId }, context);
  return { customerId: customer.customerId, name: customer.name, email: customer.email };
}
