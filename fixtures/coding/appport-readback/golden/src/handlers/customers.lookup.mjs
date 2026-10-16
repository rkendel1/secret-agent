import { errors } from "@appport/protocol";
import { getCustomer } from "../store.mjs";

// Reads the application's customer store, the system of record.
export async function lookupCustomer(input) {
  const customer = getCustomer(input.customerId);
  if (!customer) {
    throw errors.notFound(`no customer ${input.customerId}`);
  }
  return { ...customer };
}
