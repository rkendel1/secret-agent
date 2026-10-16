//! The coding loop on the fixtures with scripted completions. Needs Node.js 22+
//! (`node --test` with glob patterns), so it is ignored by a plain `cargo test`;
//! CI's agent-e2e job runs it with `--ignored`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use agent_core::generic::CodingContext;
use agent_model::{ModelConfig, Provider};
use appport_agent::coding::{coding_eval, CodingArm, CodingEvalOptions};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/coding").canonicalize().unwrap()
}

#[test]
#[ignore = "needs Node.js 22+"]
fn scripted_completions_are_judged_by_the_projects_own_tests() {
    let out = std::env::temp_dir().join(format!("appport-agent-coding-{}", std::process::id()));
    let mut options = CodingEvalOptions::new(
        vec![fixtures().join("bugfix-invoice-discount"), fixtures().join("feature-slug-max-length")],
        ModelConfig::new(Provider::Scripted),
        vec![CodingArm::of(CodingContext::Observed)],
        out.clone(),
    );
    options.trials = 1;
    options.test_timeout = Duration::from_secs(60);
    options.quiet = true;
    let report = coding_eval(&options).unwrap();

    let bugfix = report.cells.iter().find(|c| c.task == "bugfix-invoice-discount").unwrap();
    // 01.txt edits the failing test instead of the code: never applied, then 02.txt fixes the code.
    assert_eq!((bugfix.successes, bugfix.test_edit_attempts, bugfix.boundary_violations), (1, 1, 1));
    assert_eq!(bugfix.stayed_in_intended_files, 1);
    let feature = report.cells.iter().find(|c| c.task == "feature-slug-max-length").unwrap();
    assert_eq!((feature.successes, feature.accepted_first_attempt), (1, 1));
    assert!(report.trials.iter().all(|t| t.test_runs >= 2 && t.hidden_passed == Some(true)));
    assert!(out.join("bugfix-invoice-discount/observed-01/accepted.diff").exists());
    let source = std::fs::read_to_string(fixtures().join("bugfix-invoice-discount/repo/src/invoice.js")).unwrap();
    assert!(source.contains("sub * invoice.discountPercent"), "the fixture itself is never modified");
    let _ = std::fs::remove_dir_all(out);
}
