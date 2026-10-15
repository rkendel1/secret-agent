// An integration-rich reference application, built with the published AppPort
// runtime (`createApplication`, `defineCapability`, `@appport/schema`) so its
// manifest is produced by AppPort itself rather than written by hand.
//
// It exists to exercise every part of the integration descriptor the Rust
// knowledge module models: semantic annotations, effects, verification methods,
// dependencies (local and cross-application), resources and access, coverage,
// approval, preconditions, failure declarations and mapping references.
// `inspect.mjs` refuses to emit it unless the official linters report no errors.

// Namespace imports, so a package that stops exporting something fails with a
// drift report from inspect.mjs instead of a module-link error.
import * as core from "@appport/core";
import * as protocol from "@appport/protocol";
import * as schema from "@appport/schema";

const { createApplication, defineCapability, defineEvent } = core;
const { INTEGRATION_EXTENSION, digestOf } = protocol;
const { s, annotate, semantic } = schema;

const ORDER = "https://schema.example.com/commerce/Order";
const status = annotate(
  s.enum(["pending", "placed", "cancelled"]),
  semantic({
    concept: "https://schema.example.com/commerce/OrderStatus",
    enumConcepts: {
      pending: "https://schema.example.com/commerce/OrderStatus#pending",
      placed: "https://schema.example.com/commerce/OrderStatus#placed",
      cancelled: "https://schema.example.com/commerce/OrderStatus#cancelled"
    }
  })
);
const total = annotate(s.number(), semantic({ concept: "https://schema.example.com/commerce/OrderTotal", unit: "[USD]" }));
const placedAt = annotate(s.string(), semantic({ concept: "https://schema.example.com/commerce/placedAt", encoding: "iso8601" }));
const orderId = annotate(s.string(), semantic({ concept: ORDER }));

const order = s.object({ orderId, status, total, placedAt });

const noop = () => {
  throw new Error("fixture handler: never invoked during inspection");
};

const get = defineCapability({
  name: "orders.get",
  version: 1,
  description: "Reads one order.",
  input: s.object({ orderId }),
  output: order,
  authorization: ["orders.read"],
  effect: "observation",
  idempotency: "supported",
  integration: {
    purpose: "Look up the current state of an order.",
    authority: "https://schema.example.com/authority/orders",
    access: [{ resource: "urn:example:orders:order", mode: "reads" }],
    coverage: { requires: "complete", access: "complete" }
  },
  handler: noop
});

const findByRef = defineCapability({
  name: "orders.byref",
  version: 1,
  description: "Finds an order by the client's reference.",
  input: s.object({ clientRef: s.string() }),
  output: s.object({ orderId }),
  authorization: ["orders.read"],
  effect: "observation",
  idempotency: "supported",
  integration: {
    access: [{ resource: "urn:example:orders:order", mode: "reads" }]
  },
  handler: noop
});

const placed = defineEvent({
  name: "orders.placed",
  version: 1,
  description: "An order was placed.",
  payload: s.object({ orderId, total }),
  authorization: ["orders.read"]
});

const place = defineCapability({
  name: "orders.place",
  version: 1,
  description: "Places an order.",
  input: s.object({ clientRef: s.string(), sku: s.string(), quantity: s.integer(), note: s.optional(s.string()) }),
  output: order,
  authorization: ["orders.write"],
  effect: "consequential",
  idempotency: "supported",
  concurrency: { mode: "keyed", key: "clientRef" },
  timeoutMs: 10000,
  emits: ["orders.placed"],
  integration: {
    purpose: "Create a customer order and reserve stock.",
    authority: "https://schema.example.com/authority/orders",
    effects: [
      { type: "https://schema.example.com/effects/order-created", description: "A new order exists." },
      { type: "https://schema.example.com/effects/stock-reserved" }
    ],
    verify: [
      {
        id: "order-exists",
        kind: "state",
        via: { capability: "orders.get", version: 1, input: { orderId: { $output: "/orderId" } } },
        expect: [
          { at: "/status", equals: "placed" },
          { at: "/total", exists: true }
        ]
      },
      {
        id: "placed-event",
        kind: "event",
        event: "orders.placed",
        match: { "/orderId": { $output: "/orderId" } },
        strength: "corroborating"
      }
    ],
    requires: [
      {
        id: "stock",
        relationship: "invokes",
        target: { application: "com.example.inventory", capability: "inventory.reserve@1" },
        required: true,
        purpose: "Reserve stock for the order."
      },
      {
        id: "lookup",
        relationship: "observes",
        target: { capability: "orders.get@1" },
        required: false
      },
      {
        id: "pricing",
        relationship: "consumes-event",
        target: { application: "com.example.pricing", event: "pricing.changed" },
        required: false,
        versionConstraint: "^1"
      }
    ],
    access: [
      { resource: "urn:example:orders:order", mode: "changes", operations: ["create"] },
      { resource: "urn:example:inventory:stock", mode: "changes", operations: ["update"] }
    ],
    coverage: { requires: "complete", access: "partial", preconditions: "complete", failures: "partial" },
    approval: { required: true, approver: "https://schema.example.com/roles/order-desk" },
    preconditions: [
      {
        id: "no-duplicate",
        via: { capability: "orders.byref", version: 1, input: { clientRef: { $input: "/clientRef" } } },
        require: "not-found"
      }
    ],
    failures: [
      { code: "CONFLICT", retry: "after-verification", effect: "possible", description: "Stock changed mid-request." },
      { code: "INVALID_INPUT", retry: "unsafe", effect: "none" }
    ]
  },
  handler: noop
});

const cancel = defineCapability({
  name: "orders.cancel",
  version: 1,
  description: "Cancels an order.",
  input: s.object({ orderId }),
  output: order,
  authorization: ["orders.write"],
  effect: "consequential",
  idempotency: "unsupported",
  integration: {
    authority: "https://schema.example.com/authority/orders",
    effects: [{ type: "https://schema.example.com/effects/order-cancelled" }],
    verify: [
      {
        id: "order-cancelled",
        kind: "state",
        via: { capability: "orders.get", version: 1, input: { orderId: { $input: "/orderId" } } },
        expect: [{ at: "/status", equals: "cancelled" }]
      }
    ],
    requires: [
      {
        id: "read-back",
        relationship: "observes",
        target: { capability: "orders.get" },
        versionConstraint: ">=1",
        required: true
      }
    ],
    access: [{ resource: "urn:example:orders:order", mode: "changes", operations: ["update"] }],
    approval: { required: false },
    preconditions: [
      {
        id: "exists",
        via: { capability: "orders.get", version: 1, input: { orderId: { $input: "/orderId" } } },
        require: "found",
        expect: [{ at: "/status", equals: "placed" }]
      }
    ],
    failures: [{ code: "NOT_FOUND", retry: "safe", effect: "none" }]
  },
  handler: noop
});

const mappingArtifact = { id: "urn:example:mapping:legacy-order", version: 1, from: "legacy.order", to: "orders.place@1", fields: { ref: "/clientRef", item: "/sku", qty: "/quantity" } };

export function createOrdersApplication() {
  return createApplication({
    application: {
      id: "com.example.orders",
      name: "Orders",
      version: "3.4.0",
      description: "Integration-rich reference application for the Rust knowledge module."
    },
    capabilities: [get, findByRef, place, cancel],
    events: [placed],
    transports: [{ kind: "http", endpoint: "/appport" }],
    resources: [
      { id: "urn:example:orders:order", kind: "entity", concept: ORDER, owner: "self", description: "A customer order." },
      { id: "urn:example:inventory:stock", kind: "counter", owner: { application: "com.example.inventory" } }
    ],
    requires: [
      {
        id: "notify",
        relationship: "delegates",
        target: { application: "com.example.notifications", capability: "notifications.send@2" },
        required: false
      }
    ],
    mappings: [
      {
        id: "urn:example:mapping:legacy-order",
        version: 1,
        digest: digestOf(mappingArtifact),
        direction: "inbound",
        for: "orders.place@1",
        peer: { application: "com.example.legacy" },
        origin: "provider",
        artifact: mappingArtifact
      }
    ],
    extensions: [INTEGRATION_EXTENSION]
  });
}
