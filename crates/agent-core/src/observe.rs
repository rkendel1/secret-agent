//! Observation of an AppPort project on disk.
//!
//! A lightweight, static look at a JavaScript/TypeScript project built with the
//! published `@appport/*` packages: which packages it uses, which module creates
//! the application (`createApplication`), where each capability is declared
//! (`defineCapability`) and which module implements it (its `handler`), and how
//! the source modules import each other.
//!
//! This is deliberately not a language server. It reads the shapes the packages
//! define — `defineCapability({ name, version, handler })`, `createApplication(...)`
//! — and records what it could not resolve as [`Finding`]s instead of guessing.
//! The contract itself is never read from source: it is the manifest the
//! application's own runtime publishes.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use regex::Regex;
use serde::Serialize;

const SOURCE_EXTENSIONS: [&str; 6] = ["mjs", "js", "cjs", "mts", "ts", "cts"];
/// Packages that provide the application runtime (`createApplication`, `defineCapability`).
pub const RUNTIME_PACKAGES: [&str; 2] = ["@appport/core", "@appport/sdk"];
const SKIP_DIRECTORIES: [&str; 5] = ["node_modules", ".git", "target", "dist", "coverage"];

/// A project as observed on disk. Paths are relative to `root`, with `/` separators.
#[derive(Debug, Clone, Serialize)]
pub struct Project {
    pub root: PathBuf,
    /// `name` from `package.json`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    /// `@appport/*` dependencies and their declared versions.
    pub appport_packages: BTreeMap<String, String>,
    pub sources: Vec<SourceFile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application: Option<ApplicationEntry>,
    pub capabilities: Vec<CapabilityDeclaration>,
    pub findings: Vec<Finding>,
}

/// One source module.
#[derive(Debug, Clone, Serialize)]
pub struct SourceFile {
    pub path: String,
    pub bytes: usize,
    /// Project modules this one imports (resolved relative imports).
    pub imports: Vec<String>,
}

/// How to obtain the application: the module that calls `createApplication` and
/// the export that yields it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApplicationEntry {
    pub module: String,
    pub export: String,
    /// `factory` when the export is a function returning the application.
    pub kind: EntryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    Factory,
    Instance,
}

/// A `defineCapability({ ... })` call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapabilityDeclaration {
    pub name: String,
    pub version: u32,
    /// Module and line of the declaration.
    pub declared_in: String,
    pub line: usize,
    /// Where its handler is implemented, when it could be resolved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handler: Option<HandlerRef>,
}

/// A capability's implementation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HandlerRef {
    /// The module that defines the handler.
    pub module: String,
    /// The exported symbol, or `None` for an inline handler.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
}

/// Something observation could not resolve. Never fatal by itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub code: String,
    pub path: String,
    pub message: String,
}

impl Project {
    /// The declaration of `name@version`.
    pub fn capability(&self, name: &str, version: u32) -> Option<&CapabilityDeclaration> {
        self.capabilities.iter().find(|c| c.name == name && c.version == version)
    }

    pub fn source(&self, path: &str) -> Option<&SourceFile> {
        self.sources.iter().find(|s| s.path == path)
    }

    /// Modules that import `path`, directly.
    pub fn importers(&self, path: &str) -> Vec<&str> {
        self.sources.iter().filter(|s| s.imports.iter().any(|i| i == path)).map(|s| s.path.as_str()).collect()
    }

    /// Capabilities whose handler module is `path` or imports it (transitively):
    /// the capabilities whose behaviour a change to `path` can reach.
    pub fn capabilities_reaching(&self, path: &str) -> Vec<&CapabilityDeclaration> {
        let mut reached = BTreeSet::from([path.to_string()]);
        let mut frontier = vec![path.to_string()];
        while let Some(next) = frontier.pop() {
            for importer in self.importers(&next) {
                if reached.insert(importer.to_string()) {
                    frontier.push(importer.to_string());
                }
            }
        }
        self.capabilities.iter().filter(|c| c.handler.as_ref().is_some_and(|h| reached.contains(&h.module))).collect()
    }

    /// Total bytes of project source.
    pub fn source_bytes(&self) -> usize {
        self.sources.iter().map(|s| s.bytes).sum()
    }

    /// Reads a project file by its relative path. Refuses a path that leaves the
    /// root or passes through a symbolic link, so nothing outside the project is
    /// read (and sent to a model) through a link.
    pub fn read(&self, path: &str) -> io::Result<String> {
        let path = inside(&self.root, path)?;
        fs::read_to_string(path)
    }
}

/// Observes the project rooted at `root`.
pub fn observe(root: &Path) -> io::Result<Project> {
    let root = root.canonicalize()?;
    let mut findings = Vec::new();

    let (package, appport_packages) = match fs::read_to_string(root.join("package.json")) {
        Ok(text) => match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(json) => {
                let mut packages = BTreeMap::new();
                for key in ["dependencies", "devDependencies", "peerDependencies"] {
                    for (name, version) in json.get(key).and_then(|d| d.as_object()).into_iter().flatten() {
                        if name.starts_with("@appport/") {
                            packages.insert(name.clone(), version.as_str().unwrap_or_default().to_string());
                        }
                    }
                }
                (json.get("name").and_then(|n| n.as_str()).map(str::to_string), packages)
            }
            Err(e) => {
                findings.push(finding("package.invalid", "package.json", &format!("not JSON: {e}")));
                (None, BTreeMap::new())
            }
        },
        Err(_) => {
            findings.push(finding("package.missing", "package.json", "no package.json at the project root"));
            (None, BTreeMap::new())
        }
    };
    if appport_packages.is_empty() {
        findings.push(finding(
            "appport.absent",
            "package.json",
            "the project does not depend on any @appport/* package",
        ));
    } else if !RUNTIME_PACKAGES.iter().any(|p| appport_packages.contains_key(*p)) {
        findings.push(finding(
            "appport.runtime-absent",
            "package.json",
            "no @appport/core or @appport/sdk dependency: the project does not run an AppPort application runtime, so it publishes no manifest to observe",
        ));
    }

    let mut paths = Vec::new();
    let mut links = Vec::new();
    walk(&root, &root, &mut paths, &mut links)?;
    links.sort();
    for link in &links {
        findings.push(finding(
            "source.symlink",
            link,
            "a symbolic link; it is not observed, read or written (it may point outside the project)",
        ));
    }
    paths.sort();
    let known: BTreeSet<String> = paths.iter().cloned().collect();

    let mut sources = Vec::new();
    let mut texts = BTreeMap::new();
    for path in &paths {
        let raw = fs::read_to_string(root.join(path))?;
        let text = strip_comments(&raw);
        let imports = imports(&text)
            .into_iter()
            .filter_map(|i| resolve_module(path, &i.specifier, &known))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        sources.push(SourceFile { path: path.clone(), bytes: raw.len(), imports });
        texts.insert(path.clone(), text);
    }

    let mut application = None;
    let mut capabilities = Vec::new();
    for (path, text) in &texts {
        if let Some(entry) = application_entry(path, text) {
            match &application {
                None => application = Some(entry),
                Some(existing) => findings.push(finding(
                    "application.ambiguous",
                    path,
                    &format!("also calls createApplication; using {}#{}", existing.module, existing.export),
                )),
            }
        }
        for call in calls(text, "defineCapability") {
            match declaration(path, text, call, &known, &texts) {
                Ok(declaration) => capabilities.push(declaration),
                Err(message) => findings.push(finding("capability.unresolved", path, &message)),
            }
        }
    }
    if application.is_none() {
        findings.push(finding(
            "application.missing",
            "",
            "no module exports an application built with createApplication",
        ));
    }
    for declaration in &capabilities {
        if declaration.handler.is_none() {
            findings.push(finding(
                "handler.unresolved",
                &declaration.declared_in,
                &format!("could not resolve the handler of {}@{}", declaration.name, declaration.version),
            ));
        }
    }

    Ok(Project { root, package, appport_packages, sources, application, capabilities, findings })
}

fn finding(code: &str, path: &str, message: &str) -> Finding {
    Finding { code: code.into(), path: path.into(), message: message.into() }
}

/// The absolute path of `relative` under `root`, if it stays inside the root
/// and no existing component of it is a symbolic link.
pub fn inside(root: &Path, relative: &str) -> io::Result<std::path::PathBuf> {
    let denied = |why: String| io::Error::new(io::ErrorKind::PermissionDenied, why);
    let normalized =
        normalize(relative).ok_or_else(|| denied(format!("{relative} is not a path inside the project")))?;
    let mut path = root.to_path_buf();
    for part in normalized.split('/') {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(denied(format!("{relative} passes through a symbolic link ({})", path.display())))
            }
            _ => {}
        }
    }
    Ok(path)
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>, links: &mut Vec<String>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            // Dependencies are commonly linked; anything else is reported, never followed.
            if !SKIP_DIRECTORIES.contains(&name.as_str()) && !name.starts_with('.') {
                links.push(relative(root, &path));
            }
        } else if kind.is_dir() {
            if !SKIP_DIRECTORIES.contains(&name.as_str()) && !name.starts_with('.') {
                walk(root, &path, out, links)?;
            }
        } else if path.extension().and_then(|e| e.to_str()).is_some_and(|e| SOURCE_EXTENSIONS.contains(&e))
            && !name.ends_with(".d.ts")
        {
            out.push(relative(root, &path));
        }
    }
    Ok(())
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

/// Normalizes a project-relative path (`a/./b/../c` → `a/c`). `None` if it escapes the root.
pub fn normalize(path: &str) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().to_string()),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn resolve_module(from: &str, specifier: &str, known: &BTreeSet<String>) -> Option<String> {
    if !specifier.starts_with("./") && !specifier.starts_with("../") {
        return None;
    }
    let base = Path::new(from).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let joined = normalize(&format!("{base}/{specifier}"))?;
    let candidates = std::iter::once(joined.clone())
        .chain(SOURCE_EXTENSIONS.iter().map(|e| format!("{joined}.{e}")))
        .chain(SOURCE_EXTENSIONS.iter().map(|e| format!("{joined}/index.{e}")))
        // TypeScript sources imported with their emitted `.js` name.
        .chain(joined.strip_suffix(".js").map(|s| format!("{s}.ts")));
    candidates.into_iter().find(|c| known.contains(c))
}

// ---------------------------------------------------------------------------
// A small JavaScript scanner: enough to read call arguments, never executed.
// ---------------------------------------------------------------------------

/// Replaces comments with spaces, keeping byte offsets and string literals intact.
fn strip_comments(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = bytes.to_vec();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' | b'\'' | b'`' => i = skip_string(bytes, i),
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    out[i] = b' ';
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let end = text[i + 2..].find("*/").map_or(bytes.len(), |e| i + 2 + e + 2);
                for b in &mut out[i..end] {
                    if *b != b'\n' {
                        *b = b' ';
                    }
                }
                i = end;
            }
            _ => i += 1,
        }
    }
    // Only ASCII bytes inside comments were replaced, so this stays valid UTF-8
    // unless a comment held multi-byte characters; fall back to lossless in that case.
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// Index just past the string literal starting at `start`.
fn skip_string(bytes: &[u8], start: usize) -> usize {
    let quote = bytes[start];
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b if b == quote => return i + 1,
            _ => i += 1,
        }
    }
    bytes.len()
}

/// Offsets of the `(` of every call to `name`.
fn calls(text: &str, name: &str) -> Vec<usize> {
    let pattern = Regex::new(&format!(r"(^|[^\w$.]){}\s*\(", regex::escape(name))).expect("valid regex");
    pattern.find_iter(text).map(|m| m.end() - 1).collect()
}

/// The matching close of the bracket at `open`.
fn close_of(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'"' | b'\'' | b'`' => {
                i = skip_string(bytes, i);
                continue;
            }
            b'(' | b'{' | b'[' => depth += 1,
            b')' | b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Top-level members of the object literal spanning `open..=close`: `(key, value)`.
fn members(text: &str, open: usize, close: usize) -> Vec<(String, String)> {
    let bytes = text.as_bytes();
    let mut parts = Vec::new();
    let mut start = open + 1;
    let mut i = open + 1;
    while i < close {
        match bytes[i] {
            b'"' | b'\'' | b'`' => {
                i = skip_string(bytes, i);
                continue;
            }
            b'(' | b'{' | b'[' => {
                i = close_of(bytes, i).map_or(close, |c| c + 1);
                continue;
            }
            b',' => {
                parts.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(&text[start..close]);
    let key =
        Regex::new(r#"^\s*(?:async\s+)?(?:([A-Za-z_$][\w$]*)|"([^"]*)"|'([^']*)')\s*(:|\()?"#).expect("valid regex");
    parts
        .into_iter()
        .filter(|p| !p.trim().is_empty())
        .filter_map(|part| {
            let captures = key.captures(part)?;
            let name = captures.get(1).or(captures.get(2)).or(captures.get(3))?.as_str().to_string();
            let value = match captures.get(4).map(|m| m.as_str()) {
                Some(":") => part[captures.get(0)?.end()..].trim().to_string(),
                // Method shorthand: `handler(input) { ... }`.
                Some("(") => format!("function {}", part.trim()),
                // Property shorthand: `{ handler }`.
                _ => name.clone(),
            };
            Some((name, value))
        })
        .collect()
}

struct Import {
    specifier: String,
    /// local name → exported name (`default` for a default import, `*` for a namespace).
    bindings: Vec<(String, String)>,
}

fn imports(text: &str) -> Vec<Import> {
    let statement =
        Regex::new(r#"import\s+(?:([\w$]+)\s*,?\s*)?(?:\*\s+as\s+([\w$]+)\s*)?(?:\{([^}]*)\})?\s*from\s*["']([^"']+)["']|import\s*["']([^"']+)["']"#)
            .expect("valid regex");
    statement
        .captures_iter(text)
        .map(|c| {
            if let Some(bare) = c.get(5) {
                return Import { specifier: bare.as_str().to_string(), bindings: Vec::new() };
            }
            let mut bindings = Vec::new();
            if let Some(default) = c.get(1) {
                bindings.push((default.as_str().to_string(), "default".to_string()));
            }
            if let Some(namespace) = c.get(2) {
                bindings.push((namespace.as_str().to_string(), "*".to_string()));
            }
            for named in c.get(3).map(|m| m.as_str()).unwrap_or_default().split(',') {
                let named = named.trim().trim_start_matches("type ").trim();
                if named.is_empty() {
                    continue;
                }
                match named.split_once(" as ") {
                    Some((exported, local)) => bindings.push((local.trim().to_string(), exported.trim().to_string())),
                    None => bindings.push((named.to_string(), named.to_string())),
                }
            }
            Import { specifier: c[4].to_string(), bindings }
        })
        .collect()
}

fn line_of(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())].bytes().filter(|b| *b == b'\n').count() + 1
}

fn string_literal(value: &str) -> Option<String> {
    let value = value.trim();
    let quote = value.chars().next().filter(|c| matches!(c, '"' | '\'' | '`'))?;
    let inner = value.strip_prefix(quote)?.strip_suffix(quote)?;
    (!inner.contains(quote) && !inner.contains("${")).then(|| inner.to_string())
}

fn declaration(
    path: &str,
    text: &str,
    call: usize,
    known: &BTreeSet<String>,
    texts: &BTreeMap<String, String>,
) -> Result<CapabilityDeclaration, String> {
    let line = line_of(text, call);
    let bytes = text.as_bytes();
    let close_call = close_of(bytes, call).ok_or_else(|| format!("line {line}: unterminated defineCapability call"))?;
    let open = text[call..close_call]
        .find('{')
        .map(|o| call + o)
        .ok_or_else(|| format!("line {line}: defineCapability is not called with an object literal"))?;
    let close = close_of(bytes, open).ok_or_else(|| format!("line {line}: unterminated object literal"))?;
    let members: BTreeMap<String, String> = members(text, open, close).into_iter().collect();

    let name = members
        .get("name")
        .and_then(|v| string_literal(v))
        .ok_or_else(|| format!("line {line}: capability name is not a string literal"))?;
    let version = members
        .get("version")
        .and_then(|v| v.trim().parse::<u32>().ok())
        .ok_or_else(|| format!("line {line}: version of {name} is not an integer literal"))?;

    let handler = members.get("handler").and_then(|value| {
        let value = value.trim();
        let identifier = Regex::new(r"^[A-Za-z_$][\w$]*(\.[A-Za-z_$][\w$]*)?$").expect("valid regex");
        if !identifier.is_match(value) {
            // An inline function: implemented in the declaring module.
            return Some(HandlerRef { module: path.to_string(), symbol: None });
        }
        let (local, member) = match value.split_once('.') {
            Some((namespace, member)) => (namespace, Some(member)),
            None => (value, None),
        };
        for import in imports(text) {
            for (bound, exported) in &import.bindings {
                if bound == local {
                    let module = resolve_module(path, &import.specifier, known)?;
                    let symbol = match (exported.as_str(), member) {
                        ("*", Some(member)) => member.to_string(),
                        _ => exported.clone(),
                    };
                    return Some(HandlerRef { module, symbol: Some(symbol) });
                }
            }
        }
        // Defined in this module.
        let local_definition =
            Regex::new(&format!(r"(function\s+{0}\s*\(|(const|let|var)\s+{0}\s*=)", regex::escape(local))).ok()?;
        local_definition
            .is_match(texts.get(path)?)
            .then(|| HandlerRef { module: path.to_string(), symbol: Some(local.to_string()) })
    });

    Ok(CapabilityDeclaration { name, version, declared_in: path.to_string(), line, handler })
}

fn application_entry(path: &str, text: &str) -> Option<ApplicationEntry> {
    let create = *calls(text, "createApplication").first()?;
    let exports = Regex::new(
        r"export\s+(?:default\s+)?(?:(async\s+)?function\s*\*?\s*([\w$]+)|(?:const|let|var)\s+([\w$]+)\s*=)",
    )
    .expect("valid regex");
    // The export whose definition encloses (or is) the createApplication call.
    let export = exports.captures_iter(text).filter(|c| c.get(0).is_some_and(|m| m.start() < create)).last()?;
    let (name, kind) = match (export.get(2), export.get(3)) {
        (Some(function), _) => (function.as_str(), EntryKind::Factory),
        (None, Some(binding)) => {
            let rest = text[export.get(0)?.end()..].trim_start();
            let kind = if rest.starts_with("createApplication") { EntryKind::Instance } else { EntryKind::Factory };
            (binding.as_str(), kind)
        }
        _ => return None,
    };
    Some(ApplicationEntry { module: path.to_string(), export: name.to_string(), kind })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_top_level_members_only() {
        let text = r#"defineCapability({
  name: "orders.place", // trailing comment, with a comma
  version: 2,
  integration: { verify: [{ via: { capability: "orders.get", version: 1 } }] },
  handler: place
});"#;
        let call = calls(text, "defineCapability")[0];
        let open = text.find('{').unwrap();
        let close = close_of(text.as_bytes(), open).unwrap();
        let members: BTreeMap<String, String> = members(&strip_comments(text), open, close).into_iter().collect();
        assert_eq!(call, "defineCapability".len());
        assert_eq!(members["name"], "\"orders.place\"");
        assert_eq!(members["version"], "2");
        assert_eq!(members["handler"].trim(), "place");
        assert!(members["integration"].starts_with('{'));
    }

    #[test]
    fn parses_import_forms() {
        let found = imports(
            r#"import def, { a, b as c, type T } from "./x.mjs";
import * as ns from '../y.js';
import "./side-effect.mjs";"#,
        );
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].specifier, "./x.mjs");
        assert_eq!(
            found[0].bindings,
            [
                ("def".into(), "default".into()),
                ("a".into(), "a".into()),
                ("c".into(), "b".into()),
                ("T".into(), "T".into())
            ]
        );
        assert_eq!(found[1].bindings, [("ns".into(), "*".into())]);
        assert!(found[2].bindings.is_empty());
    }

    #[test]
    fn normalizes_paths_inside_the_root() {
        assert_eq!(normalize("src/./a/../b.mjs").as_deref(), Some("src/b.mjs"));
        assert_eq!(normalize("../outside.mjs"), None);
        assert_eq!(normalize("/etc/passwd"), None);
    }

    #[test]
    fn finds_the_application_export() {
        let factory = "export function createShop() {\n  return createApplication({});\n}";
        assert_eq!(
            application_entry("src/app.mjs", factory),
            Some(ApplicationEntry {
                module: "src/app.mjs".into(),
                export: "createShop".into(),
                kind: EntryKind::Factory
            })
        );
        let instance = "export const app = createApplication({});";
        assert_eq!(application_entry("a.mjs", instance).unwrap().kind, EntryKind::Instance);
        assert_eq!(application_entry("a.mjs", "const x = 1;"), None);
    }

    #[cfg(unix)]
    #[test]
    fn never_follows_a_symbolic_link() {
        let base = std::env::temp_dir().join(format!("agent-core-observe-link-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (root, outside) = (base.join("project"), base.join("outside"));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(root.join("src/a.mjs"), "export const a = 1;\n").unwrap();
        fs::write(outside.join("secret.mjs"), "export const key = 'outside';\n").unwrap();
        std::os::unix::fs::symlink(outside.join("secret.mjs"), root.join("src/b.mjs")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("src/linked")).unwrap();
        let project = observe(&root).unwrap();
        assert_eq!(project.sources.iter().map(|s| s.path.as_str()).collect::<Vec<_>>(), ["src/a.mjs"]);
        let links: Vec<&str> =
            project.findings.iter().filter(|f| f.code == "source.symlink").map(|f| f.path.as_str()).collect();
        assert_eq!(links, ["src/b.mjs", "src/linked"]);
        for path in ["src/b.mjs", "src/linked/secret.mjs", "../outside/secret.mjs"] {
            assert!(project.read(path).is_err(), "{path} was read");
        }
        assert!(project.read("src/a.mjs").is_ok());
        fs::remove_dir_all(base).unwrap();
    }
}
