/** A customer's display name: "Company (Contact)" when both are known. */
export function displayName(customer) {
  if (customer.company && customer.contact) return `${customer.company} (${customer.contact})`;
  return customer.company ?? customer.contact ?? "Unknown customer";
}

/** Whether the customer is on a plan that grants a standing discount. */
export function hasStandingDiscount(customer) {
  return customer.plan === "partner" || customer.plan === "enterprise";
}
