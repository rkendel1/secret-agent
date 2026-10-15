//! Reading the model's proposed change and applying it within the boundary.
//!
//! The model answers with whole files (see [`agent_core::plan::FILE_OPEN`]). A
//! file outside the plan's boundary is never written: the change is rejected
//! before anything runs. Every applied change can be rolled back exactly.

use std::fs;
use std::path::Path;

use agent_core::observe::normalize;
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

/// Originals of applied files, for rollback.
#[derive(Debug, Default)]
pub struct Applied {
    originals: Vec<(String, Option<String>)>,
    /// Files whose contents actually changed.
    pub changed: Vec<String>,
}

/// Writes the edits under `root`. Callers check the boundary first.
pub fn apply(root: &Path, edits: &[FileEdit]) -> Result<Applied, String> {
    let mut applied = Applied::default();
    for (edit, path) in edits.iter().zip(targets(edits)?) {
        let full = root.join(&path);
        let original = fs::read_to_string(&full).ok();
        if original.as_deref() == Some(edit.contents.as_str()) {
            continue;
        }
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{path}: {e}"))?;
        }
        applied.originals.push((path.clone(), original));
        if let Err(e) = fs::write(&full, &edit.contents) {
            rollback(root, applied);
            return Err(format!("{path}: {e}"));
        }
        applied.changed.push(path);
    }
    Ok(applied)
}

/// Restores every file an [`apply`] changed.
pub fn rollback(root: &Path, applied: Applied) {
    for (path, original) in applied.originals.into_iter().rev() {
        let full = root.join(&path);
        let _ = match original {
            Some(text) => fs::write(&full, text),
            None => fs::remove_file(&full),
        };
    }
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
        rollback(&dir, applied);
        assert_eq!(fs::read_to_string(dir.join("src/a.mjs")).unwrap(), "old\n");
        assert!(!dir.join("src/b.mjs").exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
