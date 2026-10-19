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
type Options = {
  token?: string;
  fetcher?: typeof fetch;
  onSnapshot: (value: ViewsSnapshot) => void;
  onState: (state: ConnectionState) => void;
  onError: (message: string) => void;
};
export function watchViews(options: Options): () => void {
  const lifetime = new AbortController();
  let request: AbortController | undefined,
    timer: ReturnType<typeof setTimeout> | undefined,
    wake: (() => void) | undefined;
  const fetcher = options.fetcher ?? fetch;
  async function run() {
    let failures = 0;
    while (!lifetime.signal.aborted) {
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
            failures = 0;
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
        options.onError(message);
        if (
          (error instanceof ApiError && [401, 403].includes(error.status)) ||
          failures >= 6
        ) {
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
  const [snapshot, setSnapshot] = useState<ViewsSnapshot | null>(null),
    [state, setState] = useState<ConnectionState>("connecting"),
    [error, setError] = useState(""),
    [retry, setRetry] = useState(0);
  useEffect(() => {
    setSnapshot(null);
    setError("");
    return watchViews({
      token,
      onSnapshot: setSnapshot,
      onState: setState,
      onError: setError,
    });
  }, [token, retry]);
  return { snapshot, state, error, retry: () => setRetry((n) => n + 1) };
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
