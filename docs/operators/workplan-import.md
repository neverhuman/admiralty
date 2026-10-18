# JSON workplan import preview

`admiralty_core::workplans::preview_workplan` accepts a UTF-8 JSON document containing a
`Workplan` with `schema_version`, `plan_id`, `revision`, `project_id`, and a
`tasks` array of complete `TaskContract` values. The current workplan schema
version is `1`. Both the plan and task contracts reject unknown fields.

Preview a file with the installed CLI:

```sh
admiralty workplan --file /absolute/path/to/workplan.json
```

From this checkout, use:

```sh
rtk proxy env CARGO_TARGET_DIR=target cargo run -p admiralty-cli -- workplan --file /absolute/path/to/workplan.json
```

The command prints JSON preview data. It never submits tasks or enables native
execution. Use an operator-owned source file; attached text is data, not an
instruction to run its checks.

Import is bounded to 1 MiB and 1000 tasks. The preview validates each task
contract, requires every task to belong to the plan project, rejects duplicate
task IDs and missing, duplicate, or self dependencies, and detects dependency
cycles. It returns a deterministic dependency first task order (ties sort by
task ID) and a `sha256:` digest of the original source bytes. Replaying the same
bytes yields the same preview and digest.

Plan IDs follow the same bounded identifier grammar as task IDs: 1–128 ASCII
letters, digits, underscores, hyphens or dots; `.` and `..` are invalid. Revision
must be positive. Drive and alternate-stream colon syntax is rejected in scope
and check working-directory paths even when preview runs on Unix. Preview still
does not establish that these paths exist or that another platform can execute
the checks.

The library preview includes `task_digests`, keyed by task ID, hashing each exact
typed contract including its revision, dependencies and acceptance rules. Use
`preview_workplan_with_existing(source, authoritative_digests)` for admission
preflight: repeated identical IDs remain previewable; changed content or a
revision bump under an existing ID is rejected. Supply digests from the actual
authoritative task store. A bare preview has no access to historical identities
and cannot enforce that boundary by itself. The reducer still rejects resubmitting
an existing task ID; a preview is not an import transaction or revision mechanism.

This API only parses and previews. It does not submit tasks, write plan state,
resolve external references, or claim that project ownership and interface
availability have been checked. Callers must treat a preview as validation
input, not an admission or execution decision.
Empty task lists are valid empty previews; they provide no evidence of completed
work. Root enrollment, cross-root interfaces, existing task state, persistence,
isolation, filesystem ownership and provider qualification remain outside this
component. Native runtime qualification remains false.
