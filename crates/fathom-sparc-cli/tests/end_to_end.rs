//! End-to-end smoke test: run the binary against real Apple fixtures and
//! assert the proposed `RiskFactorDrift` carries the expected shape.
//!
//! Cargo provides `CARGO_BIN_EXE_<name>` to integration tests so we can spawn
//! the just-built binary without hard-coding a path.

use std::process::Command;

#[test]
fn analyse_apple_emits_drift_for_fy24_to_fy25() {
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
    assert_eq!(proposals.len(), 1, "exactly one drift expected");

    let p = &proposals[0];
    assert_eq!(p["key"], "Proposals");
    assert_eq!(p["provenance"], "fathom:risk_factor_drift:v1");

    let content = &p["content"];
    assert_eq!(content["current"]["cik"], "0000320193");
    assert_eq!(content["current"]["fiscal_year"], 2025);
    assert_eq!(content["prior"]["fiscal_year"], 2024);
    assert_eq!(content["current_count"], 27);
    assert_eq!(content["prior_count"], 28);
    assert_eq!(content["delta"], -1);
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
