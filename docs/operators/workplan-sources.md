# Workplan source preview

`admiralty workplan preview --file plan.toml --format toml` validates the same
closed task contracts as JSON. The result contains the canonical workplan and
its digest, the original source digest, and the parser version. Save and review
the canonical JSON before using the existing explicit `workplan register` flow.
Preview creates no database and grants no execution authority.

`admiralty workplan preview --file tasks.md --format markdown` reads bounded,
explicit task tables:

```markdown
| id | title | dependencies | owned_paths | criteria | status |
| --- | --- | --- | --- | --- | --- |
| T2 | Check the importer | T1 | src/import | Named checks pass | PASS |
```

Comma-separated dependencies, paths, and criteria are retained as observations.
IDs, source byte spans and legacy status survive preview. Code fences are inert;
checkboxes, duplicate IDs, malformed rows and missing contracts produce review
diagnostics. Imported PASS or checked boxes never create trusted acceptance.
Markdown always requires explicit scope resolution and a complete typed contract
before registration. It does not infer project, root, checks or authority from
prose. JSON remains the default format and the registration format.
