import { describe, expect, it } from "vitest";
import type { GitRepository } from "./feed";
import { layoutGit, type GitLayout } from "./geometry";
import { captureGitAnchor, fitGitZoom, restoreGitAnchor, type GitViewportBand } from "./git-viewport";

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
