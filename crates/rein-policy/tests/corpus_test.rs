// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: Golden regression + adversarial corpus test suite for ShellClassifier

//! Integration test running the 214-case ShellClassifier golden corpus.

use std::os::unix::fs::symlink;
use std::path::Path;

use rein_policy::{EffectLevel, ShellClassifier, TargetScope, WorkspaceSandbox};
use serde::Deserialize;

const CORPUS_TOML: &str = include_str!("corpus/rein-shell-classifier-corpus.toml");

#[derive(Debug, Deserialize)]
struct Corpus {
    cases: Vec<Case>,
    meta: Meta,
}

#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    cmd: String,
    min: Option<EffectLevel>,
    max: Option<EffectLevel>,
    target: Option<TargetScope>,
    #[serde(default)]
    fixture: Option<String>,
    #[serde(default)]
    stretch: bool,
}

#[derive(Debug, Deserialize)]
struct Meta {
    schema_version: u32,
    canary: String,
}

#[test]
fn golden_corpus_soundness_precision_and_invariants() -> Result<(), Box<dyn std::error::Error>> {
    let corpus: Corpus = toml::from_str(CORPUS_TOML)?;
    assert_eq!(corpus.meta.schema_version, 1);

    let ws_dir = std::env::temp_dir().join("rein_classifier_corpus_ws");
    let _ = std::fs::remove_dir_all(&ws_dir);
    std::fs::create_dir_all(&ws_dir)?;
    let link_path = ws_dir.join("escape-link");
    symlink("/etc", &link_path)?;
    let sandbox = WorkspaceSandbox::new(&ws_dir);

    let mut min_count = 0usize;
    let mut unknown_on_min = 0usize;

    for case in &corpus.cases {
        let active_sb = if case.fixture.as_deref() == Some("symlink-out") {
            Some(&sandbox)
        } else {
            None
        };
        let first = ShellClassifier::classify_script_with_sandbox(&case.cmd, active_sb);
        let second = ShellClassifier::classify_script_with_sandbox(&case.cmd, active_sb);
        assert_eq!(
            first, second,
            "non-deterministic classification on case {}",
            case.id
        );

        if let Some(min_eff) = case.min {
            min_count += 1;
            if first.effect == EffectLevel::Unknown {
                unknown_on_min += 1;
            }
            assert!(
                first.effect >= min_eff,
                "SOUNDNESS failure on {} (`{}`): got {:?}, expected >= {:?}",
                case.id,
                case.cmd,
                first.effect,
                min_eff
            );
            if let Some(expected_target) = case.target
                && first.effect != EffectLevel::Unknown
            {
                assert_eq!(
                    first.target, expected_target,
                    "TARGET failure on {} (`{}`): got {:?}, expected {:?}",
                    case.id, case.cmd, first.target, expected_target
                );
            }
        }

        if let Some(max_eff) = case.max {
            assert!(
                first.effect <= max_eff,
                "PRECISION failure on {} (stretch={}, `{}`): got {:?}, expected <= {:?}",
                case.id,
                case.stretch,
                case.cmd,
                first.effect,
                max_eff
            );
            assert_eq!(
                first.target,
                TargetScope::Workspace,
                "CONTROL TARGET failure on {} (`{}`): got {:?}",
                case.id,
                case.cmd,
                first.target
            );
        }
    }

    assert!(
        !Path::new(&corpus.meta.canary).exists(),
        "purity violation: canary file '{}' exists!",
        corpus.meta.canary
    );
    // Ensure classifier does not trivially answer Unknown for most adversarial cases (< 8% Unknown rate).
    assert!(
        unknown_on_min <= 14,
        "Unknown rate too high: {unknown_on_min}/{min_count}"
    );

    let _ = std::fs::remove_dir_all(&ws_dir);
    Ok(())
}
