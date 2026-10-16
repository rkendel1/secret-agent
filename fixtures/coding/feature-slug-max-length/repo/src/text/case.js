/** "hello world" -> "Hello World" */
export function titleCase(text) {
  return text.replace(/\b\w/g, (c) => c.toUpperCase());
}

/** "hello world" -> "helloWorld" */
export function camelCase(text) {
  return text.toLowerCase().replace(/[^a-z0-9]+(.)/g, (_, c) => c.toUpperCase());
}
