/**
 * A URL slug: lowercase ASCII letters and digits separated by single hyphens.
 * Accents are dropped ("Café" -> "cafe"); anything else separates words.
 */
export function slugify(text) {
  return text
    .normalize("NFKD")
    .replace(/[̀-ͯ]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}
