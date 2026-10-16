import { lineTotal, total } from "./invoice.js";

/** One CSV row per line, then a total row. */
export function toCsv(invoice) {
  const rows = [["description", "quantity", "unit_price", "line_total"]];
  for (const line of invoice.lines) {
    rows.push([line.description, line.quantity, line.unitPrice.toFixed(2), lineTotal(line).toFixed(2)]);
  }
  rows.push(["total", "", "", total(invoice).toFixed(2)]);
  return rows.map((row) => row.map(escape).join(",")).join("\n");
}

function escape(value) {
  const text = String(value);
  return /[",\n]/.test(text) ? `"${text.replaceAll('"', '""')}"` : text;
}
