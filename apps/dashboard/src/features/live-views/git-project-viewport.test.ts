import { describe, expect, it } from "vitest";
import { gitProjectWorld, zoomGitProjectAt } from "./GitProjectViewport";
import { fitGitOverview } from "./git-viewport";
import { layoutGit } from "./geometry";
import type { GitRepository } from "./feed";

describe("project Git camera", () => {
  it("retains the exact world point under the pointer through wide zoom changes", () => {
    const before = { k: 0.01, x: -250, y: 130 }, pointer = { x: 470, y: 240 };
    const after = zoomGitProjectAt(before, pointer, 100);
    expect(after.k).toBe(1);
    expect((pointer.x - after.x) / after.k).toBeCloseTo((pointer.x - before.x) / before.k);
    expect((pointer.y - after.y) / after.k).toBeCloseTo((pointer.y - before.y) / before.k);
    const restored = zoomGitProjectAt(after, pointer, 0.01);
    expect(restored).toEqual(before);
  });

  it("permits navigation far below the old 20% detail floor with finite extremes", () => {
    const camera = zoomGitProjectAt({ k: 0.005, x: 24, y: 24 }, { x: 100, y: 100 }, 0.2);
    expect(camera.k).toBeCloseTo(0.001);
    expect(Object.values(zoomGitProjectAt(camera, { x: 100, y: 100 }, 1e100)).every(Number.isFinite)).toBe(true);
    expect(zoomGitProjectAt(camera, { x: 100, y: 100 }, NaN)).toEqual(camera);
  });

  it("fits trailing labels and collapsed ranges without changing source layout", () => {
    const repo: GitRepository = {
      instance_id: "project-camera-test", node: "test", observed_utc: "2026-10-20T00:00:00Z", shallow: false, truncated: false, refs: [], checkouts: [],
      commits: Array.from({ length: 40 }, (_, index) => ({
        oid: index.toString(16).padStart(40, "0"), parents: index ? [(index - 1).toString(16).padStart(40, "0")] : [], subject: "Camera fixture", committed_utc: "2026-10-20T00:00:00Z",
      })),
    };
    const layout = layoutGit(repo), before = structuredClone(layout), world = gitProjectWorld(layout);
    expect(world.width).toBeGreaterThanOrEqual(layout.marks.at(-1)!.x + 270);
    const camera = fitGitOverview(world, { width: 390, height: 300 })!;
    expect(camera.x).toBeGreaterThanOrEqual(24);
    expect(camera.x + world.width * camera.k).toBeLessThanOrEqual(366);
    expect(camera.y + world.height * camera.k).toBeLessThanOrEqual(276);
    expect(layout).toEqual(before);
  });
});
