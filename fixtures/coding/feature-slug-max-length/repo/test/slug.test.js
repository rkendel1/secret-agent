import { test } from "node:test";
import assert from "node:assert/strict";
import { slugify } from "../src/text/slug.js";

test("lowercases and joins words with hyphens", () => {
  assert.equal(slugify("Hello, World!"), "hello-world");
});

test("drops accents", () => {
  assert.equal(slugify("Crème brûlée à Paris"), "creme-brulee-a-paris");
});

test("trims separators at both ends", () => {
  assert.equal(slugify("  --Rust & WebAssembly--  "), "rust-webassembly");
});

test("maxLength keeps whole words", () => {
  assert.equal(slugify("The quick brown fox jumps", { maxLength: 15 }), "the-quick-brown");
  assert.equal(slugify("The quick brown fox jumps", { maxLength: 18 }), "the-quick-brown");
});

test("maxLength never leaves a trailing hyphen", () => {
  assert.equal(slugify("alpha beta gamma", { maxLength: 11 }), "alpha-beta");
});
