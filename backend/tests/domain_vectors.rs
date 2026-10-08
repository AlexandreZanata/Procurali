//! Domain-vector acceptance: every money vector in `contracts/domain-vectors.json`
//! behaves exactly as frozen, plus budget compatibility and overflow bounds.
//! The JSON file is the independent expectation source; this file asserts the
//! implementation against it at runtime (path relative to `backend/`).

use procurali_backend::domain::money::{Money, MoneyError};
use serde_json::Value;
use std::collections::HashMap;

fn contract() -> Value {
    let text = std::fs::read_to_string("../contracts/domain-vectors.json")
        .expect("frozen domain vectors are readable");
    serde_json::from_str(&text).expect("frozen domain vectors parse")
}

fn expected_code(reason: &str) -> &'static str {
    match reason {
        "missing_field" => MoneyError::Empty.code(),
        "malformed_amount" => MoneyError::Malformed.code(),
        "nonpositive_amount" => MoneyError::NonPositive.code(),
        "overprecision" => MoneyError::Overprecision.code(),
        other => panic!("contract uses an unmapped reason: {other}"),
    }
}

#[test]
fn contract_money_vectors_hold_exactly() {
    let vectors = contract()["money"].clone();
    let valid = vectors["valid"].as_array().expect("valid vectors");
    assert!(!valid.is_empty(), "contract carries valid vectors");
    for entry in valid {
        let input = entry["input"].as_str().expect("string input");
        let cents = entry["cents"].as_i64().expect("integer cents");
        let parsed = Money::parse(input).expect("valid contract input parses");
        assert_eq!(parsed.cents(), cents, "600.00-style mapping for {input}");
        // Canonical rendering round-trips through the parser.
        let rendered = parsed.format();
        assert_eq!(
            Money::parse(&rendered)
                .expect("canonical form re-parses")
                .cents(),
            cents
        );
    }
    let invalid = vectors["invalid"].as_array().expect("invalid vectors");
    assert!(!invalid.is_empty(), "contract carries invalid vectors");
    let mut reasons = HashMap::new();
    for entry in invalid {
        let input = entry["input"].as_str().expect("string input");
        let reason = entry["reason"].as_str().expect("string reason");
        let error = Money::parse(input).expect_err("invalid contract input fails");
        assert_eq!(
            error.code(),
            expected_code(reason),
            "stable refusal code for {input:?}"
        );
        *reasons.entry(reason.to_owned()).or_insert(0) += 1;
    }
    // Every refusal family in the contract is actually exercised.
    for family in [
        "missing_field",
        "malformed_amount",
        "nonpositive_amount",
        "overprecision",
    ] {
        assert!(
            reasons.get(family).is_some_and(|count| *count > 0),
            "refusal family {family} covered"
        );
    }
}

#[test]
fn budget_compatibility_follows_inv19_ac16() {
    let budget = Money::parse("600.00").expect("ceiling parses");
    let fitting = Money::parse("520.00").expect("price parses");
    let equal = Money::parse("600.00").expect("equal price parses");
    let above = Money::parse("600.01").expect("above-ceiling parses");
    assert!(Money::fits_budget(fitting, budget));
    assert!(Money::fits_budget(equal, budget), "equality fits");
    assert!(!Money::fits_budget(above, budget), "600.01 exceeds 600.00");
}

#[test]
fn overflow_and_malformed_inputs_fail_explicitly_without_panic() {
    // i64::MAX cents exactly: parses; one cent more: explicit overflow.
    let max = Money::parse("92233720368547758.07").expect("boundary parses");
    assert_eq!(max.cents(), i64::MAX);
    assert_eq!(
        Money::parse("92233720368547758.08").expect_err("one cent over fails"),
        MoneyError::Overflow
    );
    assert_eq!(
        Money::parse("99999999999999999999.99").expect_err("huge input fails"),
        MoneyError::Overflow
    );
    for malformed in [
        "", " ", " 520.00", "520.00 ", "+520.00", "520,00", "1e3", "NaN",
    ] {
        assert!(
            Money::parse(malformed).is_err(),
            "{malformed:?} is refused without panic"
        );
    }
    // Canonical rendering is exact, always two digits.
    assert_eq!(
        Money::parse("600").expect("whole parses").format(),
        "600.00"
    );
    assert_eq!(Money::parse("0.01").expect("cent parses").format(), "0.01");
}
