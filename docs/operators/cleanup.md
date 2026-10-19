# Registered local scratch retirement

The Linux `workspace::cleanup::CleanupExecutor` retires explicitly registered,
private local scratch. Registration occurs at creation under a trusted owner;
discovery and worker reports never register assets. The caller supplies separate,
existing mode-0700 managed and evidence directories. One executor holds the
evidence ledger lock. Roots, directory entries and files are opened relative to
pinned descriptors without following symlinks. Linux mount IDs, device/inode
identities and exact content hashes are retained in the registration receipt.

Use `register(scope, name, process_ids)` to capture the initial scratch manifest
and complete managed process set at creation, including boot ID and start ticks. Bind the returned
`registration_digest` to the store's `AssetBinding.sha256`, register durable
custody, then obtain `custody::retirement_fence` using an exact scoped claim.
`preview`, `claim`, `quarantine` and `delete` are separate operations. Every
effect revalidates custody history and registration. Durable retirement claims
prevent new pins; an existing pin wins the transaction race and blocks the
executor. Custody receipts alone remain recorded assertions; only the explicit
local executor performs filesystem operations.

Fresh `Preservation` facts must come from the trusted lifecycle/backup owners.
Every field must be known and false: dirty, ignored/private, unpushed, unmerged,
process-held, foreign, backup/candidate-held and unique data. Unknown is a hold.
The executor additionally rechecks every registered Linux process. A live PID,
PID reuse, inaccessible identity or over-limit observation preserves the asset.
This is a trusted local service boundary: registration must include all managed
processes, callers must stop managed writers before retirement, and new
references must use durable pins. Unknown process ownership stays a hold. It
does not isolate hostile same-UID or root processes; arbitrary process discovery
and fleet process fencing require their backend qualification.

The qualified disposable format is a flat directory with at most 128 private,
single-link regular files, totaling at most 16 MiB. Changed or additional bytes,
dotfiles, nested directories, `.git`, symlinks, hardlinks, special files and mount
changes block retirement. Git worktrees are preserved for separate verified
`git worktree remove` handling; the executor never force-removes a worktree,
recurses into directories, kills processes or traverses foreign roots.

Quarantine uses an atomic no-replace rename inside the managed root. Durable
intent is synced before rename/unlink; directories are synced afterward. Reopen
the same roots and replay the same durable custody claim after a crash. Repeat
`quarantine` to resolve a rename intent, then `delete` to finish an interrupted
delete. Missing members are allowed only after durable delete intent; unexpected
content or ambiguous original/quarantine locations remain held for inspection.
The evidence directory and custody database must be backed up together.

Receipts retain scope, registered byte debt, stage and disposed bytes as exact
decimal strings. Disposal credit appears only after completed deletion. The
registration manifest and final receipt remain outside the retired directory;
evidence, pins and backup holds are never removed to improve cleanup metrics.

Run `cargo test -p admiralty-testkit --test cleanup_custody`. The suite uses
disposable roots and covers real deletion, durable pin exclusion, preservation
holds, root/directory replacement, symlink/private/Git content, an actual live
child process, foreign custody, writer exclusion and interrupted retirement.
Linux mount identity comparison is enforced; privileged bind-mount injection
and isolated fleet execution are separate backend qualifications.
