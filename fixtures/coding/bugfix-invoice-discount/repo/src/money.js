// Money is handled in integer cents internally and rounded half away from zero.

export function roundCents(amount) {
  return Math.sign(amount) * Math.round(Math.abs(amount) * 100) / 100;
}

export function formatMoney(amount, currency = "USD") {
  return new Intl.NumberFormat("en-US", { style: "currency", currency }).format(amount);
}
