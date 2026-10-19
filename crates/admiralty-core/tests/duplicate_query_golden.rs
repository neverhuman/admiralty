use admiralty_core::intake::DuplicateQuery;
use serde::Deserialize;

#[derive(Deserialize)]
struct Golden {
    query: DuplicateQuery,
    query_digest: String,
}

#[test]
fn browser_duplicate_query_digest_matches_typed_rust_encoding() {
    let fixtures: Vec<Golden> = serde_json::from_str(include_str!(
        "../../../packages/contracts/fixtures/intake-duplicate-query-golden.json"
    ))
    .expect("checked-in Rust-generated fixtures");
    assert_eq!(fixtures.len(), 2);
    for fixture in fixtures {
        assert_eq!(
            fixture.query.identity_digest().expect("valid query"),
            fixture.query_digest
        );
        let mut changed = fixture.query.clone();
        changed.limit += 1;
        assert_ne!(
            changed.identity_digest().expect("changed valid query"),
            fixture.query_digest
        );
    }
}
