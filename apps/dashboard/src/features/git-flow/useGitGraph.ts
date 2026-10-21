import { useEffect, useRef, useState } from "react";
import type { ViewProject } from "../live-views/feed";
import type { GitGraphLayout, GraphLayoutOptions } from "./model";
import { sameGraphRequest, type GraphRequest, type GraphResponse } from "./worker-protocol";

type Scene = { key: string; project: ViewProject; layout: GitGraphLayout; revision: string };
/** Keep a coherent previous source cut visible while rebuilding this instance. */
export function useGitGraph(project: ViewProject, revision: string, accessEpoch: number, options: GraphLayoutOptions) {
  const key = JSON.stringify([accessEpoch, project.id, project.git?.instance_id]);
  const [scene, setScene] = useState<Scene | null>(null);
  const [error, setError] = useState("");
  const generation = useRef(0), prior = useRef<Scene | null>(null);
  const [done, setDone] = useState<GraphLayoutOptions | null>(null);
  useEffect(() => {
    const id = ++generation.current;
    const repo = project.git;
    setError("");
    if (!repo) { prior.current = null; setScene(null); return; }
    const worker = new Worker(new URL("./gitGraph.worker.ts", import.meta.url), { type: "module" });
    const request: GraphRequest = { id, accessEpoch, projectId: project.id, instanceId: repo.instance_id, revision, repo,
      options: { ...options, previousPaths: prior.current?.key === key ? prior.current.layout.paths : undefined } };
    worker.onmessage = (event: MessageEvent<GraphResponse>) => {
      if (generation.current !== id || !sameGraphRequest(request, event.data)) return;
      if (event.data.ok) {
        const next = { key, project, revision, layout: event.data.layout };
        prior.current = next; setScene(next); setDone(options);
      } else setError(event.data.error);
    };
    worker.onerror = () => { if (generation.current === id) setError("Git layout worker unavailable. Retry by reopening the project."); };
    worker.postMessage(request);
    return () => { generation.current++; worker.onmessage = null; worker.onerror = null; worker.terminate(); };
  }, [key, project, revision, accessEpoch, options]);
  const admitted = scene?.key === key ? scene : null;
  return { scene: admitted, error, pending: !admitted || admitted.revision !== revision || done !== options };
}
