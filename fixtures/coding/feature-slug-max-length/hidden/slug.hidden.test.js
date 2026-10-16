import { test } from "node:test";
import assert from "node:assert/strict";
import { slugify } from "../src/text/slug.js";

test("without maxLength nothing changes", () => {
  assert.equal(slugify("The quick brown fox jumps"), "the-quick-brown-fox-jumps");
  assert.equal(slugify("The quick brown fox jumps", {}), "the-quick-brown-fox-jumps");
});

test("a slug that already fits is returned whole", () => {
  assert.equal(slugify("short title", { maxLength: 11 }), "short-title");
  assert.equal(slugify("short title", { maxLength: 50 }), "short-title");
});

test("a single word longer than maxLength is cut, since it cannot be avoided", () => {
  assert.equal(slugify("Supercalifragilistic", { maxLength: 5 }), "super");
});

test("the result is never longer than maxLength and never ends with a hyphen", () => {
  for (let n = 1; n <= 30; n++) {
    const slug = slugify("Crème brûlée à Paris, the best in town", { maxLength: n });
    assert.ok(slug.length <= n, `length ${slug.length} > ${n}`);
    assert.ok(!slug.endsWith("-"), `"${slug}" ends with a hyphen`);
  }
});
