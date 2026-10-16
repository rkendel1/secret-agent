import { test } from "node:test";
import assert from "node:assert/strict";
import { postPath, displayTitle } from "../src/posts.js";

test("post paths use the date and the slug", () => {
  assert.equal(postPath({ title: "Hello World", publishedAt: "2026-03-04T10:00:00Z" }), "/2026/03/04/hello-world");
});

test("display titles are title-cased", () => {
  assert.equal(displayTitle({ title: " a day out " }), "A Day Out");
});
