# JSON workplan import preview

`admiralty_core::workplans::preview_workplan` accepts a UTF-8 JSON document containing a
`Workplan` with `schema_version`, `plan_id`, `revision`, `project_id`, and a
`tasks` array of complete `TaskContract` values. The current workplan schema
version is `1`. Both the plan and task contracts reject unknown fields.

Import is bounded to 1 MiB and 1000 tasks. The preview validates each task
contract, requires every task to belong to the plan project, rejects duplicate
task IDs and missing, duplicate, or self dependencies, and detects dependency
cycles. It returns a deterministic dependency first task order (ties sort by
task ID) and a `sha256:` digest of the original source bytes. Replaying the same
bytes yields the same preview and digest.

This API only parses and previews. It does not submit tasks, write plan state,
resolve external references, or claim that project ownership and interface
availability have been checked. Callers must treat a preview as validation
input, not an admission or execution decision.
