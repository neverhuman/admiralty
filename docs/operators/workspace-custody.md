# Workspace custody registry

`admiralty-runtime::workspace::registry` validates an existing local workspace
and records its observed identity. A valid record contains typed node, root,
and project references; an existing canonical absolute workspace directory;
relative owned and forbidden path roots; and a read-only observation of
`.git` metadata.

Workspace paths must already be absolute and canonical. The filesystem root,
the environment-declared home directory (`HOME`, falling back to `USERPROFILE`), missing paths, non-directories, and any
workspace path containing a symlink are rejected. Owned and forbidden paths
must be nonempty relative paths made only of normal components. Scope matching
uses path components, so `src-old` is a sibling of `src`, not a child. A
forbidden path always overrides an owned path, including its descendants.

Bounds: at most 1024 owned and forbidden roots combined; workspace and scope
paths at most 4096 bytes and 128 components. The initial scope policy accepts
ASCII relative names only. It rejects backslashes, drive/alternate-stream
colons, control characters, dot/empty components, trailing dots/spaces, wildcard
syntax and Windows device-name aliases. These conservative limits avoid
platform-dependent interpretation; they do not constitute Windows execution
qualification. Owned matching is exact; forbidden matching additionally denies
ASCII case variants. Existing symlinks inside owned scope are rejected and
`owns()` refuses candidates with an observed symlink prefix.

On macOS, canonicalize the selected workspace path before registration; aliases
such as `/var` may resolve through `/private/var`. Registration never silently
adopts a different spelling of the workspace root. Filesystem roots are rejected
on every platform, rather than recognizing only `/`.

Git metadata observation only distinguishes absent metadata, a `.git`
directory, a structurally resolved linked-worktree `.git` pointer file, and unknown metadata.
It does not invoke Git, inspect hooks, read status, or apply attributes or
filters. Malformed or unresolved pointer files are reported as unknown.

Pointer reads are capped at 4096 bytes, require a regular UTF-8 file, and use
nonblocking/no-follow opens on Unix with an opened-file identity check. FIFO,
symlinked, oversized and multi-line pointer files are unknown. Relative Git
pointer targets may traverse to an external common metadata directory, but their
normalized components must exist without symlink ancestors. That observation
does not prove the directory is valid Git metadata, establish repository
identity, or grant custody over the target. Permission and other I/O failures
are unknown, not evidence that metadata is absent.

The resulting custody value is `registered_only`. It records who and what the
workspace belongs to; it does not authorize execution, writes, cleanup, or
adoption. Callers must obtain those capabilities from their separate authority
boundaries. This registry does not provide race-free protection against a
filesystem change after validation; callers that later act on a path must
revalidate identity at that action boundary.
This is a bounded local observation prototype, tested on the current host with
Unix-specific protections. It does not provide a retained directory handle,
race-free sandbox, mount or hard-link exclusion, Unicode path qualification,
independent operating-system home discovery, Windows runner qualification, or
exclusive custody. Public records can be copied or modified; neither a record
nor `owns()` is an unforgeable effect capability. No Git command, hook, shell,
provider, cleanup action or external communication runs during validation.
