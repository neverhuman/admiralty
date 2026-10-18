use admiralty_core::verification::{MAX_RETAINED_RECEIPTS, ReceiptRegistry, parse_receipt};

#[test]
fn backwards_clock_skew_is_rejected_without_mutating_time() {
    let mut clock = admiralty_testkit::clock::FakeClock::new(1_000);
    let before_skew = clock.clone();

    assert!(clock.advance_to_ms(999).is_err());
    assert_eq!(clock, before_skew);
    assert_eq!(clock.now_ms(), 1_000);
}

#[test]
fn receipt_parser_rejects_forged_unknown_fields_and_registry_is_bounded() {
    let forged_claim = br#"{"schema_version":1,"receipt_id":"r1","producer":"provider_claim","verifier":"verifier","inputs":{},"checks":[],"negative_checks":[],"acceptance_granted":true}"#;
    assert!(parse_receipt(forged_claim).is_err());
    assert!(
        parse_receipt(forged_claim).is_err(),
        "replayed forged claims stay rejected"
    );
    assert!(parse_receipt(br#"{"unknown":true}"#).is_err());
    assert!(
        parse_receipt(&[0xff]).is_err(),
        "invalid UTF-8 receipt bytes are rejected"
    );

    let registry = ReceiptRegistry::new(1).expect("bounded receipt history");
    assert_eq!(registry.retained_receipts(), 0);
    assert!(ReceiptRegistry::new(0).is_err());
    assert!(ReceiptRegistry::new(MAX_RETAINED_RECEIPTS).is_ok());
    assert!(ReceiptRegistry::new(MAX_RETAINED_RECEIPTS + 1).is_err());
}

#[test]
fn receipt_parser_rejects_input_over_the_size_limit() {
    use admiralty_core::verification::MAX_RECEIPT_BYTES;

    let oversized = vec![b' '; MAX_RECEIPT_BYTES + 1];
    assert_eq!(
        parse_receipt(&oversized)
            .expect_err("oversized receipt rejected before JSON decoding")
            .0,
        "receipt exceeds 1 MiB",
        "the bound must reject before the JSON parser, not merely reject invalid JSON"
    );
    assert!(parse_receipt(br#"{} trailing"#).is_err());
}
