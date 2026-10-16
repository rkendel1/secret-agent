//! Planning for ordinary repositories, with no AppPort knowledge at all.
//!
//! The only sources of truth are the repository (its files and import graph,
//! from [`crate::observe`]) and what its own test command reports. From a
//! baseline run of that command, the planner takes the test files the output
//! names as failing, follows their imports to the code under test, adds files
//! the intent names, and draws the edit boundary around that code. Test files
//! are never editable: a change is accepted only when the project's own tests
//! pass, so letting the model edit them would let it grade itself.
//!
//! Nothing here reads contracts, capabilities or verification methods.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::observe::Project;
use crate::plan::{ContextFile, Decision, ModelTask, Refusal, FILE_CLOSE, FILE_OPEN};

/// Most files the change may touch.
const MAX_EDITABLE: usize = 4;
/// Budget for read-only context, in bytes.
const CONTEXT_BUDGET: usize = 48 * 1024;
/// How much test output is shown to the model.
const OUTPUT_TAIL: usize = 6 * 1024;

/// A task for an ordinary repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodingTask {
    pub id: String,
    /// `bug-fix`, `feature`, `refactor`, …; descriptive.
    #[serde(default)]
    pub kind: String,
    /// What to do, in words, as a person would ask.
    pub intent: String,
    /// The command whose success means the change is acceptable, run in the repository.
    pub test: Vec<String>,
    /// Acceptance checks the agent never sees (run by the evaluation only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<Vec<String>>,
    /// The files a person would change; for scoring only, never shown to the agent.
    #[serde(default)]
    pub intended_files: Vec<String>,
}

/// How much the model is told. The verifier is the same either way.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodingContext {
    /// The editable files, the failing tests, what the code imports, and the test output.
    #[default]
    Observed,
    /// The intent and the editable files only.
    Minimal,
}

impl std::str::FromStr for CodingContext {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "observed" => Ok(Self::Observed),
            "minimal" => Ok(Self::Minimal),
            other => Err(format!("unknown context {other:?}: use observed or minimal")),
        }
    }
}

impl std::fmt::Display for CodingContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Observed => "observed",
            Self::Minimal => "minimal",
        })
    }
}

/// What the baseline run of the test command showed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Baseline {
    pub passed: bool,
    pub output: String,
}

/// The plan for one task.
#[derive(Debug, Clone, Serialize)]
pub struct CodingPlan {
    pub intent: String,
    pub test: Vec<String>,
    /// Test files the baseline output names.
    pub failing_tests: Vec<String>,
    pub context: Vec<ContextFile>,
    pub decisions: Vec<Decision>,
}

impl CodingPlan {
    pub fn editable(&self) -> Vec<&str> {
        self.context.iter().filter(|f| f.editable).map(|f| f.path.as_str()).collect()
    }
}

/// Whether a path is a test file, by the conventions of common ecosystems.
pub fn is_test_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    lower.split('/').any(|part| matches!(part, "test" | "tests" | "__tests__" | "spec"))
        || name.contains(".test.")
        || name.contains(".spec.")
        || name.contains("_test.")
        || (name.starts_with("test_") && name.ends_with(".py"))
}

/// Lowercase words of at least 4 letters, with a trailing plural `s` dropped.
fn words(text: &str) -> BTreeSet<String> {
    let mut split = String::new();
    let mut previous_lower = false;
    for c in text.chars() {
        if c.is_uppercase() && previous_lower {
            split.push(' ');
        }
        previous_lower = c.is_lowercase();
        split.push(if c.is_alphanumeric() { c.to_ascii_lowercase() } else { ' ' });
    }
    split
        .split_whitespace()
        .filter(|w| w.len() >= 4)
        .map(|w| w.strip_suffix('s').filter(|s| s.len() >= 4).unwrap_or(w).to_string())
        .collect()
}

/// Plans a change to `project` from the intent and a baseline run of the test command.
pub fn plan_coding(
    project: &Project,
    intent: &str,
    test: &[String],
    baseline: &Baseline,
) -> Result<CodingPlan, Refusal> {
    let mut decisions = Vec::new();
    let mut decide = |step: &'static str, fact: String| decisions.push(Decision { step, fact });
    let refuse = |code: &str, message: String| Refusal { code: code.into(), message };

    decide("observe", format!("{} source file(s); test command `{}`", project.sources.len(), test.join(" ")));
    let failing: Vec<String> = if baseline.passed {
        decide("baseline", "the test command passes before any change".into());
        Vec::new()
    } else {
        let named: Vec<String> = project
            .sources
            .iter()
            .map(|s| s.path.clone())
            .filter(|p| is_test_file(p) && baseline.output.contains(p.as_str()))
            .collect();
        decide(
            "baseline",
            format!(
                "the test command fails before any change; its output names {}",
                if named.is_empty() { "no test file in the repository".to_string() } else { named.join(", ") }
            ),
        );
        named
    };

    // Code under test: what the failing tests import, directly.
    let mut editable: BTreeMap<String, String> = BTreeMap::new();
    for test_file in &failing {
        for import in project.source(test_file).map(|s| s.imports.clone()).unwrap_or_default() {
            if !is_test_file(&import) {
                editable.entry(import.clone()).or_insert_with(|| format!("imported by the failing test {test_file}"));
            }
        }
    }
    // Files the intent names. Evidence wins: when failing tests point at code,
    // a name match is only read-only context; otherwise it bounds the change.
    let intent_words = words(intent);
    let mut named: BTreeMap<String, String> = BTreeMap::new();
    for source in project.sources.iter().filter(|s| !is_test_file(&s.path)) {
        let stem = source.path.rsplit('/').next().unwrap_or(&source.path);
        let stem = stem.split('.').next().unwrap_or(stem);
        let word = words(stem).into_iter().find(|w| intent_words.iter().any(|i| i == w || i.starts_with(w.as_str())));
        if let Some(word) = word {
            named.insert(source.path.clone(), format!("its name matches \"{word}\" in the intent"));
        }
    }
    let evidence = !editable.is_empty();
    if !evidence {
        editable = named.clone();
    }
    if editable.is_empty() {
        return Err(refuse(
            "target.unlocated",
            "neither a failing test nor the intent points at any source file; the change cannot be bounded".into(),
        ));
    }
    if editable.len() > MAX_EDITABLE {
        // Prefer what the failing tests import over name matches.
        let keep: Vec<String> = editable
            .iter()
            .filter(|(_, reason)| reason.starts_with("imported"))
            .map(|(p, _)| p.clone())
            .chain(editable.keys().cloned())
            .collect::<Vec<_>>()
            .into_iter()
            .fold(Vec::new(), |mut acc, p| {
                if !acc.contains(&p) && acc.len() < MAX_EDITABLE {
                    acc.push(p);
                }
                acc
            });
        editable.retain(|p, _| keep.contains(p));
        decide("plan", format!("the boundary is capped at {MAX_EDITABLE} files"));
    }

    let mut context: Vec<ContextFile> = editable
        .iter()
        .map(|(path, reason)| ContextFile { path: path.clone(), editable: true, reason: reason.clone() })
        .collect();
    let mut seen: BTreeSet<String> = editable.keys().cloned().collect();
    let mut budget = CONTEXT_BUDGET;
    let mut add = |path: &str, reason: String, context: &mut Vec<ContextFile>| {
        let Some(source) = project.source(path) else { return };
        if seen.contains(path) || source.bytes > budget {
            return;
        }
        budget -= source.bytes;
        seen.insert(path.to_string());
        context.push(ContextFile { path: path.to_string(), editable: false, reason });
    };
    for test_file in &failing {
        add(test_file, "a failing test (read-only: tests decide acceptance)".into(), &mut context);
    }
    if evidence {
        for (path, reason) in &named {
            add(path, format!("{reason}; not editable, since the failing tests point elsewhere"), &mut context);
        }
    }
    for path in editable.keys() {
        for import in project.source(path).map(|s| s.imports.clone()).unwrap_or_default() {
            add(&import, format!("imported by {path}"), &mut context);
        }
    }
    for file in &context {
        decide("context", format!("{} {}: {}", if file.editable { "edit" } else { "read" }, file.path, file.reason));
    }

    Ok(CodingPlan { intent: intent.to_string(), test: test.to_vec(), failing_tests: failing, context, decisions })
}

/// The last `limit` bytes of `text`, starting at a line.
pub fn tail(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    let start = text.len() - limit;
    let start = (start..text.len()).find(|i| text.is_char_boundary(*i)).unwrap_or(start);
    let start = text[start..].find('\n').map_or(start, |n| start + n + 1);
    &text[start..]
}

/// The model task for a coding plan.
pub fn coding_task(
    mode: CodingContext,
    plan: &CodingPlan,
    files: &BTreeMap<String, String>,
    baseline: &Baseline,
    feedback: &[String],
) -> ModelTask {
    let system = format!(
        "You are a careful software engineer making one change in a repository. Change only the editable files you \
are given, and keep their existing exports working. The change is accepted only if the project's own tests pass; \
test files are read-only. Reply with the complete new contents of each file you change, and nothing else, in this \
exact form:\n{FILE_OPEN}<path> ===\n<entire file contents>\n{FILE_CLOSE}"
    );
    let editable = plan.editable();
    let mut prompt = format!("# Task\n{}\n\nEditable files: {}\n", plan.intent, editable.join(", "));
    if mode == CodingContext::Observed {
        prompt.push_str(&format!("Acceptance: `{}` must pass.\n", plan.test.join(" ")));
        if !baseline.passed {
            prompt.push_str(&format!(
                "\n# Test output before the change\n```\n{}\n```\n",
                tail(&baseline.output, OUTPUT_TAIL).trim_end()
            ));
        }
    }
    if !feedback.is_empty() {
        prompt.push_str("\n# The previous attempt was rejected\n");
        for item in feedback {
            prompt.push_str(&format!("{item}\n"));
        }
        prompt.push_str("Fix the cause.\n");
    }
    prompt.push_str("\n# Files\n");
    for file in &plan.context {
        if mode == CodingContext::Minimal && !file.editable {
            continue;
        }
        let Some(contents) = files.get(&file.path) else { continue };
        // The minimal arm gets the file and nothing observed about it: the reason
        // would name the failing test.
        let label = match mode {
            CodingContext::Observed => {
                format!("{}; {}", if file.editable { "EDITABLE" } else { "read-only" }, file.reason)
            }
            CodingContext::Minimal => "EDITABLE".to_string(),
        };
        prompt.push_str(&format!("## {} ({label})\n```\n{}\n```\n", file.path, contents.trim_end()));
    }
    prompt
        .push_str(&format!("\nReply with the new contents of the files you change among: {}.\n", editable.join(", ")));
    ModelTask { system, prompt }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_test_files() {
        for path in
            ["test/a.js", "src/a.test.ts", "pkg/a_test.go", "tests/test_a.py", "src/__tests__/a.js", "a.spec.js"]
        {
            assert!(is_test_file(path), "{path}");
        }
        for path in ["src/testing.js", "src/latest.js", "contest/a.js"] {
            assert!(!is_test_file(path), "{path}");
        }
    }

    #[test]
    fn splits_intent_words() {
        let w = words("Fix the invoiceTotal for Invoices; add slugify maxLength");
        for expected in ["invoice", "total", "slugify", "length", "maxlength"] {
            assert!(w.contains(expected) || expected == "maxlength", "{expected} in {w:?}");
        }
        assert!(!w.contains("the") && !w.contains("for"));
    }

    #[test]
    fn tails_at_a_line() {
        assert_eq!(tail("one\ntwo\nthree", 7), "three");
        assert_eq!(tail("short", 100), "short");
    }
}
