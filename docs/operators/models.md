# Own models and routing policy

Admiralty's editable model registry places self-hosted models below external
models in the configured order. The [example registry](../../configs/examples/models.json)
starts with Ornith at tier `0`, then Luna at tier `1`. These are example data;
model names, node IDs, endpoints and tier order are not fleet constants.
Lower signed integers are preferred. Tier order expresses policy, not a measured
price or a claim about provider quality.

The registry and selection helpers are functional local configuration. They do
not qualify native execution, connect to an endpoint, start SSH, read credential
files, reserve real inference slots, or measure savings. A selected model is a
routing proposal; the execution owner must atomically reserve capacity and use
a qualified provider adapter before launching it.

Initialize a dedicated private policy file with the installed CLI:

```sh
task_models_dir="$HOME/.local/state/admiralty/models"
install -d -m 700 "$task_models_dir"
admiralty models replace --registry "$task_models_dir/models.json" \
  --file configs/examples/models.json --expected-revision 0
admiralty models list --registry "$task_models_dir/models.json"
```

Later replacements require the revision returned by `list`. The `--file` input
is a complete registry document; its model definitions are validated before
publication. Keep your edited source document outside Git if it contains
installation-specific endpoints or credential-file references.

Store the registry in an owned private directory (`0700`) as an owned file
(`0600`). The example is public configuration material, so copy it into private
state before using a registry file reader. The file reader rejects symlinks,
multiple links, unknown fields, duplicate model IDs, invalid escalation graphs
and inputs larger than one MiB. It never reads the referenced credential files.

Each model defines its provider, capabilities, maximum concurrency and maximum
attempts per task at that tier. Models in the same tier share the same attempt
cap. The example local maximum of `16` is configurable; it is not a measured
capacity guarantee. Counts in `CapacitySnapshot` are observations, and this
selection module does not replace the scheduler's reservation transaction.
`id` is the stable registry key; required `provider_model` names the native model
(for example `ornith/Ornith-1.5-35B-A3B`) independently of the label and routing key.

Supported registry provider values:

| Provider | Configuration |
| --- | --- |
| `openai_compatible` | Explicit HTTP(S) endpoint without URL credentials, query parameters or fragments. |
| `opencode_ssh` | Configured `node_id` and optional `ssh://profile-name` reference; never a shell command. Transport remains unqualified until its adapter passes. |
| `codex_cli` | Qualified CLI transport selected elsewhere; `endpoint` stays `null`. |

`credential_file_ref` is either `null` or a relative `credentials/...` reference.
Do not put API keys, passwords, SSH commands, authorization headers or private
keys in this file. Secret fields are absent from the schema and common token
markers are rejected. The public model list omits endpoint and credential-file
references. Local models' usage costs remain `{"status":"unknown"}` until a
qualified accounting source exists; self-hosted usage is never silently priced
at zero.

Selection is deterministic: enabled capable models sort by tier, then ID. A
pinned task model is honored and never silently replaced. If every capable
model in the cheapest tier is full or unavailable, the default result is
`waiting`. It does not turn a queue into an expensive model run. The caller may
explicitly enable `allow_capacity_fallback`; the default is `false`.

Task failure evidence uses one of `failure`, `test_fail`, `context_exhausted` or
`provider_unavailable`. Ordinary failures retry within the configured tier cap.
When that cap is reached, escalation requires both explicit failure evidence
and `allow_escalation=true`. Context exhaustion/provider unavailability may
propose the configured next model immediately under that same explicit policy.
Missing, disabled or incapable next models remain visible waiting states.
Escalation follows `escalate_to`; cycles and decreasing tier order are invalid.
`setup_error` and `blocked_input` always produce `setup_required` or `needs_input`;
an unborn checkout, denied filesystem permission or unanswered prompt cannot
cause token escalation, even when escalation flags are enabled.

Dry-run routing uses `admiralty models select --registry /absolute/models.json
--request /absolute/request.json --capacity /absolute/capacity.json`. A minimal
request is `{"required_capabilities":["coding"]}`; initial capacity can be
`{"in_use":{},"unavailable":[]}`. These are observations for a proposal, not
proof that slots are reserved. Authenticated HTTP equivalents are `GET`/`PUT`
`/v1/models` and `POST /v1/models/select` with body
`{"request":{...},"capacity":{...}}`.

Updates use optimistic revisions. Read the current decimal-string `revision`,
then submit `{"expected_revision":"<current>","models":[...]}`. A successful
atomic replacement increments it once. Concurrent stale writes return conflict;
they cannot silently overwrite another operator's update. A busy writer is a
retryable error. After any filesystem/durability error, read current revision
before retrying because publication may already have completed. No model routes
should be registered without authentication, including local configuration
reads, and demo mode must reject configuration writes.
Public-list round trips preserve existing endpoint and credential references
when omitted or `null` in an update. To deliberately clear those references or
change transport type, edit the complete private configuration through the
registry's revision-checked file interface; list responses never expose them.
