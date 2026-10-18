# Workspace custody registry

`admiralty-runtime::workspace::registry` validates an existing local workspace
and records its observed identity. A valid record contains typed node, root,
and project references; an existing canonical absolute workspace directory;
relative owned and forbidden path roots; and a read-only observation of
`.git` metadata.

Workspace paths must already be absolute and canonical. The filesystem root,
the current user's home directory, missing paths, non-directories, and any
workspace path containing a symlink are rejected. Owned and forbidden paths
must be nonempty relative paths made only of normal components. Scope matching
uses path components, so `src-old` is a sibling of `src`, not a child. A
forbidden path always overrides an owned path, including its descendants.

Git metadata observation only distinguishes absent metadata, a `.git`
directory, a valid linked-worktree `.git` pointer file, and unknown metadata.
It does not invoke Git, inspect hooks, read status, or apply attributes or
filters. Malformed or unresolved pointer files are reported as unknown.

The resulting custody value is `registered_only`. It records who and what the
workspace belongs to; it does not authorize execution, writes, cleanup, or
adoption. Callers must obtain those capabilities from their separate authority
boundaries. This registry does not provide race-free protection against a
filesystem change after validation; callers that later act on a path must
revalidate identity at that action boundary.
