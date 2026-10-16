/**
 * A URL slug: lowercase ASCII letters and digits separated by single hyphens.
 * Accents are dropped ("Café" -> "cafe"); anything else separates words.
 *
 * With `maxLength`, the slug is cut to at most that many characters at a word
 * boundary; a first word longer than `maxLength` is cut inside the word.
 */
export function slugify(text, { maxLength } = {}) {
  const slug = text
    .normalize("NFKD")
    .replace(/[̀-ͯ]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  if (maxLength === undefined || slug.length <= maxLength) return slug;
  const cut = slug.slice(0, maxLength + 1);
  const boundary = cut.lastIndexOf("-");
  return (boundary > 0 ? cut.slice(0, boundary) : slug.slice(0, maxLength)).replace(/-+$/, "");
}
