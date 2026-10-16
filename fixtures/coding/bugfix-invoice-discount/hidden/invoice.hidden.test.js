import { test } from "node:test";
import assert from "node:assert/strict";
import { total } from "../src/invoice.js";

const lines = [{ description: "Service", unitPrice: 200, quantity: 1 }];

test("no discount leaves the total unchanged", () => {
  assert.equal(total({ lines, taxRate: 0.1 }), 220);
  assert.equal(total({ lines, discountPercent: 0, taxRate: 0.1 }), 220);
});

test("a full discount makes the invoice free", () => {
  assert.equal(total({ lines, discountPercent: 100, taxRate: 0.1 }), 0);
});

test("a discount without tax", () => {
  assert.equal(total({ lines, discountPercent: 25 }), 150);
});

test("fractional percentages", () => {
  assert.equal(total({ lines, discountPercent: 12.5, taxRate: 0.2 }), 210);
});
