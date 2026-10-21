# Publish a compatible dashboard without rebuilding Rust

This is the default for a frontend-only change.

## 1. Prove the source and compatibility

Use the owned source checkout. Record exact UI source, runtime source, tree,
actual review, PR checks, receiving-main checks and source cleanliness.

Run this comparison after substituting the two recorded commit IDs:

```sh
rtk proxy git diff --exit-code RUNTIME_SOURCE UI_SOURCE -- crates Cargo.toml Cargo.lock schemas packages/contracts
```

Any difference requires an explicit compatibility assessment or the full-release
path. An unchanged contract tree alone does not prove a new frontend request is
supported: review its API use and test against the deployed runtime.

Do not edit the original foreign/dirty checkout, switch its branch or copy its
working files. Export the frozen Git object into a new owned build directory.

## 2. Build an isolated, reproducible bundle

Use repository-pinned Node 24 and pnpm 11.15.1. Work in a fresh owned export, not
another agent's active checkout.

```sh
rtk proxy node --version
rtk proxy pnpm --version
rtk proxy pnpm install --frozen-lockfile --ignore-scripts
rtk proxy env NODE_ENV=production pnpm --dir apps/dashboard build
```

Retain the command, versions, source/tree, exit and raw log. Do not force a pnpm
module purge against shared or foreign dependencies. The package's build is
TypeScript compilation followed by Vite's production build.

Package the **complete** `apps/dashboard/dist/` directory, not only index.html.
Include every lazy feature chunk, CSS, image and module worker. For each regular
file record relative path, byte count and SHA-256. Refuse symlinks, path traversal,
missing files and name/hash collisions. Record the manifest's own hash.

## 3. Stage on xbabe2

Create a new owned, private staging directory. Copy the archive and manifest.
Verify the archive hash, extract only into that staging directory, then read
every extracted file back and compare its size/hash. This is staging, not live.

The operator must discover the actual daemon and its `--assets` path. Do not
guess from the newest directory, a Git branch or an old watcher status file.

The existing locks are:

- `/home/ubuntu/admiralty-v2-build/private-config/.sc156-deploy-watch.lock`
- `/home/ubuntu/admiralty-v2-build/private-config/.sc156-primary-deploy.lock`

Acquire **both exclusively and nonblocking**, in that order. Verify their real
identity and ownership. If either is held, leave production unchanged and
coordinate with that writer. Hold both through the switch and immediate HTTP
verification.

## 4. Publish one atomic entry change

Before mutation, verify the daemon UID/start/executable, current authority,
current served entry hash, manifest and available space. Honor an actual operator
STOP; do not clear it silently.

1. Save the original index bytes in the private staging directory and verify the
   backup hash. Inventory the existing asset hashes.
2. Account for every manifest file. For each content-hashed file, keep an
   identical existing file; otherwise create the new file exclusively and fsync
   it. An existing same-name file with different bytes stops publication. Verify
   every nonhashed file other than index.html is already identical; a missing or
   changed nonhashed file stops this procedure and requires reconciliation before
   publication. Never silently skip a manifest member.
3. Keep every old hashed asset so already-open clients continue to work.
4. Write the new index to a temporary regular file **beside** the production
   index, fsync it, then atomically rename it over index.html. Fsync the directory.
5. Verify the root response and every manifest asset over production HTTP.
   Roll the entry back immediately if these checks fail.
6. Record the original entry hash, new entry hash, added assets, both source
   revisions, process identity, lock ownership and rollback location.

Do not restart the daemon, observer or database for this path. Do not claim the
old complete-release manifest still describes the modified UI: preserve it and
add an explicit component-overlay receipt.

## 5. Finish in the actual browser

Follow [VERIFY.md](VERIFY.md). Reload the same user tab, verify the loaded lazy
JS/CSS/worker and exercise the feature. Keep the rollback backup until the
replacement is accepted. Release only the locks owned by this transaction.

If an outcome is ambiguous, inspect the entry and receipt before any further
effect. Do not rerun an asset switch blindly.
