# Access projection integration test evidence

The `access_projection` test binds the existing Axum application to a
test-owned `127.0.0.1:0` listener and sends bounded raw HTTP requests over Tokio
TCP. It opens an owned private Store fixture and exercises the actual fleet,
health, model, legacy, and unknown-route handlers. The private fixture files use
mode `0600` beneath a mode `0700` temporary directory; each server task is
aborted and awaited by its own test.

The tested transport boundary requires the exact configured bearer for canonical
fleet and model reads and model selection. Health is read-only and reports
`native_execution_qualified: false`. Authorized fleet and model responses keep
simulation and qualification false and hide model endpoints and credential
references. Authorized model selection returns the configured selection
proposal; the test makes no provider call or execution claim. A configured
`legacy.public_read` permits only its configured legacy snapshot read. It does
not remove authentication from canonical fleet/model routes. Unknown API paths
return 404. The synthetic demo projection is labeled simulated and the
existing app constructor rejects real model or legacy attachments.

This test proves only these existing route semantics in a private local
fixture. It does not prove activation in a running service, browser/session
security, TLS or role scoping, live provider execution, native process
supervision, external-source qualification, or acceptance. No service is
started outside the test-owned ephemeral listener, and no existing port is
probed or contacted.
