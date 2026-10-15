import { allCustomers } from "../store.mjs";

export async function listCustomers() {
  return { customers: allCustomers().map(({ customerId, name }) => ({ customerId, name })) };
}
