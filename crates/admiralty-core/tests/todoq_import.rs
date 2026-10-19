use admiralty_core::{ProjectId, RootId, import::todoq::*};
use sha2::{Digest, Sha256};
struct File {
    path: String,
    bytes: Vec<u8>,
    blob: String,
    digest: String,
}
impl File {
    fn new(path: &str, text: &str) -> Self {
        let bytes = text.as_bytes().to_vec();
        let digest = format!("sha256:{:x}", Sha256::digest(&bytes));
        let mut h = Sha256::new();
        h.update(format!("blob {}\0", bytes.len()).as_bytes());
        h.update(&bytes);
        Self {
            path: path.into(),
            bytes,
            blob: format!("{:x}", h.finalize()),
            digest,
        }
    }
    fn pinned(&self) -> PinnedDocument<'_> {
        PinnedDocument {
            path: &self.path,
            blob_oid: &self.blob,
            raw_sha256: &self.digest,
            bytes: &self.bytes,
        }
    }
}
fn queue() -> QueuePin {
    QueuePin {
        repository: "synthetic-queue".into(),
        ref_name: "refs/heads/queue".into(),
        commit_oid: "c".repeat(64),
    }
}
fn mapping(family: &str, alias: &str) -> RepositoryMapping {
    RepositoryMapping {
        family: family.into(),
        repository_alias: alias.into(),
        project_id: ProjectId::new("project").expect("valid synthetic fixture"),
        root_id: RootId::new("root").expect("valid synthetic fixture"),
    }
}
fn record(id: &str, extra: &str, body: &str) -> String {
    format!(
        "+++\nid = {id:?}\nfamily = \"atlas-suite\"\ntitle = \"Synthetic request\"\nrepos = [\"engine\"]\n{extra}\n+++\n{body}"
    )
}
fn parse(
    family: &str,
    files: &[File],
    mappings: &[RepositoryMapping],
    prior: &[PriorSource],
    policy: &ImportPolicy,
) -> Result<SnapshotObservation, ImportError> {
    let q = queue();
    let f = File::new("family.toml", family);
    let todos: Vec<_> = files.iter().map(File::pinned).collect();
    parse_pinned_snapshot(
        &PinnedSnapshot {
            queue: &q,
            family: f.pinned(),
            todos: &todos,
            mappings,
            prior_sources: prior,
        },
        policy,
    )
}
const BATCH: &str = include_str!("../../../tests/fixtures/todoq/family-batch.toml");
const SHIFTS: &str = include_str!("../../../tests/fixtures/todoq/family-shifts.toml");
const LEGACY: &str = include_str!("../../../tests/fixtures/todoq/family-legacy.toml");
const MINIMAL: &str = include_str!("../../../tests/fixtures/todoq/family-minimal.toml");
fn codes(s: &SnapshotObservation) -> Vec<DiagnosticCode> {
    s.family_diagnostics
        .iter()
        .chain(s.records.iter().flat_map(|r| &r.diagnostics))
        .map(|d| d.code)
        .collect()
}
#[test]
fn four_shapes_use_verified_dialect_without_installation_defaults() {
    for (raw, name) in [
        (BATCH, "atlas-suite"),
        (SHIFTS, "lighthouse-suite"),
        (LEGACY, "archive-suite"),
        (MINIMAL, "sparse-suite"),
    ] {
        let s =
            parse(raw, &[], &[], &[], &ImportPolicy::default()).expect("valid synthetic fixture");
        assert_eq!(s.family.name, name);
        assert_eq!(s.sources[0].raw_text, raw);
        assert_eq!(s.parser_revision, PARSER_REVISION);
        assert_eq!(s.queue, queue());
    }
    let s =
        parse(LEGACY, &[], &[], &[], &ImportPolicy::default()).expect("valid synthetic fixture");
    assert_eq!(s.family.dayshift_prefix, None);
    assert_eq!(
        s.family.legacy_bulletshift_prefix.as_deref(),
        Some("legacy-prefix")
    );
    assert_eq!(s.family.shift_tz, None);
    let s =
        parse(MINIMAL, &[], &[], &[], &ImportPolicy::default()).expect("valid synthetic fixture");
    assert_eq!(s.family.landing, None);
    assert_eq!(s.family.base_branch, None);
    assert_eq!(s.family.shift_tz, None);
    assert_eq!(s.family.auto_shift_pr, None);
}
#[test]
fn historical_done_claimant_and_commands_never_become_acceptance_or_lease() {
    let raw = include_str!("../../../tests/fixtures/todoq/historical-done.md");
    let fs = [File::new("todos/history.md", raw)];
    let s = parse(
        BATCH,
        &fs,
        &[mapping("atlas-suite", "engine")],
        &[],
        &ImportPolicy::default(),
    )
    .expect("valid synthetic fixture");
    let t = s.records[0].todo.as_ref().expect("valid synthetic fixture");
    assert_eq!(t.declared_status.as_deref(), Some("done"));
    assert_eq!(t.declared_merged, Some(false));
    assert_eq!(t.historical_claimant.as_deref(), Some("historical-slot"));
    assert_eq!(
        t.observed_lease_until.as_deref(),
        Some("2099-01-01T00:00:00Z")
    );
    assert_eq!(t.legacy_filed_by.as_deref(), Some("claimed-requester"));
    assert!(
        s.family
            .worker_notes
            .as_ref()
            .expect("valid synthetic fixture")
            .contains("$(touch SHOULD_NOT_RUN)")
    );
    assert_eq!(
        s.family.repositories[0].gate.as_deref(),
        Some("printf gate-text-only")
    );
    assert_eq!(
        s.family.repositories[0].setup.as_deref(),
        Some("printf setup-text-only")
    );
    let suggestion = t.suggestion.as_ref().expect("valid synthetic fixture");
    assert_eq!(
        &raw[suggestion.source_span.start as usize..suggestion.source_span.end as usize],
        suggestion.text
    );
    assert!(suggestion.text.contains("Do not remove the guard"));
    assert!(suggestion.text.contains("$(touch SHOULD_NOT_RUN)"));
    assert!(codes(&s).contains(&DiagnosticCode::CriteriaUnresolved));
    assert!(codes(&s).contains(&DiagnosticCode::UnknownMetadata));
    assert_eq!(s.sources[1].raw_text, raw);
    let value = serde_json::to_value(&s).expect("valid synthetic fixture");
    assert!(value["records"][0]["todo"].get("accepted").is_none());
    assert!(value["records"][0]["todo"].get("active_lease").is_none());
}
#[test]
fn full_toml_quoted_keys_multiline_arrays_strings_dotted_tables_and_dates_parse() {
    let family = r#"["family"]
name="atlas-suite"
worker_notes="""line one
line two"""
unknown.date=2026-10-19T00:00:00Z
unknown.float=inf
[["repo"]]
name="engine"
url='https://example.invalid/engine.git'
order=0x2
"#;
    let raw = record(
        "A",
        "future = { nested = { value = 1 } }\nunknown-list = [\n'a',\n'b',\n]",
        "Paragraph one\n\nParagraph two\n",
    );
    let s = parse(
        family,
        &[File::new("todos/a.md", &raw)],
        &[],
        &[],
        &ImportPolicy::default(),
    )
    .expect("valid synthetic fixture");
    assert_eq!(s.family.repositories[0].order, Some(2));
    assert_eq!(s.family.worker_notes.as_deref(), Some("line one\nline two"));
    assert!(s.records[0].todo.is_some());
    assert!(
        s.family_diagnostics
            .iter()
            .any(|d| d.field.as_deref() == Some("/family/unknown"))
    );
}
#[test]
fn multi_repo_unknown_mapping_does_not_infer_scope() {
    let raw = record("A", "", "one request")
        .replace("atlas-suite", "lighthouse-suite")
        .replace("[\"engine\"]", "[\"engine\",\"frontend\"]");
    let fs = [File::new("todos/a.md", &raw)];
    let s = parse(
        SHIFTS,
        &fs,
        &[mapping("lighthouse-suite", "engine")],
        &[],
        &ImportPolicy::default(),
    )
    .expect("valid synthetic fixture");
    let t = s.records[0].todo.as_ref().expect("valid synthetic fixture");
    assert_eq!(t.repositories, ["engine", "frontend"]);
    assert_eq!(
        t.suggestion
            .as_ref()
            .expect("valid synthetic fixture")
            .claimed_project_id,
        None
    );
    assert!(codes(&s).contains(&DiagnosticCode::UnmappedRepository));
    let s = parse(
        SHIFTS,
        &fs,
        &[
            mapping("lighthouse-suite", "engine"),
            mapping("lighthouse-suite", "frontend"),
        ],
        &[],
        &ImportPolicy::default(),
    )
    .expect("valid synthetic fixture");
    assert_eq!(
        s.records[0]
            .todo
            .as_ref()
            .expect("valid synthetic fixture")
            .suggestion
            .as_ref()
            .expect("valid synthetic fixture")
            .claimed_project_id,
        Some(ProjectId::new("project").expect("valid synthetic fixture"))
    );
}
#[test]
fn duplicate_keys_bad_fences_empty_and_mistyped_files_remain_malformed_records() {
    let good = record("A", "", "request");
    let fs = [
        File::new("todos/a.md", &good),
        File::new(
            "todos/b.md",
            &record(
                "B",
                "id='duplicate'",
                "private-looking source is not a diagnostic",
            ),
        ),
        File::new("todos/c.md", ""),
        File::new("todos/d.md", &record("D", "mode=1", "request")),
        File::new("todos/e.md", "+++evil\nid='E'\n+++\nbody"),
    ];
    let s = parse(BATCH, &fs, &[], &[], &ImportPolicy::default()).expect("valid synthetic fixture");
    assert_eq!(s.records.len(), 5);
    assert_eq!(s.records.iter().filter(|r| r.todo.is_some()).count(), 1);
    assert_eq!(s.sources.len(), 6);
    let ds = serde_json::to_string(
        &s.records
            .iter()
            .flat_map(|r| &r.diagnostics)
            .collect::<Vec<_>>(),
    )
    .expect("valid synthetic fixture");
    assert!(!ds.contains("private-looking"));
    for c in [
        DiagnosticCode::MalformedToml,
        DiagnosticCode::MissingFence,
        DiagnosticCode::WrongType,
    ] {
        assert!(codes(&s).contains(&c));
    }
    assert_eq!(
        parse(
            "[family]\nname='a'\nname='b'",
            &[],
            &[],
            &[],
            &ImportPolicy::default()
        ),
        Err(ImportError::Family)
    );
}
#[test]
fn dependencies_missing_duplicate_ids_and_cycles_remain_unresolved() {
    let fs = [
        File::new(
            "todos/a.md",
            &record("A", "blocked_by=['B','B','missing']", "request"),
        ),
        File::new("todos/b.md", &record("B", "blocked_by=['A']", "request")),
        File::new("todos/c.md", &record("C", "blocked_by=['A']", "request")),
        File::new("todos/d.md", &record("D", "", "request")),
        File::new("todos/e.md", &record("D", "", "request")),
    ];
    let s = parse(BATCH, &fs, &[], &[], &ImportPolicy::default()).expect("valid synthetic fixture");
    assert_eq!(s.records.len(), 5);
    assert_eq!(
        s.records
            .iter()
            .filter(|r| r
                .diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::CycleOrCycleDependent))
            .count(),
        3
    );
    assert_eq!(
        s.records
            .iter()
            .filter(|r| r
                .diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::DuplicateTaskId))
            .count(),
        2
    );
    assert!(codes(&s).contains(&DiagnosticCode::MissingDependency));
    assert!(codes(&s).contains(&DiagnosticCode::DuplicateDependency));
}
#[test]
fn unknown_values_alias_conflict_placeholder_and_history_are_explicit() {
    let raw = record(
        "A",
        "status='future-state'\nmode='soon'\nrequested_by='one'\nfiled_by='two'\nworked_by=[{outcome='done',cost_usd=0.0}]",
        "TBD",
    );
    let s = parse(
        BATCH,
        &[File::new("todos/a.md", &raw)],
        &[],
        &[],
        &ImportPolicy::default(),
    )
    .expect("valid synthetic fixture");
    for c in [
        DiagnosticCode::UnknownStatus,
        DiagnosticCode::UnknownMode,
        DiagnosticCode::RequesterAliasConflict,
        DiagnosticCode::ImportedHistoryUnverified,
        DiagnosticCode::Placeholder,
    ] {
        assert!(codes(&s).contains(&c));
    }
    assert_eq!(
        s.records[0]
            .todo
            .as_ref()
            .expect("valid synthetic fixture")
            .declared_status
            .as_deref(),
        Some("future-state")
    );
}
#[test]
fn replay_file_order_and_pages_bind_exact_manifest() {
    let a = File::new("todos/a.md", &record("A", "", "one\n\ntwo"));
    let b = File::new("todos/b.md", &record("B", "status='closed'", "request"));
    let first =
        parse(BATCH, &[b, a], &[], &[], &ImportPolicy::default()).expect("valid synthetic fixture");
    let prior: Vec<_> = first
        .sources
        .iter()
        .map(|s| PriorSource {
            source_key: s.source_key.clone(),
            raw_sha256: s.raw_sha256.clone(),
        })
        .collect();
    let a = File::new("todos/a.md", &record("A", "", "one\n\ntwo"));
    let b = File::new("todos/b.md", &record("B", "status='closed'", "request"));
    let second = parse(BATCH, &[a, b], &[], &prior, &ImportPolicy::default())
        .expect("valid synthetic fixture");
    assert_eq!(first, second);
    let page = first.page(None, 1).expect("valid synthetic fixture");
    assert_eq!(page.records.len(), 1);
    assert_eq!(page.sources[0].path, "family.toml");
    assert_eq!(page.sources[1].path, "todos/a.md");
    let next = first
        .page(page.continuation.as_deref(), 1)
        .expect("valid synthetic fixture");
    assert_eq!(next.sources[0].path, "family.toml");
    assert_eq!(next.sources[1].path, "todos/b.md");
    assert!(next.continuation.is_none());
    assert_eq!(first.page(None, 101), Err(ImportError::Cursor));
    assert_eq!(first.page(Some("foreign:0"), 1), Err(ImportError::Cursor));
    assert_eq!(
        first.page(Some(&format!("{}:01", first.snapshot_digest)), 1),
        Err(ImportError::Cursor)
    );
    let mut changed = first;
    changed.records.clear();
    assert_eq!(changed.page(None, 1), Err(ImportError::Cursor));
}
#[test]
fn raw_mismatch_and_prior_identity_collision_fail_closed() {
    let old = parse(
        BATCH,
        &[File::new("todos/a.md", &record("A", "", "before"))],
        &[],
        &[],
        &ImportPolicy::default(),
    )
    .expect("valid synthetic fixture");
    let mut changed = File::new("todos/a.md", &record("A", "", "after"));
    changed.blob = old.sources[1].blob_oid.clone();
    let prior = [PriorSource {
        source_key: old.sources[1].source_key.clone(),
        raw_sha256: old.sources[1].raw_sha256.clone(),
    }];
    assert_eq!(
        parse(BATCH, &[changed], &[], &prior, &ImportPolicy::default()),
        Err(ImportError::SourceCollision)
    );
    let mut wrong = File::new("todos/a.md", &record("A", "", "after"));
    wrong.digest = old.sources[1].raw_sha256.clone();
    assert_eq!(
        parse(BATCH, &[wrong], &[], &[], &ImportPolicy::default()),
        Err(ImportError::Digest)
    );
}
#[test]
fn duplicate_paths_repos_and_mappings_never_choose_a_winner() {
    let fs = [
        File::new("todos/a.md", &record("A", "", "one")),
        File::new("todos/a.md", &record("B", "", "two")),
    ];
    assert_eq!(
        parse(BATCH, &fs, &[], &[], &ImportPolicy::default()),
        Err(ImportError::SourceCollision)
    );
    let duplicate =
        format!("{BATCH}\n[[repo]]\nname='engine'\nurl='https://example.invalid/other.git'\n");
    assert_eq!(
        parse(&duplicate, &[], &[], &[], &ImportPolicy::default()),
        Err(ImportError::Family)
    );
    let m = mapping("atlas-suite", "engine");
    assert_eq!(
        parse(BATCH, &[], &[m.clone(), m], &[], &ImportPolicy::default()),
        Err(ImportError::Mapping)
    );
}
#[test]
fn bytes_records_fields_nodes_depth_edges_and_diagnostic_caps_never_evict() {
    let too_large = "x".repeat(1024 * 1024);
    assert_eq!(
        parse(
            BATCH,
            &[File::new("todos/a.md", &too_large)],
            &[],
            &[],
            &ImportPolicy::default()
        ),
        Err(ImportError::Capacity)
    );
    let fs: Vec<_> = (0..1001)
        .map(|i| {
            File::new(
                &format!("todos/{i}.md"),
                &record(&format!("T{i}"), "", "request"),
            )
        })
        .collect();
    assert_eq!(
        parse(BATCH, &fs, &[], &[], &ImportPolicy::default()),
        Err(ImportError::Capacity)
    );
    let deep = format!("future={}0{}", "[".repeat(33), "]".repeat(33));
    assert_eq!(
        parse(
            BATCH,
            &[File::new("todos/a.md", &record("A", &deep, "request"))],
            &[],
            &[],
            &ImportPolicy::default()
        ),
        Err(ImportError::Capacity)
    );
    let field = format!("future={:?}", "x".repeat(65537));
    assert_eq!(
        parse(
            BATCH,
            &[File::new("todos/a.md", &record("A", &field, "request"))],
            &[],
            &[],
            &ImportPolicy::default()
        ),
        Err(ImportError::Capacity)
    );
    let policy = ImportPolicy {
        max_nodes: 1,
        ..ImportPolicy::default()
    };
    assert_eq!(
        parse(BATCH, &[], &[], &[], &policy),
        Err(ImportError::Capacity)
    );
    let deps = format!(
        "blocked_by=[{}]",
        (0..4096)
            .map(|i| format!("'missing-{i}'"))
            .collect::<Vec<_>>()
            .join(",")
    );
    let fs = [
        File::new("todos/a.md", &record("A", &deps, "request")),
        File::new("todos/b.md", &record("B", "blocked_by=['A']", "request")),
    ];
    assert_eq!(
        parse(BATCH, &fs, &[], &[], &ImportPolicy::default()),
        Err(ImportError::Capacity)
    );
    let unknown = (0..8)
        .map(|i| format!("unknown{i}=true"))
        .collect::<Vec<_>>()
        .join("\n");
    let fs: Vec<_> = (0..500)
        .map(|i| {
            File::new(
                &format!("todos/{i}.md"),
                &record(&format!("T{i}"), &unknown, "request"),
            )
        })
        .collect();
    assert_eq!(
        parse(BATCH, &fs, &[], &[], &ImportPolicy::default()),
        Err(ImportError::Capacity)
    );
}
#[test]
fn unsafe_pins_utf8_and_invalid_policy_fail_without_using_paths() {
    let q = queue();
    let f = File::new("family.toml", BATCH);
    let bad = File::new("todos/../escape.md", &record("A", "", "request"));
    assert_eq!(
        parse_pinned_snapshot(
            &PinnedSnapshot {
                queue: &q,
                family: f.pinned(),
                todos: &[bad.pinned()],
                mappings: &[],
                prior_sources: &[]
            },
            &ImportPolicy::default()
        ),
        Err(ImportError::Pin)
    );
    let mut bad = File::new("todos/a.md", "source");
    bad.bytes = vec![255];
    bad.digest = format!("sha256:{:x}", Sha256::digest(&bad.bytes));
    assert_eq!(
        parse(BATCH, &[bad], &[], &[], &ImportPolicy::default()),
        Err(ImportError::Pin)
    );
    for policy in [
        ImportPolicy {
            max_bytes: 0,
            ..ImportPolicy::default()
        },
        ImportPolicy {
            max_records: 1001,
            ..ImportPolicy::default()
        },
        ImportPolicy {
            max_depth: 33,
            ..ImportPolicy::default()
        },
    ] {
        assert_eq!(
            parse(BATCH, &[], &[], &[], &policy),
            Err(ImportError::Policy)
        );
    }
}
#[test]
fn git_pins_are_preserved_claims_not_authenticated_commit_membership() {
    let first =
        parse(BATCH, &[], &[], &[], &ImportPolicy::default()).expect("valid synthetic fixture");
    let q = QueuePin {
        commit_oid: "d".repeat(64),
        ..queue()
    };
    let f = File::new("family.toml", BATCH);
    let changed = parse_pinned_snapshot(
        &PinnedSnapshot {
            queue: &q,
            family: f.pinned(),
            todos: &[],
            mappings: &[],
            prior_sources: &[],
        },
        &ImportPolicy::default(),
    )
    .expect("valid synthetic fixture");
    assert_ne!(first.snapshot_digest, changed.snapshot_digest);
    assert_ne!(first.sources[0].source_key, changed.sources[0].source_key);
    assert_eq!(first.sources[0].raw_sha256, changed.sources[0].raw_sha256);
}
#[test]
fn empty_body_is_placeholder_without_invalid_empty_span() {
    let s = parse(
        BATCH,
        &[File::new("todos/a.md", &record("A", "", ""))],
        &[],
        &[],
        &ImportPolicy::default(),
    )
    .expect("valid synthetic fixture");
    assert!(
        s.records[0]
            .todo
            .as_ref()
            .expect("valid synthetic fixture")
            .suggestion
            .is_none()
    );
    assert!(codes(&s).contains(&DiagnosticCode::Placeholder));
}

#[test]
fn foreign_family_record_never_resolves_dependency_or_claimed_project() {
    let a = File::new("todos/a.md", &record("A", "blocked_by=['B']", "request"));
    let b = File::new(
        "todos/b.md",
        &record("B", "", "request").replace("atlas-suite", "foreign-suite"),
    );
    let s = parse(
        BATCH,
        &[a, b],
        &[mapping("atlas-suite", "engine")],
        &[],
        &ImportPolicy::default(),
    )
    .expect("valid synthetic fixture");
    assert!(
        s.records[0]
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::MissingDependency)
    );
    assert!(
        s.records[1]
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::FamilyMismatch)
    );
    assert!(
        s.records[1]
            .todo
            .as_ref()
            .expect("valid synthetic fixture")
            .suggestion
            .as_ref()
            .expect("valid synthetic fixture")
            .claimed_project_id
            .is_none()
    );
}

#[test]
fn unicode_crlf_body_offsets_are_raw_bytes_and_do_not_split_paragraphs() {
    let raw = record(
        "UTF8",
        "status='open'",
        "Snowman ☃ preserves 5 µs.\n\nDo not drop negation.\n",
    )
    .replace('\n', "\r\n");
    let s = parse(
        BATCH,
        &[File::new("todos/unicode.md", &raw)],
        &[],
        &[],
        &ImportPolicy::default(),
    )
    .expect("valid synthetic fixture");
    let p = s.records[0]
        .todo
        .as_ref()
        .expect("valid synthetic fixture")
        .suggestion
        .as_ref()
        .expect("valid synthetic fixture");
    assert_eq!(
        &raw[p.source_span.start as usize..p.source_span.end as usize],
        p.text
    );
    assert!(p.text.contains("☃"));
    assert!(p.text.contains("\r\n\r\n"));
    assert_eq!(s.records.len(), 1);
}
#[test]
fn exact_record_cap_pages_without_eviction_or_duplicate_omission() {
    let fs: Vec<_> = (0..1000)
        .map(|i| {
            File::new(
                &format!("todos/{i}.md"),
                &record(&format!("T{i}"), "status='closed'", "one request"),
            )
        })
        .collect();
    let s = parse(
        BATCH,
        &fs,
        &[mapping("atlas-suite", "engine")],
        &[],
        &ImportPolicy::default(),
    )
    .expect("valid synthetic fixture");
    assert_eq!(s.records.len(), 1000);
    let mut cursor = None;
    let mut ids = std::collections::BTreeSet::new();
    let mut offset = 0;
    loop {
        let p = s
            .page(cursor.as_deref(), 100)
            .expect("valid synthetic fixture");
        assert!(p.records.len() <= 100);
        assert_eq!(usize::from(p.record_offset), offset);
        assert_eq!(p.sources[0], s.sources[0]);
        assert_eq!(p.queue, s.queue);
        assert_eq!(p.family, s.family);
        assert_eq!(p.family_diagnostics, s.family_diagnostics);
        for (local, r) in p.records.iter().enumerate() {
            let original = &s.records[offset + local];
            let source = &p.sources[usize::from(r.source_index)];
            assert_eq!(source, &s.sources[usize::from(original.source_index)]);
            assert_eq!(r.todo, original.todo);
            assert_eq!(r.diagnostics, original.diagnostics);
            let todo = r.todo.as_ref().expect("valid synthetic fixture");
            let suggestion = todo.suggestion.as_ref().expect("nonempty body");
            assert_eq!(
                &source.raw_text
                    [suggestion.source_span.start as usize..suggestion.source_span.end as usize],
                suggestion.text,
            );
            assert!(ids.insert(todo.id.clone()));
        }
        offset += p.records.len();
        cursor = p.continuation;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(ids.len(), 1000);
}

#[test]
fn page_indices_resolve_exact_sources_including_malformed_and_later_pages() {
    let files = [
        File::new(
            "todos/a.md",
            &record("A", "status='open'", "Snowman ☃.\n\nSecond paragraph."),
        ),
        File::new(
            "todos/b.md",
            "+++\nid='broken'\nid='duplicate'\n+++\nMalformed body remains raw.\n",
        ),
        File::new("todos/c.md", &record("C", "", "Last exact request.")),
    ];
    let snapshot = parse(BATCH, &files, &[], &[], &ImportPolicy::default())
        .expect("bounded synthetic snapshot");
    let mut cursor = None;
    let mut offset = 0;
    loop {
        let page = snapshot.page(cursor.as_deref(), 1).expect("bounded page");
        assert_eq!(usize::from(page.record_offset), offset);
        assert_eq!(page.sources[0], snapshot.sources[0]);
        assert_eq!(page.queue, snapshot.queue);
        assert_eq!(page.family, snapshot.family);
        assert_eq!(page.family_diagnostics, snapshot.family_diagnostics);
        for (local, record) in page.records.iter().enumerate() {
            assert_eq!(usize::from(record.source_index), local + 1);
            let expected = &snapshot.records[offset + local];
            let original_source = &snapshot.sources[usize::from(expected.source_index)];
            let resolved_source = page
                .sources
                .get(usize::from(record.source_index))
                .expect("every page record index must resolve in its own page");
            assert_eq!(resolved_source, original_source);
            assert_eq!(record.todo, expected.todo);
            assert_eq!(record.diagnostics, expected.diagnostics);
            if let Some(suggestion) = record
                .todo
                .as_ref()
                .and_then(|todo| todo.suggestion.as_ref())
            {
                assert_eq!(
                    &resolved_source.raw_text[suggestion.source_span.start as usize
                        ..suggestion.source_span.end as usize],
                    suggestion.text,
                );
            }
        }
        offset += page.records.len();
        cursor = page.continuation;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(offset, 3);
    assert!(snapshot.records[1].todo.is_none());
}
