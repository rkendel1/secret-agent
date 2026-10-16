import { permissionAuthorizer } from "@appport/authorization";
import { createApplication } from "@appport/core";

import { CUSTOMER, CUSTOMER_RESOURCE, contact, list, lookup, register, registered } from "./contract.mjs";

export function createCustomersApplication() {
  return createApplication({
    application: {
      id: "com.example.customers",
      name: "Customers",
      version: "1.0.0",
      description: "Customer records for the PR 2 agent demo."
    },
    capabilities: [register, lookup, list, contact],
    events: [registered],
    authorizer: permissionAuthorizer(),
    transports: [{ kind: "ipc" }],
    resources: [{ id: CUSTOMER_RESOURCE, kind: "entity", concept: CUSTOMER, owner: "self", description: "A customer record." }],
    extensions: ["appport.integration/1"]
  });
}
