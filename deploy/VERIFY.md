# Verify what the user is actually running

## Server and component identity

Check the actual listener, not an old watcher record:

```sh
rtk proxy ssh xbabe2 "ss -ltnp 'sport = :18890'"
```

Read the listener PID's executable, start identity and command line. Record the
configured asset directory. Do not copy token values from process configuration
or print credential files.

Fetch the production root and compare its bytes with the staged index. Follow
the entry's imports and verify the lazy feature chunk, CSS and worker files
against the component manifest. An unchanged entry filename is not sufficient
evidence; compare hashes.

For an asset-only release, it is valid to record different runtime and UI source
IDs. It is invalid to claim the executable was rebuilt or the old full manifest
matches the overlay.

## Actual Chrome acceptance

Use the user's existing Chrome tab and real producer repository data. Preserve
drafts and user input. Do not use a synthetic preview, CSS injection, browser
zoom or a saved offline capture as a live fix.

For the requested Git Flow journey:

1. Reload the deployed page and confirm the actual loaded entry, lazy feature
   JS/CSS and graph worker against the served manifest.
2. Select **one repository**, then Expand project. A fleet overview is secondary.
3. Click Fit graph. Measure all four bounds inside the actual graph viewport
   with padding; ensure scroll overflow does not hide the graph.
4. Zoom to 5% and below. Check the camera scale, not only a rounded percentage.
5. Reset to 100% and confirm actual camera scale 1. Pan with a real pointer and
   verify Fit restores the whole graph.
6. Expand a real collapsed commit sequence with the pointer. Focus a present
   collapse control and press Enter to recollapse it; check hit targets and counts.
   At low zoom an individual control may be hidden to preserve commit picking
   and lane clearance. Zoom in to use that control, or use Collapse linear.
7. For the trunk-first release, verify a straight selected-main first-parent
   backbone, topics above it and traceable actual fork/merge junctions. A label
   at the main tip alone does not satisfy this.
8. Inspect a suspicious branch with main still locatable. Real parent edges and
   evidence-backed rebase/squash integration must remain distinct; never invent
   a merge parent to make the drawing look connected.
9. Verify camera/focus restoration and controls after returning to the project.
   Record any source coverage limits.

Save the actual screenshot and numerical observations with timestamp, URL,
source/asset hashes, viewport, zoom, fit bounds, interactions and limitations.
Embed the screenshot when reporting the result.

## Completion message

Use a statement like:

> Deployed UI SOURCE on xbabe2 at TIME. Actual Chrome verified FEATURES. Runtime
> remains RUNTIME_SOURCE because its code was unchanged. Rollback receipt: PATH.
> Remaining scope: LIMITS.

Do not say “everything is deployed” when only a compatible UI component changed.
A 400-commit loaded view does not prove full reachable repository history.
