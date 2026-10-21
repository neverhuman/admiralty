import { layoutGitGraph } from "./layout";
import type { GraphRequest, GraphResponse } from "./worker-protocol";

// One immutable source generation per request. Termination cancels synchronous
// layout work; no pointer or camera update serializes the graph again.
globalThis.onmessage = (event: MessageEvent<GraphRequest>) => {
  const request = event.data;
  const binding = { id: request.id, accessEpoch: request.accessEpoch, projectId: request.projectId, instanceId: request.instanceId, revision: request.revision };
  let result: GraphResponse;
  try {
    if (request.repo.instance_id !== request.instanceId) throw new Error("Repository instance changed");
    result = { ...binding, ok: true, layout: layoutGitGraph(request.repo, request.options) };
  } catch {
    result = { ...binding, ok: false, error: "Git graph could not be laid out. Source coverage remains unknown." };
  }
  globalThis.postMessage(result);
};
