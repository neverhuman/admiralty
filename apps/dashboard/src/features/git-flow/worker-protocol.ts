import type { GitGraphLayout, GitRepository, GraphLayoutOptions } from "./model";

export type GraphRequest = { id: number; accessEpoch: number; projectId: string; instanceId: string; revision: string; repo: GitRepository; options: GraphLayoutOptions };
export type GraphResponse = Pick<GraphRequest, "id" | "accessEpoch" | "projectId" | "instanceId" | "revision"> & ({ ok: true; layout: GitGraphLayout } | { ok: false; error: string });
export function sameGraphRequest(request: GraphRequest, response: GraphResponse): boolean {
  return request.id === response.id && request.accessEpoch === response.accessEpoch && request.projectId === response.projectId && request.instanceId === response.instanceId && request.revision === response.revision;
}
