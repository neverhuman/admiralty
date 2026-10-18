use admiralty_core::verification::{ReceiptRegistry, parse_receipt};

#[test]
fn receipt_parser_rejects_forged_unknown_fields_and_registry_is_bounded() {
    let forged_claim = br#"{"schema_version":1,"receipt_id":"r1","producer":"provider_claim","verifier":"verifier","inputs":{},"checks":[],"negative_checks":[],"acceptance_granted":true}"#;
    assert!(parse_receipt(forged_claim).is_err());
    assert!(parse_receipt(br#"{"unknown":true}"#).is_err());

    let registry = ReceiptRegistry::new(1).expect("bounded receipt history");
    assert_eq!(registry.retained_receipts(), 0);
    assert!(ReceiptRegistry::new(0).is_err());
}
