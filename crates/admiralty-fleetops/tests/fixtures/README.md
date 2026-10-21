# Captured parity fixtures

These are synthetic terminal and resource observations, not live sessions.
Terminal prompts deliberately retain trailing spaces and CRLF. The scoped
Git attributes preserve those exact bytes without disabling source checks or
secret scanning.

The delivery harness explicitly creates the original host's `/tmp` and
captain-directory equivalents under its private sandbox. Git cannot retain
empty directories. `send_add1/empty-seed.txt` declares that it starts with no
seed files; any other missing seed still fails.

`kick/*/expected_kick.txt` was reconstructed by running the original
`2f5b9315` kick/classifier/screen helpers with a frozen clock and simulated
screen only. All11 pre-existing stdout, stderr, state and keystroke expectations
matched before the log files were adopted. Resource `expected_global_resources.json`
comes from the original sampler with the committed probe inputs; its existing
table and history matched byte-for-byte. No expected output came from the new
implementation. Original failed attempts and raw oracle proofs remain retained
in the maintainer's private integration evidence.
