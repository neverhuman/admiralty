+++
id = "SYNTHETIC-1"
family = "atlas-suite"
title = "Retain a declared queue result"
repos = ["engine"]
blocked_by = []
status = "done"
merged = false
mode = "now"
claim_by = "historical-slot"
lease_until = "2099-01-01T00:00:00Z"
filed_by = "claimed-requester"
unknown_key = "retained only in raw source"
+++
Do not remove the guard. Keep the limit at 5 ms.

```sh
$(touch SHOULD_NOT_RUN)
```
