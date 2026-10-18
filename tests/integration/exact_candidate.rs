use admiralty_core::verification::{MAX_RETAINED_RECEIPTS, ReceiptRegistry, parse_receipt};

#[test]
fn receipt_parser_rejects_forged_unknown_fields_and_registry_is_bounded() {
    let forged_claim = br#"{"schema_version":1,"receipt_id":"r1","producer":"provider_claim","verifier":"verifier","inputs":{},"checks":[],"negative_checks":[],"acceptance_granted":true}"#;
    assert!(parse_receipt(forged_claim).is_err());
    assert!(
        parse_receipt(forged_claim).is_err(),
        "replayed forged claims stay rejected"
    );
    assert!(parse_receipt(br#"{"unknown":true}"#).is_err());

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
    assert!(parse_receipt(&oversized).is_err());
    assert!(parse_receipt(br#"{} trailing"#).is_err());
}
