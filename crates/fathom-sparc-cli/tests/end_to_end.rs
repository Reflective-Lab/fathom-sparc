//! End-to-end smoke test: run the binary against real Apple fixtures and
//! assert the proposed `RiskFactorDrift` carries the expected shape.
//!
//! Cargo provides `CARGO_BIN_EXE_<name>` to integration tests so we can spawn
//! the just-built binary without hard-coding a path.

use std::process::Command;

#[test]
fn analyse_apple_emits_both_drift_signals_for_fy24_to_fy25() {
    let output = Command::new(env!("CARGO_BIN_EXE_fathom-sparc"))
        .args(["analyse", "0000320193"])
        .output()
        .expect("spawn fathom binary");

    assert!(
        output.status.success(),
        "binary exited non-zero: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout utf-8");
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout is JSON");

    let proposals = parsed.as_array().expect("array of proposals");
    assert_eq!(proposals.len(), 2, "expected one drift + one language proposal");

    let by_provenance: std::collections::HashMap<&str, &serde_json::Value> = proposals
        .iter()
        .map(|p| (p["provenance"].as_str().unwrap(), p))
        .collect();

    let count = by_provenance
        .get("fathom:risk_factor_drift:v1")
        .expect("count drift proposal");
    assert_eq!(count["content"]["current"]["fiscal_year"], 2025);
    assert_eq!(count["content"]["prior"]["fiscal_year"], 2024);
    assert_eq!(count["content"]["current_count"], 27);
    assert_eq!(count["content"]["prior_count"], 28);
    assert_eq!(count["content"]["delta"], -1);

    let language = by_provenance
        .get("fathom-sparc:risk_factor_language:v1")
        .expect("language drift proposal");
    let content = &language["content"];
    assert_eq!(content["current"]["fiscal_year"], 2025);
    assert_eq!(content["prior"]["fiscal_year"], 2024);
    let identical = content["identical_count"].as_u64().expect("identical_count");
    let added = content["added"].as_array().expect("added array");
    let removed = content["removed"].as_array().expect("removed array");
    let jaccard = content["jaccard_similarity"].as_f64().expect("jaccard f64");
    // Apple kept most headings byte-identical but rephrased a handful and
    // dropped the dedicated retail-stores risk factor. Strict numeric
    // equality is brittle; assert plausible bounds that would catch a
    // regression but tolerate a future re-extraction tweaking by ±2.
    assert!(
        identical >= 18 && identical <= 25,
        "identical={identical} outside expected window"
    );
    assert!(
        !added.is_empty() && !removed.is_empty(),
        "expected non-trivial added+removed lists; added={} removed={}",
        added.len(),
        removed.len()
    );
    assert!(
        jaccard > 0.4 && jaccard < 1.0,
        "jaccard={jaccard} outside expected window"
    );
}

#[test]
fn analyse_unknown_cik_fails_with_clear_message() {
    let output = Command::new(env!("CARGO_BIN_EXE_fathom-sparc"))
        .args(["analyse", "9999999999"])
        .output()
        .expect("spawn fathom binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("9999999999") && stderr.contains("need at least 2"),
        "expected helpful error, got: {stderr}"
    );
}
