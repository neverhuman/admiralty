import { useMemo, useRef, useState, type ReactNode } from "react";
import type { ViewProject } from "../live-views/feed";
import type { Selection } from "../live-views/LiveViews";
import type { CommitRange } from "./model";
import type { GitHealthReport } from "../live-views/gitHealth";
import type { CameraMemory } from "./camera";
import GraphCanvas from "./GraphCanvas";
import ProjectViewport from "./ProjectViewport";
import { useGitGraph } from "./useGitGraph";
import { loadedAncestry } from "./scene";
import { createGitMergeIndex } from "./merges";

export default function ProjectInvestigation({ project, revision, accessEpoch, memory, expanded, collapsed, selection, focusId, query, report, onSelect, onToggleRange, dock }: {
  project: ViewProject; revision: string; accessEpoch: number; memory: CameraMemory;
  expanded: ReadonlySet<string>; collapsed: ReadonlySet<string>; selection: Selection | null;
  focusId: string; query: string; report?: GitHealthReport;
  onSelect: (selection: Selection) => void; onToggleRange: (range: CommitRange) => void; dock?: ReactNode;
}) {
  const [explicitRef, setExplicitRef] = useState("");
  const [branchRef, setBranchRef] = useState("");
  const [isolate, setIsolate] = useState(false);
  const [hiddenRefs, setHiddenRefs] = useState(0);
  const backboneOptions = useRef<HTMLDetailsElement>(null);
  const selectedOid = selection?.kind === "commit" && selection.project.id === project.id ? selection.commit.oid : selection?.kind === "checkout" && selection.project.id === project.id ? selection.checkout.head ?? "" : "";
  const options = useMemo(() => {
    const needle = query.trim().toLowerCase(), expandedOids = new Set<string>(), collapsedOids = new Set<string>();
    for (const commit of project.git?.commits ?? []) {
      const key = `${project.id}:${commit.oid}`;
      if (collapsed.has(key)) collapsedOids.add(commit.oid);
      if (expanded.has(key) || (!collapsed.has(key) && (commit.oid === focusId || commit.oid === selectedOid || (needle && `${commit.oid} ${commit.subject}`.toLowerCase().includes(needle))))) expandedOids.add(commit.oid);
    }
    // Independent health requests cannot silently move this graph's display
    // trunk. Only a matching instance AND observation cut is compatible.
    const compatible = report && report.instance_id === project.git?.instance_id && report.observed_utc === project.git.observed_utc && report.integration_ref;
    return { expandedOids, collapsedOids, explicitRef: explicitRef || undefined,
      integration: compatible ? { instanceId: report.instance_id, refName: report.integration_ref!, oid: report.integration_oid ?? null } : undefined };
  }, [project, expanded, collapsed, selectedOid, focusId, query, explicitRef, report]);
  const { scene, pending, error } = useGitGraph(project, revision, accessEpoch, options);
  const commits = useMemo(() => new Map(scene?.project.git?.commits.map((commit) => [commit.oid, commit]) ?? []), [scene]);
  const branch = scene?.project.git?.refs.find((ref) => ref.name === branchRef);
  const backboneMerges = useMemo(() => scene?.project.git ? [...createGitMergeIndex(scene.project.git, scene.layout.backboneOids).values()].filter((merge) => merge.onBackbone) : [], [scene]);
  const context = useMemo(() => isolate && scene && branch?.commit_oid ? loadedAncestry(scene.layout, branch.commit_oid) : undefined, [scene, isolate, branch?.commit_oid]);
  const status = error || (pending ? "Updating loaded graph…" : scene ? `${scene.layout.orderedCommits.length} loaded commits · ${scene.layout.backbone.source === "display-default" && scene.layout.backbone.kind === "resolved" ? "Display default: local main" : scene.layout.backbone.reason}` : "Preparing loaded graph…");
  const refStatus = scene && hiddenRefs > 0 ? ` · ${hiddenRefs} refs without labels; use Inspect branch or ref` : "";
  return <>
    <div className="git-investigation-heading">
      <details className="git-display-options" ref={backboneOptions}><summary>Backbone{backboneMerges.length ? ` · ${backboneMerges.length} merge junction${backboneMerges.length === 1 ? "" : "s"}` : " options"}</summary><label>Display backbone <select aria-label="Display backbone" value={explicitRef} onChange={(event) => setExplicitRef(event.target.value)}>
        <option value="">Automatic · local main convention</option>
        {explicitRef && !project.git?.refs.some((ref) => ref.name === explicitRef) && <option value={explicitRef}>{explicitRef} · no longer observed</option>}
        {project.git?.refs.map((ref) => <option key={ref.name} value={ref.name}>{ref.name}</option>)}
      </select></label><div className="git-merge-junctions">
        <strong>Actual merge junctions on this backbone</strong>
        {backboneMerges.map((merge) => <button className="lv-text-button" key={merge.commitOid} onClick={() => {
          const commit = commits.get(merge.commitOid);
          if (scene && commit) { setIsolate(false); onSelect({ kind: "commit", project: scene.project, commit }); if (backboneOptions.current) backboneOptions.current.open = false; }
        }}><code>{merge.commitOid.slice(0, 10)}</code> · {merge.parentCount} parents · {commits.get(merge.commitOid)?.subject}</button>)}
        {!backboneMerges.length && <p>No multi-parent junction in the loaded backbone. Rebase, squash and fast-forward integration need separate evidence.</p>}
      </div></details>
      <label>Inspect branch or ref <select aria-label="Inspect branch or ref" value={branchRef} onChange={(event) => {
        setBranchRef(event.target.value);
        const oid = scene?.project.git?.refs.find((ref) => ref.name === event.target.value)?.commit_oid;
        const commit = oid && commits.get(oid);
        if (scene && commit) onSelect({ kind: "commit", project: scene.project, commit });
      }}><option value="">Choose a ref…</option>{scene?.project.git?.refs.map((ref) => <option key={ref.name} value={ref.name}>{ref.name}{ref.commit_oid && commits.has(ref.commit_oid) ? "" : " · tip not loaded"}</option>)}</select></label>
      <button className="lv-button" disabled={!branch?.commit_oid || !commits.has(branch.commit_oid)} aria-pressed={isolate} onClick={() => setIsolate(!isolate)}>{isolate ? "Show full context" : "Isolate branch"}</button>
      <p className="git-graph-status" role="status" title={scene?.layout.backbone.reason}>{status + refStatus}</p>
    </div>
    {scene ? <ProjectViewport key={scene.key} project={scene.project} layout={scene.layout} accessEpoch={accessEpoch} memory={memory} selectionOid={selectedOid} dock={dock}
      render={(camera, size, controller) => <GraphCanvas project={scene.project} layout={scene.layout} zoom={camera.k} left={-camera.x} top={-camera.y} width={size.width} height={size.height}
        selection={selection} onSelect={onSelect} onToggleRange={(range) => controller.preserveAnchor(range.commits[0].oid, () => onToggleRange(range))} visibleOids={context} safeInset={controller.padding} onHiddenRefs={setHiddenRefs} fixed />}
    /> : <div className="git-project-preparing" role="status">{error || "Preparing repository graph…"}</div>}
    {branch && <details className="git-branch-evidence"><summary><code>{branch.name}</code> · {context ? `${context.size} loaded commits in ancestry context` : "full loaded context"}</summary><p>Tip <code>{branch.commit_oid ?? "not a loaded commit"}</code>. Integration status requires separate source-bound evidence.</p></details>}
  </>;
}
