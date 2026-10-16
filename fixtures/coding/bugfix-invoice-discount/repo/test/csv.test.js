import { test } from "node:test";
import assert from "node:assert/strict";
import { toCsv } from "../src/csv.js";

test("exports one row per line and a total", () => {
  const csv = toCsv({ lines: [{ description: "Bolt, M4", unitPrice: 0.1, quantity: 10 }] });
  assert.equal(csv, 'description,quantity,unit_price,line_total\n"Bolt, M4",10,0.10,1.00\ntotal,,,1.00');
});
