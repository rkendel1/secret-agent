import { slugify } from "./text/slug.js";
import { titleCase } from "./text/case.js";

/** The public path of a post. */
export function postPath(post) {
  const date = post.publishedAt.slice(0, 10).replaceAll("-", "/");
  return `/${date}/${slugify(post.title)}`;
}

/** The title as shown in listings. */
export function displayTitle(post) {
  return titleCase(post.title.trim());
}
