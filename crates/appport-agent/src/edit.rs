//! Reading the model's proposed change and applying it within the boundary.
//!
//! The model answers with whole files (see [`agent_core::plan::FILE_OPEN`]). A
//! file outside the plan's boundary is never written: the change is rejected
//! before anything runs. Every applied change can be rolled back exactly.

use std::fs;
use std::path::Path;

use agent_core::observe::{inside, normalize};
use agent_core::plan::{FILE_CLOSE, FILE_OPEN};

/// One file the model wants to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEdit {
    pub path: String,
    pub contents: String,
}

/// Parses `=== FILE: <path> ===` … `=== END FILE ===` blocks. A Markdown fence
/// wrapped around a file's contents is removed.
pub fn parse(text: &str) -> Result<Vec<FileEdit>, String> {
    let mut edits = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let Some(rest) = line.trim().strip_prefix(FILE_OPEN.trim_end()) else { continue };
        let path = rest.trim().trim_end_matches('=').trim().to_string();
        let mut body: Vec<&str> = Vec::new();
        let mut closed = false;
        for line in lines.by_ref() {
            if line.trim() == FILE_CLOSE {
                closed = true;
                break;
            }
            body.push(line);
        }
        if !closed {
            return Err(format!("the block for {path} has no {FILE_CLOSE} line"));
        }
        if body.first().is_some_and(|l| l.trim_start().starts_with("```"))
            && body.last().is_some_and(|l| l.trim() == "```")
        {
            body.remove(0);
            body.pop();
        }
        let mut contents = body.join("\n");
        contents.push('\n');
        edits.push(FileEdit { path, contents });
    }
    if edits.is_empty() {
        return Err(format!("the response contains no `{FILE_OPEN}<path> ===` block"));
    }
    Ok(edits)
}

/// A response with no file block: if exactly one file is editable and the
/// response holds exactly one fenced code block, that block is the file.
/// Anything less certain stays unusable.
pub fn single_fenced(text: &str, editable: &[String]) -> Option<FileEdit> {
    let [path] = editable else { return None };
    let mut blocks = Vec::new();
    let mut current: Option<Vec<&str>> = None;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            match current.take() {
                Some(body) => blocks.push(body),
                None => current = Some(Vec::new()),
            }
        } else if let Some(body) = current.as_mut() {
            body.push(line);
        }
    }
    let [body] = blocks.as_slice() else { return None };
    let mut contents = body.join("\n");
    contents.push('\n');
    Some(FileEdit { path: path.clone(), contents })
}

/// The files an edit set touches, normalized; `Err` for a path outside the project.
pub fn targets(edits: &[FileEdit]) -> Result<Vec<String>, String> {
    edits
        .iter()
        .map(|e| normalize(&e.path).ok_or_else(|| format!("{} is not a path inside the project", e.path)))
        .collect()
}

/// One file an [`apply`] wrote.
#[derive(Debug, Clone)]
struct Written {
    path: String,
    /// The bytes before the write; `None` when the file did not exist.
    original: Option<Vec<u8>>,
    /// The bytes the agent wrote.
    wrote: Vec<u8>,
}

/// Originals of applied files, for rollback.
#[derive(Debug, Default)]
pub struct Applied {
    written: Vec<Written>,
    /// Files whose contents actually changed.
    pub changed: Vec<String>,
}

impl Applied {
    /// Per changed file: `(path, existed before, sha256 before, sha256 after)`.
    pub fn digests(&self) -> Vec<(String, bool, Option<String>, String)> {
        use agent_core::appport::knowledge::rules::sha256_bytes_hex;
        self.written
            .iter()
            .map(|w| {
                (
                    w.path.clone(),
                    w.original.is_some(),
                    w.original.as_deref().map(sha256_bytes_hex),
                    sha256_bytes_hex(&w.wrote),
                )
            })
            .collect()
    }

    /// The original text of each changed file that existed (for a recovery record).
    pub fn originals(&self) -> Vec<(String, Option<String>)> {
        self.written
            .iter()
            .map(|w| (w.path.clone(), w.original.as_ref().map(|b| String::from_utf8_lossy(b).into_owned())))
            .collect()
    }
}

/// What a rollback did, file by file. A rollback is only reported as done when
/// every file was checked after it was restored.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
#[must_use = "a rollback is only done when it is checked"]
pub struct Rollback {
    /// Restored, and checked to hold the original bytes (or to be absent again).
    pub restored: Vec<String>,
    /// Not touched: the file changed after the agent wrote it, so restoring it
    /// would overwrite someone else's work.
    pub modified_since: Vec<String>,
    /// Could not be restored, or did not check out after restoring.
    pub failed: Vec<String>,
}

impl Rollback {
    /// Every file is back as it was.
    pub fn verified(&self) -> bool {
        self.modified_since.is_empty() && self.failed.is_empty()
    }

    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if !self.restored.is_empty() {
            parts.push(format!("restored and checked: {}", self.restored.join(", ")));
        }
        if !self.modified_since.is_empty() {
            parts.push(format!(
                "left alone because they changed after the agent wrote them: {}",
                self.modified_since.join(", ")
            ));
        }
        if !self.failed.is_empty() {
            parts.push(format!("NOT restored: {}", self.failed.join("; ")));
        }
        parts.join("; ")
    }
}

/// Writes `bytes` to `path` through a temporary file in the same directory and
/// a rename, so the file is never left half-written.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let temp = path.with_file_name(format!(".{name}.appport-agent-{}.tmp", std::process::id()));
    let result = fs::write(&temp, bytes).and_then(|_| {
        if let Ok(meta) = fs::metadata(path) {
            let _ = fs::set_permissions(&temp, meta.permissions());
        }
        fs::rename(&temp, path)
    });
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Reads the current bytes of a file: `Ok(None)` when it does not exist.
fn current(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Why an [`apply`] failed.
#[derive(Debug, Clone)]
pub struct ApplyError {
    pub message: String,
    /// When some files had been written: the rollback of those writes.
    pub rollback: Option<Rollback>,
}

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)?;
        if let Some(r) = &self.rollback {
            write!(f, " (earlier writes: {})", r.describe())?;
        }
        Ok(())
    }
}

impl From<ApplyError> for String {
    fn from(e: ApplyError) -> String {
        e.to_string()
    }
}

/// Writes the edits under `root`. Callers check the boundary first.
///
/// Every target is checked before anything is written: it must stay inside the
/// root without passing through a symbolic link, must be a regular file if it
/// exists, and must be readable (so it can be restored exactly). If a write
/// fails part-way, what was written is rolled back, and the error says whether
/// that rollback checked out.
pub fn apply(root: &Path, edits: &[FileEdit]) -> Result<Applied, ApplyError> {
    let refuse = |message: String| ApplyError { message, rollback: None };
    let mut planned = Vec::new();
    for (edit, path) in edits.iter().zip(targets(edits).map_err(refuse)?) {
        let full = inside(root, &path).map_err(|e| refuse(format!("not written: {e}")))?;
        if let Ok(meta) = fs::symlink_metadata(&full) {
            if !meta.file_type().is_file() {
                return Err(refuse(format!("not written: {path} exists and is not a regular file")));
            }
        }
        let original =
            current(&full).map_err(|e| refuse(format!("not written: {path} cannot be read to keep a copy: {e}")))?;
        if original.as_deref() == Some(edit.contents.as_bytes()) {
            continue;
        }
        planned.push((path, full, original, edit.contents.clone().into_bytes()));
    }
    let mut applied = Applied::default();
    for (path, full, original, bytes) in planned {
        if let Some(parent) = full.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                let rollback = (!applied.written.is_empty()).then(|| rollback(root, applied));
                return Err(ApplyError { message: format!("{path}: {e}"), rollback });
            }
        }
        if let Err(e) = write_atomic(&full, &bytes) {
            let rollback = (!applied.written.is_empty()).then(|| rollback(root, applied));
            return Err(ApplyError { message: format!("{path}: {e}"), rollback });
        }
        applied.written.push(Written { path: path.clone(), original, wrote: bytes });
        applied.changed.push(path);
    }
    Ok(applied)
}

/// Restores every file an [`apply`] changed, newest first, and checks each one.
/// A file that no longer holds what the agent wrote is left alone and reported.
pub fn rollback(root: &Path, applied: Applied) -> Rollback {
    let mut result = Rollback::default();
    for w in applied.written.into_iter().rev() {
        let full = match inside(root, &w.path) {
            Ok(full) => full,
            Err(e) => {
                result.failed.push(format!("{}: {e}", w.path));
                continue;
            }
        };
        match current(&full) {
            Ok(Some(now)) if now == w.wrote => {}
            Ok(_) => {
                result.modified_since.push(w.path);
                continue;
            }
            Err(e) => {
                result.failed.push(format!("{}: {e}", w.path));
                continue;
            }
        }
        let restored = match &w.original {
            Some(bytes) => write_atomic(&full, bytes),
            None => fs::remove_file(&full),
        };
        match (restored, current(&full)) {
            (Ok(()), Ok(now)) if now == w.original => result.restored.push(w.path),
            (Err(e), _) => result.failed.push(format!("{}: {e}", w.path)),
            (_, Err(e)) => result.failed.push(format!("{}: {e}", w.path)),
            (Ok(()), Ok(_)) => {
                result.failed.push(format!("{}: does not hold its original contents after restoring", w.path))
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blocks_and_strips_fences() {
        let text = "Here you go.\n=== FILE: src/a.mjs ===\n```js\nexport const a = 1;\n```\n=== END FILE ===\nthanks";
        assert_eq!(
            parse(text).unwrap(),
            [FileEdit { path: "src/a.mjs".into(), contents: "export const a = 1;\n".into() }]
        );
        assert!(parse("no blocks").is_err());
        assert!(parse("=== FILE: a.mjs ===\nunterminated").is_err());
    }

    #[test]
    fn takes_a_single_fenced_block_only_when_unambiguous() {
        let one = vec!["src/a.mjs".to_string()];
        let text = "Sure:\n```js\nexport const a = 2;\n```\nDone.";
        assert_eq!(single_fenced(text, &one).unwrap().contents, "export const a = 2;\n");
        assert!(single_fenced("```\na\n```\n```\nb\n```", &one).is_none(), "two blocks are ambiguous");
        assert!(single_fenced(text, &["a".into(), "b".into()]).is_none(), "two editable files are ambiguous");
        assert!(single_fenced("no code", &one).is_none());
    }

    #[test]
    fn rejects_paths_outside_the_project() {
        let edits = [FileEdit { path: "../../etc/passwd".into(), contents: String::new() }];
        assert!(targets(&edits).is_err());
    }

    #[test]
    fn applies_and_rolls_back() {
        let dir = std::env::temp_dir().join(format!("appport-agent-edit-{}", std::process::id()));
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/a.mjs"), "old\n").unwrap();
        let edits = [
            FileEdit { path: "src/a.mjs".into(), contents: "new\n".into() },
            FileEdit { path: "src/b.mjs".into(), contents: "created\n".into() },
        ];
        let applied = apply(&dir, &edits).unwrap();
        assert_eq!(applied.changed, ["src/a.mjs", "src/b.mjs"]);
        assert_eq!(fs::read_to_string(dir.join("src/a.mjs")).unwrap(), "new\n");
        let undone = rollback(&dir, applied);
        assert!(undone.verified(), "{undone:?}");
        assert_eq!(undone.restored, ["src/b.mjs", "src/a.mjs"]);
        assert_eq!(fs::read_to_string(dir.join("src/a.mjs")).unwrap(), "old\n");
        assert!(!dir.join("src/b.mjs").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("appport-agent-edit-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        dir
    }

    #[test]
    fn restores_bytes_exactly_even_when_they_are_not_utf8() {
        let dir = scratch("bytes");
        let original = vec![0x63, 0x61, 0x66, 0xe9, 0x0a]; // "café" in Latin-1
        fs::write(dir.join("src/a.mjs"), &original).unwrap();
        let applied = apply(&dir, &[FileEdit { path: "src/a.mjs".into(), contents: "new\n".into() }]).unwrap();
        assert!(rollback(&dir, applied).verified());
        assert_eq!(fs::read(dir.join("src/a.mjs")).unwrap(), original, "restored, not deleted");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn never_overwrites_a_file_changed_after_the_agent_wrote_it() {
        let dir = scratch("concurrent");
        fs::write(dir.join("src/a.mjs"), "old\n").unwrap();
        let applied = apply(&dir, &[FileEdit { path: "src/a.mjs".into(), contents: "agent\n".into() }]).unwrap();
        fs::write(dir.join("src/a.mjs"), "the user's edit\n").unwrap();
        let undone = rollback(&dir, applied);
        assert!(!undone.verified());
        assert_eq!(undone.modified_since, ["src/a.mjs"]);
        assert_eq!(fs::read_to_string(dir.join("src/a.mjs")).unwrap(), "the user's edit\n");
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn never_writes_through_a_symbolic_link() {
        let dir = scratch("link");
        let outside = dir.with_extension("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("target.mjs"), "outside\n").unwrap();
        std::os::unix::fs::symlink(outside.join("target.mjs"), dir.join("src/a.mjs")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("linked")).unwrap();
        for path in ["src/a.mjs", "linked/target.mjs", "linked/new.mjs"] {
            let error = apply(&dir, &[FileEdit { path: path.into(), contents: "agent\n".into() }]).unwrap_err();
            assert!(error.message.contains("symbolic link"), "{path}: {error}");
            assert!(error.rollback.is_none(), "nothing was written");
        }
        assert_eq!(fs::read_to_string(outside.join("target.mjs")).unwrap(), "outside\n");
        assert!(!outside.join("new.mjs").exists());
        fs::remove_dir_all(dir).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn checks_every_target_before_writing_any() {
        let dir = scratch("precheck");
        fs::write(dir.join("src/a.mjs"), "old\n").unwrap();
        fs::create_dir_all(dir.join("src/dir.mjs")).unwrap();
        let edits = [
            FileEdit { path: "src/a.mjs".into(), contents: "new\n".into() },
            FileEdit { path: "src/dir.mjs".into(), contents: "x\n".into() },
        ];
        assert!(apply(&dir, &edits).unwrap_err().message.contains("not a regular file"));
        assert_eq!(fs::read_to_string(dir.join("src/a.mjs")).unwrap(), "old\n", "nothing was written");
        fs::remove_dir_all(dir).unwrap();
    }
}
