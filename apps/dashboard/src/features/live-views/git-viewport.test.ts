import { describe, expect, it } from "vitest";
import type { GitRepository } from "./feed";
import { layoutGit, type GitLayout } from "./geometry";
import { captureGitAnchor, fitGitOverview, fitGitZoom, gitFrontierScroll, restoreGitAnchor, type GitViewportBand } from "./git-viewport";

const oid = (n: number) => n.toString(16).padStart(40, "0");
function repository(count = 24): GitRepository {
  return {
    instance_id: "viewport-fixture", node: "test", observed_utc: "2026-10-20T12:00:00Z",
    shallow: false, truncated: false, refs: [], checkouts: [],
    commits: Array.from({ length: count }, (_, i) => ({
      oid: oid(i), parents: i ? [oid(i - 1)] : [], subject: `Commit ${i}`, committed_utc: "2000-01-01",
    })),
  };
}
const viewport = { width: 640, height: 400 };
function band(projectId: string, layout: GitLayout | null, top = 0, height = 500): GitViewportBand {
  return { projectId, layout, top, height };
}

describe("Git full-content overview fit", () => {
  it.each([{ width: 120_000, height: 80_000 }, { width: 8_000, height: 160_000 }])("fits huge wide or tall histories below the detail zoom floor", (world) => {
    const camera = fitGitOverview(world, viewport)!;
    expect(camera.k).toBeGreaterThan(0);
    expect(camera.k).toBeLessThan(0.05);
    expect(camera.x).toBeGreaterThanOrEqual(24);
    expect(camera.y).toBeGreaterThanOrEqual(24);
    expect(camera.x + world.width * camera.k).toBeLessThanOrEqual(viewport.width - 24);
    expect(camera.y + world.height * camera.k).toBeLessThanOrEqual(viewport.height - 24);
  });

  it("refits both dimensions when a small viewport or inspector changes the pane", () => {
    const world = { width: 10_000, height: 6_000 };
    const wide = fitGitOverview(world, { width: 1440, height: 900 })!;
    const inspector = fitGitOverview(world, { width: 980, height: 900 })!;
    const mobile = fitGitOverview(world, { width: 280, height: 240 })!;
    expect(inspector.k).toBeLessThan(wide.k);
    expect(mobile.k).toBeLessThan(inspector.k);
    expect(mobile.x + world.width * mobile.k).toBeLessThanOrEqual(256);
    expect(mobile.y + world.height * mobile.k).toBeLessThanOrEqual(216);
    const tiny = fitGitOverview(world, { width: 20, height: 10 })!;
    expect(tiny.k).toBeGreaterThan(0);
    expect(tiny.x + world.width * tiny.k).toBeLessThanOrEqual(20);
    expect(tiny.y + world.height * tiny.k).toBeLessThanOrEqual(10);
  });

  it("centers compact content without forcing a one-to-one scale", () => {
    expect(fitGitOverview({ width: 100, height: 50 }, { width: 448, height: 248 }))
      .toEqual({ k: 4, x: 24, y: 24 });
    for (const invalid of [{ width: 0, height: 400 }, { width: Infinity, height: 400 }, { width: 640, height: NaN }])
      expect(fitGitOverview({ width: 100, height: 50 }, invalid)).toBeNull();
    expect(fitGitOverview({ width: 0, height: 0 }, viewport)).toBeNull();
  });
});

describe("Git viewport anchoring", () => {
  it("keeps a visible selected OID fixed through zoom-dependent range layout", () => {
    const repo = repository();
    const oldZoom = 0.2, newZoom = 1;
    const oldLayout = layoutGit(repo, new Set(), oldZoom);
    const newLayout = layoutGit(repo, new Set(), newZoom);
    const selected = { projectId: "one", oid: oid(23) };
    const mark = oldLayout.byOid.get(selected.oid)!;
    const scroll = { left: mark.x * oldZoom - 280, top: 600 };
    const anchor = captureGitAnchor({
      bands: [band("one", oldLayout, 650)], zoom: oldZoom, scroll, viewport, selected,
    });
    expect(anchor).toMatchObject({ ...selected, screenX: 280, screenY: 142 });
    const restored = restoreGitAnchor(anchor, [band("one", newLayout, 810)], newZoom)!;
    const next = newLayout.byOid.get(selected.oid)!;
    expect(next.x * newZoom - restored.left).toBeCloseTo(anchor!.screenX);
    expect(810 + 48 + next.y * newZoom - restored.top).toBeCloseTo(anchor!.screenY);
    // Scaling the previous world coordinate would lose this commit's position.
    expect(restored.left).not.toBeCloseTo((scroll.left + 280) / oldZoom * newZoom - 280);
  });

  it("ignores an offscreen selection and anchors near the current viewport center", () => {
    const repo = repository(8), expanded = new Set(repo.commits.map((commit) => commit.oid));
    const layout = layoutGit(repo, expanded);
    const anchor = captureGitAnchor({
      bands: [band("one", layout)], zoom: 1,
      scroll: { left: 350, top: 0 }, viewport,
      selected: { projectId: "one", oid: oid(0) },
    });
    expect(anchor?.oid).toBe(oid(5));
    expect(anchor?.screenX).toBe(274);
  });

  it("prefers the project under the viewport center over a closer adjacent mark", () => {
    const layout = layoutGit(repository(2));
    const anchor = captureGitAnchor({
      bands: [band("above", layout, 0, 200), band("center", layout, 200, 400)],
      zoom: 1, scroll: { left: 0, top: 20 }, viewport,
    });
    expect(anchor?.projectId).toBe("center");
    expect(anchor?.screenY).toBe(272);
  });

  it("uses range membership to retain a selected interior OID across expansion", () => {
    const repo = repository(), compact = layoutGit(repo);
    const selected = { projectId: "one", oid: oid(12) };
    const anchor = captureGitAnchor({
      bands: [band("one", compact)], zoom: 1,
      scroll: { left: 0, top: 0 }, viewport, selected,
    });
    expect(anchor?.oid).toBe(selected.oid);
    const expanded = layoutGit(repo, new Set([selected.oid]));
    const restored = restoreGitAnchor(anchor, [band("one", expanded)], 1)!;
    expect(expanded.byOid.get(selected.oid)!.x - restored.left).toBe(anchor!.screenX);
  });

  it("clamps scroll origins and handles missing, empty or invalid geometry", () => {
    const bands = [band("one", layoutGit(repository(2)))];
    expect(restoreGitAnchor({ projectId: "one", oid: oid(0), screenX: 600, screenY: 600 }, bands, 1))
      .toEqual({ left: 0, top: 0 });
    expect(restoreGitAnchor({ projectId: "absent", oid: oid(0), screenX: 0, screenY: 0 }, bands, 1)).toBeNull();
    expect(restoreGitAnchor({ projectId: "one", oid: "missing", screenX: 0, screenY: 0 }, bands, 1)).toBeNull();
    expect(restoreGitAnchor(null, bands, 1)).toBeNull();
    for (const empty of [[], [band("empty", null)], [band("empty", layoutGit(repository(0)))]])
      expect(captureGitAnchor({ bands: empty, zoom: 1, scroll: { left: 0, top: 0 }, viewport })).toBeNull();
    expect(captureGitAnchor({ bands, zoom: 0, scroll: { left: 0, top: 0 }, viewport })).toBeNull();
    expect(captureGitAnchor({ bands, zoom: 1, scroll: { left: 0, top: 0 }, viewport: { width: 0, height: 400 } })).toBeNull();
  });
});

describe("Git observed frontier", () => {
  it("keeps a short visible repository in view despite a much wider following project", () => {
    const short = layoutGit(repository(2)), longRepo = repository(100);
    const long = layoutGit(longRepo, new Set(longRepo.commits.map((commit) => commit.oid)));
    expect(long.width).toBeGreaterThan(short.width * 10);
    const position = gitFrontierScroll({
      bands: [band("short", short), band("long", long, 500)],
      zoom: 1, scroll: { left: long.width - viewport.width, top: 0 }, viewport,
    });
    expect(position).toEqual({ left: short.width - viewport.width, top: 0 });
  });

  it("follows the lower project under the viewport center in its own coordinates", () => {
    const longRepo = repository(100);
    const long = layoutGit(longRepo, new Set(longRepo.commits.map((commit) => commit.oid)));
    const position = gitFrontierScroll({
      bands: [band("short", layoutGit(repository(2))), band("long", long, 500)],
      zoom: 1, scroll: { left: 0, top: 450 }, viewport,
    });
    expect(position).toEqual({ left: long.width - viewport.width, top: 450 });
  });

  it("prefers the rightmost currently visible lane to an offscreen later mark", () => {
    const base = layoutGit(repository(3));
    const marks = base.marks.map((mark, index) => ({ ...mark, x: [100, 300, 1500][index], y: [40, 80, 600][index] }));
    const layout = { ...base, width: 1640, marks, byOid: new Map(marks.map((mark) => [mark.commit.oid, mark])) };
    expect(gitFrontierScroll({
      bands: [band("lanes", layout, 0, 1000)], zoom: 1,
      scroll: { left: 900, top: 0 }, viewport: { width: 200, height: 200 },
    })).toEqual({ left: 196, top: 0 });
    // No current marks are visible: center the rightmost observed lane instead.
    expect(gitFrontierScroll({
      bands: [band("lanes", layout, 0, 1000)], zoom: 1,
      scroll: { left: 0, top: 300 }, viewport: { width: 200, height: 200 },
    })).toEqual({ left: 1440, top: 548 });
  });

  it("honors an explicit project and skips an empty center band", () => {
    const layout = layoutGit(repository(2));
    const bands = [band("first", layout, 0, 100), band("empty", null, 100, 400), band("last", layout, 500)];
    expect(gitFrontierScroll({ bands, zoom: 1, scroll: { left: 0, top: 0 }, viewport, projectId: "last" }))
      .toEqual({ left: layout.width - viewport.width, top: 500 });
    expect(gitFrontierScroll({ bands, zoom: 1, scroll: { left: 0, top: 0 }, viewport }))
      .toEqual({ left: layout.width - viewport.width, top: 0 });
    expect(gitFrontierScroll({ bands, zoom: 1, scroll: { left: 0, top: 0 }, viewport, projectId: "empty" })).toBeNull();
    expect(gitFrontierScroll({ bands: [], zoom: 1, scroll: { left: 0, top: 0 }, viewport })).toBeNull();
    expect(gitFrontierScroll({ bands, zoom: NaN, scroll: { left: 0, top: 0 }, viewport })).toBeNull();
    expect(gitFrontierScroll({ bands, zoom: 1, scroll: { left: 0, top: 0 }, viewport: { width: 0, height: 400 } })).toBeNull();
  });

  it("centers the nearest populated repository when the viewport contains only empty projects", () => {
    const layout = layoutGit(repository(2));
    const bands = [band("nearest", layout, 0, 180), band("empty", null, 180, 1200), band("distant", layout, 1380)];
    expect(gitFrontierScroll({ bands, zoom: 1, scroll: { left: 200, top: 400 }, viewport }))
      .toEqual({ left: layout.width - viewport.width, top: 0 });
    expect(gitFrontierScroll({ bands, zoom: 1, scroll: { left: 200, top: 950 }, viewport }))
      .toEqual({ left: layout.width - viewport.width, top: 1380 });
  });

  it("reaches the actual layout end for a sole populated repository at every tested zoom", () => {
    const repo = repository(40), expanded = new Set(repo.commits.map((commit) => commit.oid));
    for (const zoom of [0.2, 0.9, 1, 2]) {
      const layout = layoutGit(repo, expanded, zoom);
      const position = gitFrontierScroll({
        bands: [band("only", layout, 0, layout.height * zoom + 76)],
        zoom, scroll: { left: 0, top: 0 }, viewport,
      })!;
      expect(layout.width * zoom - viewport.width - position.left).toBeCloseTo(0);
    }
  });
});

describe("Git viewport fit", () => {
  it("finds the largest scale fitting both measured dimensions", () => {
    const measure = (zoom: number) => ({ width: 100 + 1200 * zoom, height: 200 + 900 * zoom });
    const zoom = fitGitZoom(measure, { width: 800, height: 650 });
    expect(zoom).toBeCloseTo(0.5, 8);
    expect(measure(zoom).height).toBeLessThanOrEqual(650);
    expect(measure(zoom + 0.00001).height).toBeGreaterThan(650);
  });

  it("remeasures actual range geometry instead of scaling its current dimensions", () => {
    const repo = repository(100);
    const measure = (zoom: number) => {
      const layout = layoutGit(repo, new Set(), zoom);
      return { width: layout.width * zoom, height: 48 + layout.height * zoom };
    };
    const size = { width: 400, height: 300 }, zoom = fitGitZoom(measure, size);
    expect(zoom).toBeGreaterThan(0.05);
    expect(zoom).toBeLessThan(1);
    expect(measure(zoom).width).toBeLessThanOrEqual(size.width);
    expect(measure(zoom + 0.00001).width).toBeGreaterThan(size.width);
    expect(zoom).not.toBeCloseTo(size.width / measure(1).width, 3);
  });

  it("returns the minimum when fixed extents cannot fit", () => {
    expect(fitGitZoom(() => ({ width: 900, height: 500 }), viewport)).toBe(0.05);
    expect(fitGitZoom(() => ({ width: Infinity, height: 1 }), viewport, 0.1, 2)).toBe(0.1);
  });

  it("returns the maximum for an empty graph and finite bounds for unusable viewports", () => {
    expect(fitGitZoom(() => ({ width: 0, height: 0 }), viewport)).toBe(1);
    expect(fitGitZoom(() => ({ width: 10, height: 10 }), viewport, 0.2, 2)).toBe(2);
    for (const invalid of [{ width: 0, height: 400 }, { width: 640, height: 0 }, { width: NaN, height: 10 }])
      expect(fitGitZoom(() => ({ width: 0, height: 0 }), invalid)).toBe(0.05);
    expect(fitGitZoom(() => ({ width: 0, height: 0 }), viewport, NaN, Infinity)).toBe(1);
  });
});
