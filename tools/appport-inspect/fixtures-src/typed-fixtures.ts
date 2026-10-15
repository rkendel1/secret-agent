// Fixtures for shapes that no published package *produces* during inspection:
// verification evidence (produced by a caller's runtime), runtime evidence
// records and provider handover records. They are written here, but each one
// is type-checked against the published `.d.ts` files with `satisfies` before
// `inspect.mjs` emits it, so a fixture cannot drift from the real contract.

import type { VerificationEvidence } from "@appport/protocol";
import type {
  InvocationEvidence,
  StateAccessEvidence,
  ProviderHandoverRequest,
  ProviderHandoverRecord,
  ProviderHandoverRejection
} from "@appport/core";

export const verificationEvidence = [
  {
    type: "appport.verification-evidence/1",
    application: "com.example.orders",
    capability: { name: "orders.place", version: 1 },
    requestId: "req_01JABCDEFGHJKMNPQRSTVWXYZ0",
    checkedAt: "2026-10-15T12:00:00.000Z",
    outcome: "verified",
    methods: [
      {
        id: "order-exists",
        kind: "state",
        status: "confirmed",
        probe: { capability: "orders.get", version: 1, requestId: "req_01JABCDEFGHJKMNPQRSTVWXYZ1" }
      },
      { id: "placed-event", kind: "event", status: "not-run" }
    ]
  },
  {
    type: "appport.verification-evidence/1",
    application: "com.example.orders",
    capability: { name: "orders.cancel", version: 1 },
    checkedAt: "2026-10-15T12:05:00.000Z",
    outcome: "unverified",
    methods: [
      {
        id: "order-cancelled",
        kind: "state",
        status: "contradicted",
        probe: { capability: "orders.get", version: 1 },
        mismatches: [{ at: "/status", expected: "cancelled", observed: "placed" }]
      }
    ]
  },
  {
    type: "appport.verification-evidence/1",
    application: "com.example.orders",
    capability: { name: "orders.cancel", version: 1 },
    checkedAt: "2026-10-15T12:06:00.000Z",
    outcome: "unverifiable",
    methods: [{ id: "order-cancelled", kind: "state", status: "unavailable", errorCode: "FORBIDDEN" }]
  }
] satisfies VerificationEvidence[];

export const runtimeEvidence = [
  {
    type: "appport.evidence.invocation/1",
    observedAt: "2026-10-15T12:00:00.010Z",
    invocationId: "inv_01",
    traceId: "trace_01",
    parentRequestId: "req_01JABCDEFGHJKMNPQRSTVWXYZ0",
    caller: {
      application: "com.example.orders",
      capability: { name: "orders.place", version: 1 },
      attribution: "capability",
      requestId: "req_01JABCDEFGHJKMNPQRSTVWXYZ0"
    },
    target: { application: "com.example.inventory", capability: { name: "inventory.reserve", version: 1 } },
    outcome: "success"
  } satisfies InvocationEvidence,
  {
    type: "appport.evidence.invocation/1",
    observedAt: "2026-10-15T12:00:00.020Z",
    invocationId: "inv_02",
    caller: { application: "com.example.orders", attribution: "application-only" },
    target: { application: "com.example.notifications", capability: { name: "notifications.send", version: 2 } },
    outcome: "refused",
    detail: "binding-not-issued"
  } satisfies InvocationEvidence,
  {
    type: "appport.evidence.state-access/1",
    observedAt: "2026-10-15T12:00:00.030Z",
    sequence: 0,
    application: "com.example.orders",
    invocationId: "inv_root",
    requestId: "req_01JABCDEFGHJKMNPQRSTVWXYZ0",
    capability: { name: "orders.place", version: 1 },
    operation: "write",
    resource: { classification: "mapped", id: "urn:example:orders:order" },
    outcome: "ok"
  } satisfies StateAccessEvidence,
  {
    type: "appport.evidence.state-access/1",
    observedAt: "2026-10-15T12:00:00.040Z",
    sequence: 1,
    application: "com.example.orders",
    invocationId: "inv_root",
    operation: "query",
    resource: { classification: "unclassified" },
    outcome: "error"
  } satisfies StateAccessEvidence
];

export const handoverRequest = {
  capability: { name: "orders.place", version: 1 },
  from: { id: "com.example.orders", registrationId: "reg_old", fingerprint: "sha256:" + "a".repeat(64) },
  to: { id: "com.example.orders-v2", registrationId: "reg_new", fingerprint: "sha256:" + "b".repeat(64) }
} satisfies ProviderHandoverRequest;

export const handoverRecord = {
  handoverId: "hand_01",
  capability: { name: "orders.place", version: 1 },
  from: { ...handoverRequest.from, statusBefore: "active", statusAfter: "retired", revisionAfter: 4 },
  to: { ...handoverRequest.to, statusBefore: "registered", statusAfter: "active", revisionAfter: 2 },
  at: "2026-10-15T12:10:00.000Z"
} satisfies ProviderHandoverRecord;

export const handoverRejections = ["contract-mismatch", "to-not-validated"] satisfies ProviderHandoverRejection[];
