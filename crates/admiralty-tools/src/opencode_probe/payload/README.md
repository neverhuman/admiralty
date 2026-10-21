# OpenCode qualification payload

`admiralty opencode probe` sends these files over SSH as data; nothing here runs
on the Admiralty host.

- `remote_harness.txt` is passed to the qualified node's own `/usr/bin/python3 -c`.
  It creates the private candidate checkout, including the deliberate
  `NotImplementedError` stub that the coding agent under test must replace,
  runs the agent and the owner oracle, and prints a bounded JSON result.
- `owner_oracle.txt` is the independent `unittest` program for that task. Its
  SHA-256 (`oracle_owner_expected_sha256`) is recorded in every receipt, so
  changing a byte makes new results incomparable with earlier ones.

Both files are byte-identical to the constants of the retired
`tools/probe_opencode.py`; `opencode_probe_tests.rs` pins their digests.
