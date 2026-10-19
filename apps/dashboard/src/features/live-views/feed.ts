import { useEffect, useState } from "react";
import { validateViewsSnapshot } from "../../../../../packages/contracts/index";
import type { ViewsSnapshot } from "../../../../../packages/contracts/generated/views-snapshot";
import { ApiError } from "../../api";
import {
  createSseParser,
  retryDelayMs,
  type ConnectionState,
} from "../../events";
import { layoutGit } from "./geometry";
import { API_VALIDATION_LIMITS, assertBoundedJson } from "../../validation";
import { data, getLegacy, list, parseLegacy, text } from "../../legacy";

export type {
  ViewsSnapshot,
  ViewAgent,
  ViewProject,
  GitRepository,
  GitCommit,
  GitCheckout,
} from "../../../../../packages/contracts/generated/views-snapshot";
export function parseViews(value: unknown): ViewsSnapshot {
  assertBoundedJson(value, {
    ...API_VALIDATION_LIMITS,
    nodes: 500_000,
    collection: 20_000,
  });
  if (
    !value ||
    typeof value !== "object" ||
    (value as { schema_version?: unknown }).schema_version !== 1
  )
    throw new Error("Unsupported visual snapshot version");
  if (!validateViewsSnapshot(value))
    throw new Error("Unsupported visual snapshot");
  if (
    new Set(value.agents.map((a) => a.id)).size !== value.agents.length ||
    new Set(value.projects.map((p) => p.id)).size !== value.projects.length
  )
    throw new Error("Duplicate visual identity");
  for (const p of value.projects)
    if (p.git) {
      const validOid = (s: string) =>
        /^(?:[a-f0-9]{40}|[a-f0-9]{64})$/i.test(s);
      if (
        new Set(p.git.commits.map((c) => c.oid)).size !==
          p.git.commits.length ||
        p.git.commits.some(
          (c) => !validOid(c.oid) || c.parents.some((p) => !validOid(p)),
        )
      )
        throw new Error("Invalid Git identity");
      if (
        p.git.refs.some(
          (r) => !validOid(r.oid) || (r.commit_oid && !validOid(r.commit_oid)),
        ) ||
        p.git.checkouts.some((c) => c.head && !validOid(c.head))
      )
        throw new Error("Invalid Git attachment");
      layoutGit(p.git);
    }
  return value;
}

/** Bounded rendering of fields exposed by the explicitly public legacy route. */
export function projectPublicLegacyViews(value: unknown): ViewsSnapshot {
  const source = parseLegacy(value), generated = source.generated_utc;
  const host = text(source.host) || "unknown-node";
  const names = new Map<string, string>();
  for (const project of list(source.project_catalog).map(data)) {
    const id = text(project.id);
    if (id) names.set(id, text(project.name) || id);
  }
  for (const [id, value] of Object.entries(source.projects))
    names.set(id, text(data(value).name) || names.get(id) || id);
  const ids = [...new Set([...(source.order ?? []), ...names.keys()])].filter((id) => names.has(id));
  const git = data(source.git);
  const projects: ViewsSnapshot["projects"] = ids.map((id) => {
    const project: ViewsSnapshot["projects"][number] = { id, name: names.get(id) || id };
    const repo = git[id];
    if (repo && typeof repo === "object" && !Array.isArray(repo))
      project.git = repo as NonNullable<ViewsSnapshot["projects"][number]["git"]>;
    const count = data(source.projects[id]).total;
    if (typeof count === "number" && Number.isSafeInteger(count) && count >= 0)
      project.task_count = count;
    return project;
  });
  const agents = new Map<string, ViewsSnapshot["agents"][number]>();
  const add = (items: unknown, observed: boolean) => {
    for (const raw of list(items).map(data)) {
      const identity = text(raw.id) || text(raw.screen) || text(raw.name);
      if (!identity) continue;
      const name = text(raw.screen) || text(raw.name) || identity;
      const node = text(raw.host) || text(raw.node) || text(raw.node_id) || host;
      const full = /^\d+\..+/.test(identity) ? identity : name;
      const id = `screen:${node}:${full}`;
      const state = text(raw.state) || text(raw.status) ||
        (raw.agent_running === true ? "active" : raw.alive === true ? "screen alive; agent unknown" : "unknown");
      const ended = ["finished", "exited", "gone", "dead", "cancelled"].includes(state);
      const model = text(raw.model);
      const project = text(raw.project);
      agents.set(id, {
        id,
        name,
        role: "worker",
        project_id: project && project !== "unassigned" ? project : null,
        node,
        presence: ended ? "ended" : observed || raw.agent_running === true ? "observed" : "unobserved",
        state,
        ...(model && !["unknown", "—", "not observed"].includes(model.toLowerCase()) ? { model } : {}),
        ...(text(raw.requested_model) || text(raw.configured_model)
          ? { requested_model: text(raw.requested_model) || text(raw.configured_model) }
          : {}),
        ...(text(raw.model_observed_utc) ? { model_observed_utc: text(raw.model_observed_utc) } : {}),
        model_source: text(raw.model_source) || "Producer report; provider confirmation unavailable",
        ...(text(raw.effort) ? { effort: text(raw.effort) } : {}),
        ...(text(raw.task) || text(raw.packet_id)
          ? { task: text(raw.task) || text(raw.packet_id) }
          : {}),
        ...(text(raw.branch) ? { branch: text(raw.branch) } : {}),
        observed_utc: text(raw.observed_utc) || text(raw.source_updated_utc) || generated,
        source: "Public producer observation",
      });
    }
  };
  add(source.agents, true);
  add(source.remote_screens, false);
  return parseViews({
    schema_version: 1,
    revision: `public-${generated}`,
    simulated: false,
    observed_utc: generated,
    projects,
    agents: [...agents.values()],
    sources: [
      { id: "producer", label: "Public producer snapshot", observed_utc: generated, state: "observed", coverage: "Read-only producer fields from the explicitly public legacy endpoint; source timestamps are preserved." },
      { id: "native", label: "Private native assignments", state: "unavailable", coverage: "Authenticated native task assignments are unavailable in this public-only projection." },
    ],
  });
}
type Options = {
  token?: string;
  fetcher?: typeof fetch;
  onSnapshot: (value: ViewsSnapshot) => void;
  onState: (state: ConnectionState) => void;
  onError: (message: string) => void;
  onSessionAccessRequired?: (required: boolean) => void;
  onPublicFallback?: (active: boolean) => void;
};
export function watchViews(options: Options): () => void {
  const lifetime = new AbortController();
  let request: AbortController | undefined,
    timer: ReturnType<typeof setTimeout> | undefined,
    wake: (() => void) | undefined;
  const fetcher = options.fetcher ?? fetch;
  async function waitForFallback(): Promise<void> {
    await new Promise<void>((resolve) => {
      const finish = () => {
        clearTimeout(timer);
        lifetime.signal.removeEventListener("abort", finish);
        wake = undefined;
        resolve();
      };
      wake = finish;
      timer = setTimeout(finish, 15_000);
      lifetime.signal.addEventListener("abort", finish, { once: true });
    });
  }
  async function watchPublicLegacy(): Promise<void> {
    while (!lifetime.signal.aborted) {
      request = new AbortController();
      const abort = () => request?.abort();
      lifetime.signal.addEventListener("abort", abort, { once: true });
      timer = setTimeout(abort, 15_000);
      try {
        const legacy = await getLegacy(undefined, request.signal, fetcher);
        clearTimeout(timer);
        if (!legacy) throw new Error("Public producer snapshot is not configured");
        if (lifetime.signal.aborted) return;
        options.onSnapshot(projectPublicLegacyViews(legacy));
        options.onPublicFallback?.(true);
        options.onError("");
        options.onState("paused");
      } catch (error) {
        clearTimeout(timer);
        if (lifetime.signal.aborted) return;
        options.onError(error instanceof Error ? error.message : "Public producer snapshot unavailable");
        options.onState("paused");
        return;
      } finally {
        lifetime.signal.removeEventListener("abort", abort);
      }
      await waitForFallback();
    }
  }
  async function run() {
    let failures = 0;
    while (!lifetime.signal.aborted) {
      let opened = 0;
      try {
        options.onState(failures ? "reconnecting" : "connecting");
        request = new AbortController();
        timer = setTimeout(() => request?.abort(), 15_000);
        const response = await fetcher("/v2/views/events", {
          signal: request.signal,
          cache: "no-store",
          headers: {
            Accept: "text/event-stream",
            ...(options.token
              ? { Authorization: `Bearer ${options.token}` }
              : {}),
          },
        });
        clearTimeout(timer);
        if (!response.ok)
          throw new ApiError(
            `Visual feed failed (${response.status})`,
            response.status,
          );
        if (
          !response.body ||
          !response.headers.get("content-type")?.includes("text/event-stream")
        )
          throw new Error("Unsupported visual feed");
        options.onSessionAccessRequired?.(false);
        options.onPublicFallback?.(false);
        opened = Date.now();
        const parser = createSseParser(
          (frame) => {
            if (frame.event === "unavailable")
              throw new Error(
                "Visual projection unavailable; last observations retained",
              );
            if (frame.event !== "views") return;
            const snapshot = parseViews(JSON.parse(frame.data));
            if (frame.id !== snapshot.revision)
              throw new Error("Visual revision mismatch");
            if (lifetime.signal.aborted) return;
            options.onSnapshot(snapshot);
            options.onState("connected");
            options.onError("");
          },
          8 * 1024 * 1024,
        );
        const reader = response.body.getReader(),
          decoder = new TextDecoder("utf-8", { fatal: true });
        try {
          while (!lifetime.signal.aborted) {
            timer = setTimeout(() => request?.abort(), 45_000);
            const chunk = await reader.read();
            clearTimeout(timer);
            if (chunk.done) throw new Error("Visual feed disconnected");
            parser.push(decoder.decode(chunk.value, { stream: true }));
          }
        } finally {
          await reader.cancel().catch(() => undefined);
          reader.releaseLock();
        }
      } catch (error) {
        clearTimeout(timer);
        request?.abort();
        if (lifetime.signal.aborted) return;
        const message =
          error instanceof Error ? error.message : "Visual feed unavailable";
        if (opened && Date.now() - opened >= 30_000) failures = 0;
        options.onError(message);
        if (error instanceof ApiError && [401, 403].includes(error.status)) {
          options.onSessionAccessRequired?.(true);
          options.onPublicFallback?.(false);
          await watchPublicLegacy();
          return;
        }
        if (failures >= 6) {
          options.onState("paused");
          return;
        }
        options.onState("reconnecting");
        await new Promise<void>((resolve) => {
          wake = resolve;
          timer = setTimeout(resolve, retryDelayMs(failures++));
        });
        wake = undefined;
      }
    }
  }
  void run();
  return () => {
    lifetime.abort();
    request?.abort();
    clearTimeout(timer);
    wake?.();
  };
}
export function useViews(token?: string) {
  const [snapshotState, setSnapshotState] = useState<{
    token?: string;
    snapshot: ViewsSnapshot;
  } | null>(null),
    [state, setState] = useState<ConnectionState>("connecting"),
    [sessionAccessRequired, setSessionAccessRequired] = useState(false),
    [publicFallback, setPublicFallback] = useState(false),
    [error, setError] = useState(""),
    [retry, setRetry] = useState(0);
  useEffect(() => {
    setSnapshotState(null);
    setError("");
    setSessionAccessRequired(false);
    setPublicFallback(false);
    return watchViews({
      token,
      onSnapshot: (snapshot) => setSnapshotState({ token, snapshot }),
      onState: setState,
      onError: setError,
      onSessionAccessRequired: setSessionAccessRequired,
      onPublicFallback: setPublicFallback,
    });
  }, [token, retry]);
  const snapshot = snapshotState && snapshotState.token === token ? snapshotState.snapshot : null;
  return { snapshot, state, error, sessionAccessRequired, publicFallback, retry: () => setRetry((n) => n + 1) };
}
export function age(
  value: string | null | undefined,
  now = Date.now(),
): string {
  if (!value) return "time unknown";
  const seconds = (now - Date.parse(value)) / 1000;
  if (!Number.isFinite(seconds)) return "time unknown";
  if (seconds < -30) return "source clock ahead";
  if (seconds > 180)
    return `stale · ${seconds < 3600 ? `${Math.floor(seconds / 60)}m` : seconds < 86400 ? `${Math.floor(seconds / 3600)}h` : `${Math.floor(seconds / 86400)}d`} old`;
  return `${Math.max(0, Math.floor(seconds))}s old`;
}
export function isStale(value?: string | null, now = Date.now()) {
  return (
    !!value &&
    (!Number.isFinite(Date.parse(value)) || now - Date.parse(value) > 180_000)
  );
}
