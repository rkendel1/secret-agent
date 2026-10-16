//! The general (no AppPort) planner on the coding fixtures, offline.

use std::path::PathBuf;

use agent_core::generic::{coding_task, plan_coding, Baseline, CodingContext};
use agent_core::observe::observe;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/coding").join(name).join("repo")
}

fn test_cmd() -> Vec<String> {
    vec!["npm".into(), "test".into()]
}

#[test]
fn failing_tests_bound_the_change_and_name_matches_stay_read_only() {
    let project = observe(&fixture("bugfix-invoice-discount")).unwrap();
    let baseline = Baseline {
        passed: false,
        output: "not ok 4 - a percentage discount\n  location: 'test/invoice.test.js:24:1'".into(),
    };
    let plan = plan_coding(
        &project,
        "Customers with a percentage discount are charged the wrong invoice total.",
        &test_cmd(),
        &baseline,
    )
    .unwrap();
    assert_eq!(plan.failing_tests, ["test/invoice.test.js"]);
    assert_eq!(plan.editable(), ["src/invoice.js"]);
    let read: Vec<&str> = plan.context.iter().filter(|f| !f.editable).map(|f| f.path.as_str()).collect();
    assert!(
        read.contains(&"test/invoice.test.js") && read.contains(&"src/money.js") && read.contains(&"src/customer.js"),
        "{read:?}"
    );
}

#[test]
fn without_failing_tests_the_intent_bounds_the_change() {
    let project = observe(&fixture("feature-slug-max-length")).unwrap();
    let baseline = Baseline { passed: true, output: String::new() };
    let plan = plan_coding(&project, "Add a maxLength option to slugify", &test_cmd(), &baseline).unwrap();
    assert_eq!(plan.editable(), ["src/text/slug.js"]);
    assert!(plan.failing_tests.is_empty());
}

#[test]
fn refuses_when_nothing_points_anywhere() {
    let project = observe(&fixture("feature-slug-max-length")).unwrap();
    let baseline = Baseline { passed: true, output: String::new() };
    assert_eq!(plan_coding(&project, "Make it better", &test_cmd(), &baseline).unwrap_err().code, "target.unlocated");
}

#[test]
fn the_minimal_arm_sends_only_the_editable_file() {
    let project = observe(&fixture("bugfix-invoice-discount")).unwrap();
    let baseline = Baseline { passed: false, output: "test/invoice.test.js failed".into() };
    let plan = plan_coding(&project, "Fix the invoice total", &test_cmd(), &baseline).unwrap();
    let files = plan.context.iter().map(|f| (f.path.clone(), project.read(&f.path).unwrap())).collect();
    let observed = coding_task(CodingContext::Observed, &plan, &files, &baseline, &[]);
    let minimal = coding_task(CodingContext::Minimal, &plan, &files, &baseline, &[]);
    assert!(
        observed.prompt.contains("## test/invoice.test.js (read-only")
            && observed.prompt.contains("Test output before the change")
    );
    assert!(!minimal.prompt.contains("test/invoice.test.js") && !minimal.prompt.contains("src/money.js"));
    assert!(minimal.prompt.contains("## src/invoice.js (EDITABLE"));
}
