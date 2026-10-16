import { test } from "node:test";
import assert from "node:assert/strict";
import { lineTotal, subtotal, total } from "../src/invoice.js";

const invoice = (extra = {}) => ({
  lines: [
    { description: "Widget", unitPrice: 19.99, quantity: 3 },
    { description: "Gadget", unitPrice: 5.5, quantity: 2 }
  ],
  ...extra
});

test("line totals multiply price by quantity", () => {
  assert.equal(lineTotal({ unitPrice: 19.99, quantity: 3 }), 59.97);
});

test("the subtotal adds every line", () => {
  assert.equal(subtotal(invoice()), 70.97);
});

test("tax is applied to the subtotal", () => {
  assert.equal(total(invoice({ taxRate: 0.2 })), 85.16);
});

test("a percentage discount comes off before tax", () => {
  assert.equal(total(invoice({ discountPercent: 10, taxRate: 0.2 })), 76.65);
});
