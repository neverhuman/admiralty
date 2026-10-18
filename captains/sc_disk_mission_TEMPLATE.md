# Disk cleanup mission (delegated by super_captain) — FILL IN BEFORE SENDING
Host: <xbabeN>   Filesystem: <mount>   Now: <pct>% used, <free> GB free, growth <GB/h>, ~<h> h to full (from globalres.py at <UTC>)
Target: get below 85% used (or recover ~<N> GB) using ONLY the allowed actions below. Stop when reached.

Allowed (smallest risk first; record size before/after of each path in sc_disk_receipt_<host>.md, append-only, with UTC times):
1. `du -xsh` survey first (top 30 dirs under the mount, `-x` to stay on one filesystem). Write the survey to the receipt before deleting anything.
2. Regenerable build caches: target/ of merged+inactive worktrees, __pycache__, .pytest_cache, ~/.cache/pip, ~/.npm/_cacache, ~/.cargo/registry/cache.
3. (removed: /tmp files are allowed only if this session created them)
4. Worktrees of MERGED branches: status --porcelain empty, branch merged into origin/main, record branch + `git log -1 --format='%H %s'` in sc_disk_archive_<host>.md, then `git worktree remove <path>` (no --force).
5. Logs >100 MB and >1 day old: compress, verify, then remove the uncompressed copy.

Forbidden: datasets, results, checkpoints, weights, evidence/receipts/custody, S3-mirrored data, unmerged/dirty worktrees, .git objects, Docker volumes/images, anything of Jeryu or other services/users, /mnt/*-archive, /mnt/amphora-backup, /mnt/xbabe3-external, rm -rf on any home-level/top-level dir, sudo, killing processes, xbabe0 (never connect to it).
If the allowed actions can't reach the target, STOP and write `NEEDS JEPSON: <host> <mount> still <pct>%; biggest remaining: <paths + sizes>` at the end of the receipt.
When finished, write `MISSION DONE <UTC>` with total GB recovered at the end of the receipt.

## HARD SCOPE LIMIT (added 03:55Z after the xbabe3 delegate deleted Jeryu build dirs)
- You may ONLY touch: (a) user caches ~/.cache/pip, ~/.npm/_cacache, ~/.cargo/registry/cache; (b) paths INSIDE these project trees and their lane/worktree dirs: ~/joil, ~/dope, ~/JopeDime, ~/jarambit, ~/jope-evidence, ~/jarambit_*, ~/lanes/<project>; (c) temp files you created yourself in this session.
- Any path whose text contains `jeryu`, `jain`, `Jeryu`, `runner`, `actions-runner`, `docker`, `containerd`, `buildkit`, or another user/service name is FORBIDDEN, even if it is a regenerable `target/` dir. Do not even plan deletions there.
- target/ dirs only for worktrees whose branch is MERGED into origin/main (check with `git branch -r --merged origin/main`). "Inactive but not merged" is NOT allowed.
- Do NOT delete /tmp files you did not create, even if old.
- Before EACH deletion, write the exact path and the rule letter (a/b/c) that allows it into the receipt. If no rule letter fits, do not delete it.
