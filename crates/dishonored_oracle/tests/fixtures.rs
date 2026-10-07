//! The authored oracle fixtures (scenario specs, cause book) parse and validate.

use std::path::PathBuf;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/oracle")
}

#[test]
fn scenarios_and_cause_book_parse() {
    let all = dishonored_oracle::scenario::load_dir(&fixtures().join("scenarios")).unwrap();
    assert!(all.len() >= 3);
    assert!(all.iter().any(|(_, s)| s.sweep.as_ref().is_some_and(|w| w.structure && !w.states.is_empty())));
    let book = dishonored_oracle::residual::CauseBook::load(&fixtures().join("causes.ron")).unwrap();
    assert!(!book.rules.is_empty());
}
