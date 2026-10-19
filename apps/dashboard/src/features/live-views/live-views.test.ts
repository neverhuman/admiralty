import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { parseRoute, routeHref } from "../../router";
import {
  age,
  isStale,
  parseViews,
  projectPublicLegacyViews,
  watchViews,
  type ViewAgent,
  type ViewsSnapshot,
  type GitRepository,
} from "./feed";
import { fitCamera, layoutAtlas, layoutGit, visible, zoomAt } from "./geometry";

const agent = (id: string, project_id = "project"): ViewAgent => ({
  id,
  name: id,
  role: "worker",
  project_id,
  node: "node",
  presence: "observed",
  state: "unknown",
  model_source: "fixture",
  source: "fixture",
});
const fixture = (): ViewsSnapshot => ({
  schema_version: 1,
  revision: "cut-1",
  simulated: true,
  observed_utc: "2026-10-18T12:00:00Z",
  projects: [{ id: "project", name: "Project" }],
  agents: [agent("worker-b"), agent("worker-c")],
  sources: [],
});
const oid = (n: number) => n.toString(16).padStart(40, "0");
const repository = (): GitRepository => ({
  instance_id: "repo",
  node: "node",
  observed_utc: "2026-10-18T12:00:00Z",
  shallow: false,
  truncated: false,
  refs: [],
  checkouts: [],
  commits: [
    {
      oid: oid(4),
      parents: [oid(2), oid(3)],
      subject: "merge",
      committed_utc: "2000-01-01",
    },
    {
      oid: oid(3),
      parents: [oid(1)],
      subject: "right",
      committed_utc: "2001-01-01",
    },
    {
      oid: oid(1),
      parents: [oid(9)],
      subject: "boundary",
      committed_utc: "2030-01-01",
    },
    {
      oid: oid(2),
      parents: [oid(1)],
      subject: "left",
      committed_utc: "2020-01-01",
    },
  ],
});
const publicLegacy = () => ({
  generated_utc: "2026-10-19T13:14:42Z",
  host: "node-a",
  order: ["project-a"],
  project_catalog: [{ id: "project-a", name: "Project A" }],
  projects: { "project-a": { total: 3, stages: { done: 2, blocked: 1 } } },
  agents: [{ id: "123.worker-a", screen: "worker-a", host: "node-a", project: "project-a", state: "unknown", model: "unknown", model_source: "not observed", observed_utc: "2026-10-19T13:14:12Z" }],
  remote_screens: [{ id: "remote-b", screen: "worker-b", host: "node-b", project: "unassigned", state: "unknown" }],
  git: {},
});

describe("visual truth and deterministic geometry", () => {
  it("projects only explicit public producer fields and preserves source times", () => {
    const source = publicLegacy(), projected = projectPublicLegacyViews(source);
    expect(projected.observed_utc).toBe(source.generated_utc);
    expect(projected.projects).toEqual([{ id: "project-a", name: "Project A", task_count: 3 }]);
    expect(projected.agents).toMatchObject([
      { id: "screen:node-a:123.worker-a", project_id: "project-a", presence: "observed", state: "unknown", observed_utc: "2026-10-19T13:14:12Z" },
      { id: "screen:node-b:worker-b", project_id: null, presence: "unobserved", state: "unknown", observed_utc: source.generated_utc },
    ]);
    expect(projected.agents[0].model).toBeUndefined();
    expect(projected.agents.some((agent) => agent.role === "assignment")).toBe(false);
    expect(projected.sources.find((item) => item.id === "native")?.state).toBe("unavailable");
  });
  it("rejects an invalid public Git payload instead of partially rendering it", () => {
    const source = publicLegacy();
    source.git = { "project-a": { instance_id: "repo", node: "node-a", observed_utc: source.generated_utc, shallow: false, truncated: false, refs: [], checkouts: [], commits: [{ oid: "bad", parents: [], subject: "bad", committed_utc: source.generated_utc }] } };
    expect(() => projectPublicLegacyViews(source)).toThrow("Invalid Git identity");
  });
  it("keeps all merge parents left of children despite skewed dates and unordered input", () => {
    const repo = repository(),
      layout = layoutGit(repo);
    expect(layout.boundaries).toBe(1);
    expect(layout.marks).toHaveLength(4);
    for (const c of repo.commits)
      for (const p of c.parents)
        if (layout.byOid.has(p))
          expect(layout.byOid.get(p)!.x).toBeLessThan(
            layout.byOid.get(c.oid)!.x,
          );
    expect(
      layoutGit({ ...repo, commits: [...repo.commits].reverse() }).marks,
    ).toEqual(layout.marks);
  });
  it("rejects cyclic input instead of dropping ancestry silently", () => {
    const repo = repository();
    repo.commits[2].parents = [oid(4)];
    expect(() => layoutGit(repo)).toThrow("cyclic");
  });
  it("retains slots across state changes, earlier lexical arrivals and departures", () => {
    const slots = new Map(),
      snapshot = fixture();
    const before = layoutAtlas(snapshot, slots);
    snapshot.agents[0].state = "blocked";
    expect(layoutAtlas(snapshot, slots).nodes).toEqual(before.nodes);
    snapshot.agents.unshift(agent("worker-a"));
    const added = layoutAtlas(snapshot, slots);
    for (const n of before.nodes) {
      const after = added.nodes.find((a) => a.agent.id === n.agent.id)!;
      expect({ x: after.x, y: after.y }).toEqual({ x: n.x, y: n.y });
    }
    snapshot.agents = snapshot.agents.filter((a) => a.id !== "worker-b");
    expect(
      layoutAtlas(snapshot, slots).nodes.find((a) => a.agent.id === "worker-c")
        ?.y,
    ).toBe(before.nodes.find((a) => a.agent.id === "worker-c")?.y);
  });
  it("assigns geometry to every agent including offscreen/unassigned identities", () => {
    const snapshot = fixture();
    snapshot.agents = Array.from({ length: 500 }, (_, i) =>
      agent(`worker-${i}`),
    );
    snapshot.agents.push({ ...agent("unassigned"), project_id: null });
    const layout = layoutAtlas(snapshot, new Map());
    expect(layout.nodes).toHaveLength(501);
    expect(layout.boards.some((b) => b.id === "__unassigned")).toBe(true);
    expect(
      layout.nodes.filter((n) => visible(n, { x: 0, y: 0, k: 1 }, 1000, 700))
        .length,
    ).toBeLessThan(501);
  });
  it("zooms around an exact world anchor and fits with padding", () => {
    const camera = { x: 20, y: -10, k: 0.5 },
      point = { x: 200, y: 100 },
      next = zoomAt(camera, point, 2);
    expect((point.x - camera.x) / camera.k).toBe((point.x - next.x) / next.k);
    expect((point.y - camera.y) / camera.k).toBe((point.y - next.y) / next.k);
    const fit = fitCamera(
      { x: 0, y: 0, width: 2000, height: 1000 },
      { width: 1000, height: 600 },
    );
    expect(fit.x).toBeGreaterThanOrEqual(32);
    expect(fit.y).toBeGreaterThanOrEqual(32);
  });
  it("round-trips opaque selections and safe route aliases", () => {
    const href = routeHref("atlas", {
        focus: "screen:a:12.agent /?#",
        project: "project",
      }),
      route = parseRoute(href);
    expect(route.name).toBe("atlas");
    expect(route.params.get("focus")).toBe("screen:a:12.agent /?#");
    expect(parseRoute("#/fleet-map").name).toBe("atlas");
    expect(parseRoute("#/unknown").name).toBe("detail");
  });
  it("validates complete frames, unique identities, real OIDs and explicit unknowns", () => {
    expect(parseViews(fixture()).agents[0].model).toBeUndefined();
    expect(() => parseViews({ ...fixture(), schema_version: 2 })).toThrow();
    expect(() =>
      parseViews({ ...fixture(), agents: [agent("same"), agent("same")] }),
    ).toThrow("Duplicate");
    const value = fixture();
    value.projects[0].git = repository();
    value.projects[0].git.commits[0].oid = "fake";
    expect(() => parseViews(value)).toThrow("Git identity");
  });
  it("distinguishes stale, unknown and future source clocks", () => {
    const now = Date.parse("2026-10-18T12:10:00Z");
    expect(isStale("2026-10-18T12:00:00Z", now)).toBe(true);
    expect(age(null, now)).toBe("time unknown");
    expect(age("2026-10-18T13:00:00Z", now)).toBe("source clock ahead");
  });
});

it("rejects non-JSON accessors before reading visual version metadata", () => {
  let accessed = false;
  const value = Object.defineProperty({}, "schema_version", {
    enumerable: true,
    get() {
      accessed = true;
      return 1;
    },
  });
  expect(() => parseViews(value)).toThrow("accessors");
  expect(accessed).toBe(false);
});

describe("atomic authenticated visual feed", () => {
  const stops: Array<() => void> = [];
  beforeEach(() => vi.useFakeTimers());
  afterEach(async () => {
    stops.splice(0).forEach((stop) => stop());
    await vi.advanceTimersByTimeAsync(0);
    vi.useRealTimers();
  });
  function service() {
    const channels: ReadableStreamDefaultController<Uint8Array>[] = [];
    const fetcher = vi.fn<typeof fetch>(
      async (_url, init) =>
        new Response(
          new ReadableStream<Uint8Array>({
            start(c) {
              channels.push(c);
              init?.signal?.addEventListener(
                "abort",
                () => c.error(new DOMException("Aborted", "AbortError")),
                { once: true },
              );
            },
          }),
          { headers: { "Content-Type": "text/event-stream" } },
        ),
    );
    const onSnapshot = vi.fn(),
      onState = vi.fn(),
      onError = vi.fn();
    stops.push(
      watchViews({
        token: "private-secret",
        fetcher,
        onSnapshot,
        onState,
        onError,
      }),
    );
    const send = (value: unknown, id = "cut-1") =>
      channels
        .at(-1)!
        .enqueue(
          new TextEncoder().encode(
            `event: views\nid: ${id}\ndata: ${JSON.stringify(value)}\n\n`,
          ),
        );
    return { channels, fetcher, onSnapshot, onState, onError, send };
  }
  it("uses one stream, makes no idle polls and never puts token in URL", async () => {
    const server = service();
    await vi.advanceTimersByTimeAsync(0);
    server.send(fixture());
    await vi.advanceTimersByTimeAsync(0);
    for (let i = 0; i < 4; i++) {
      server.channels[0].enqueue(new TextEncoder().encode(": heartbeat\n\n"));
      await vi.advanceTimersByTimeAsync(14_000);
    }
    expect(server.fetcher).toHaveBeenCalledTimes(1);
    expect(server.fetcher.mock.calls[0][0]).toBe("/v2/views/events");
    expect(
      new Headers(server.fetcher.mock.calls[0][1]?.headers).get(
        "Authorization",
      ),
    ).toBe("Bearer private-secret");
    expect(server.onState).toHaveBeenLastCalledWith("connected");
  });
  it("rejects malformed frames then reconnects with complete replacement, without stale memberships", async () => {
    const server = service();
    await vi.advanceTimersByTimeAsync(0);
    server.send(fixture());
    await vi.advanceTimersByTimeAsync(0);
    server.send({ ...fixture(), agents: null });
    await vi.advanceTimersByTimeAsync(500);
    expect(server.onSnapshot).toHaveBeenCalledTimes(1);
    expect(server.fetcher).toHaveBeenCalledTimes(2);
    const next = {
      ...fixture(),
      revision: "cut-2",
      agents: [agent("new-worker")],
    };
    server.send(next, "cut-2");
    await vi.advanceTimersByTimeAsync(0);
    expect(server.onSnapshot).toHaveBeenLastCalledWith(next);
  });
  it("checks the public route after auth failure, then pauses and cancels callbacks", async () => {
    const onState = vi.fn(),
      onSnapshot = vi.fn(),
      fetcher = vi
        .fn<typeof fetch>()
        .mockResolvedValue(new Response("", { status: 401 }));
    const stop = watchViews({ fetcher, onState, onSnapshot, onError: vi.fn() });
    stops.push(stop);
    await vi.advanceTimersByTimeAsync(0);
    expect(fetcher).toHaveBeenCalledTimes(2);
    expect(fetcher.mock.calls[0][0]).toBe("/v2/views/events");
    expect(fetcher.mock.calls[1][0]).toBe("/v1/legacy");
    expect(new Headers(fetcher.mock.calls[1][1]?.headers).has("Authorization")).toBe(false);
    expect(onState).toHaveBeenLastCalledWith("paused");
    stop();
    expect(onSnapshot).not.toHaveBeenCalled();
  });
  it("uses only public legacy reads after private auth denial and accepts a refreshed token", async () => {
    const publicSnapshot = publicLegacy(), callbacks = {
      onSnapshot: vi.fn(), onState: vi.fn(), onError: vi.fn(),
      onSessionAccessRequired: vi.fn(), onPublicFallback: vi.fn(),
    };
    const publicFetcher = vi.fn<typeof fetch>(async (input) =>
      String(input) === "/v2/views/events"
        ? new Response("", { status: 401 })
        : new Response(JSON.stringify(publicSnapshot), { headers: { "Content-Type": "application/json" } }),
    );
    const stopPublic = watchViews({ token: "expired-token", fetcher: publicFetcher, ...callbacks });
    stops.push(stopPublic);
    await vi.advanceTimersByTimeAsync(0);
    expect(publicFetcher).toHaveBeenCalledTimes(2);
    expect(new Headers(publicFetcher.mock.calls[0][1]?.headers).get("Authorization")).toBe("Bearer expired-token");
    expect(new Headers(publicFetcher.mock.calls[1][1]?.headers).has("Authorization")).toBe(false);
    expect(callbacks.onSnapshot).toHaveBeenCalledWith(expect.objectContaining({
      observed_utc: publicSnapshot.generated_utc,
      sources: expect.arrayContaining([expect.objectContaining({ id: "native", state: "unavailable" })]),
    }));
    expect(callbacks.onSessionAccessRequired).toHaveBeenLastCalledWith(true);
    expect(callbacks.onPublicFallback).toHaveBeenLastCalledWith(true);

    const channels: ReadableStreamDefaultController<Uint8Array>[] = [];
    const privateFetcher = vi.fn<typeof fetch>(async () => new Response(
      new ReadableStream<Uint8Array>({ start(controller) { channels.push(controller); } }),
      { headers: { "Content-Type": "text/event-stream" } },
    ));
    const stopPrivate = watchViews({ token: "refreshed-token", fetcher: privateFetcher, ...callbacks });
    stops.push(stopPrivate);
    await vi.advanceTimersByTimeAsync(0);
    expect(new Headers(privateFetcher.mock.calls[0][1]?.headers).get("Authorization")).toBe("Bearer refreshed-token");
    channels[0].enqueue(new TextEncoder().encode(`event: views\nid: cut-1\ndata: ${JSON.stringify(fixture())}\n\n`));
    await vi.advanceTimersByTimeAsync(0);
    expect(callbacks.onSnapshot).toHaveBeenLastCalledWith(fixture());
    expect(callbacks.onSessionAccessRequired).toHaveBeenLastCalledWith(false);
    expect(callbacks.onPublicFallback).toHaveBeenLastCalledWith(false);
    stopPublic();
    stopPrivate();
  });
  it("aborts an old public fallback and ignores its late response after access changes", async () => {
    let finishLegacy!: (response: Response) => void;
    let legacySignal: AbortSignal | undefined;
    const onSnapshot = vi.fn(), onState = vi.fn(), onError = vi.fn();
    const oldFetcher = vi.fn<typeof fetch>((input, init) => {
      if (String(input) === "/v2/views/events") return Promise.resolve(new Response("", { status: 401 }));
      legacySignal = init?.signal ?? undefined;
      return new Promise((resolve) => { finishLegacy = resolve; });
    });
    const stopOld = watchViews({ token: "old-token", fetcher: oldFetcher, onSnapshot, onState, onError });
    await vi.advanceTimersByTimeAsync(0);
    expect(oldFetcher).toHaveBeenCalledTimes(2);
    stopOld();
    expect(legacySignal?.aborted).toBe(true);
    finishLegacy(new Response(JSON.stringify(publicLegacy()), { headers: { "Content-Type": "application/json" } }));
    await vi.advanceTimersByTimeAsync(0);
    expect(onSnapshot).not.toHaveBeenCalled();
  });
  it("bounds reconnect storms even when each short connection sends a valid snapshot", async () => {
    const server = service();
    await vi.advanceTimersByTimeAsync(0);
    for (let attempt = 0; attempt < 7; attempt++) {
      server.send(fixture());
      await vi.advanceTimersByTimeAsync(0);
      server.channels.at(-1)!.close();
      await vi.advanceTimersByTimeAsync(0);
      if (attempt < 6)
        await vi.advanceTimersByTimeAsync(Math.min(30_000, 500 * 2 ** attempt));
    }
    expect(server.fetcher).toHaveBeenCalledTimes(7);
    expect(server.onState).toHaveBeenLastCalledWith("paused");
    await vi.advanceTimersByTimeAsync(60_000);
    expect(server.fetcher).toHaveBeenCalledTimes(7);
  });
});
