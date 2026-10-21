import { describe, expect, it } from "vitest";
import type { Camera, Point } from "../live-views/geometry";
import {
  anchorCamera,
  cameraMemoryKey,
  defaultGitPadding,
  fitGraph,
  makeCameraMemory,
  panGraph,
  preserveCenter,
  projectPoint,
  worldAt,
  zoomGraphAt,
} from "./camera";

function expectPoint(actual: Point | null, expected: Point) {
  expect(actual).not.toBeNull();
  expect(actual!.x).toBeCloseTo(expected.x, 10);
  expect(actual!.y).toBeCloseTo(expected.y, 10);
}

describe("continuous Git camera", () => {
  it("defaults to 48px desktop padding and 16px on narrow panes", () => {
    expect(defaultGitPadding({ width: 640, height: 400 })).toBe(48);
    expect(defaultGitPadding({ width: 639, height: 400 })).toBe(16);
  });

  it("centers negative-origin graph bounds with all four padded edges contained", () => {
    const bounds = { minX: -500, minY: -800, maxX: 1500, maxY: 0 };
    const viewport = { width: 1200, height: 800 }, camera = fitGraph(bounds, viewport)!;
    const topLeft = projectPoint(camera, { x: bounds.minX, y: bounds.minY })!;
    const bottomRight = projectPoint(camera, { x: bounds.maxX, y: bounds.maxY })!;
    expect(topLeft.x).toBeGreaterThanOrEqual(48);
    expect(topLeft.y).toBeGreaterThanOrEqual(48);
    expect(bottomRight.x).toBeLessThanOrEqual(viewport.width - 48);
    expect(bottomRight.y).toBeLessThanOrEqual(viewport.height - 48);
    expectPoint(projectPoint(camera, { x: 500, y: -400 }), { x: 600, y: 400 });
  });

  it("fits 400 expanded commits without an arbitrary lower zoom floor", () => {
    const bounds = { minX: -64, minY: -300, maxX: 400 * 112 - 64, maxY: 0 };
    const camera = fitGraph(bounds, { width: 1200, height: 800 })!;
    expect(camera.k).toBeCloseTo((1200 - 96) / (400 * 112), 12);
    expect(camera.k).toBeLessThan(0.05);
    expectPoint(projectPoint(camera, { x: bounds.minX, y: -150 }), { x: 48, y: 400 });
    expectPoint(projectPoint(camera, { x: bounds.maxX, y: -150 }), { x: 1152, y: 400 });
    expect(fitGraph({ ...bounds, maxX: 10_000_000 }, { width: 1200, height: 800 })!.k).toBeLessThan(0.001);
  });

  it("caps tiny graphs at actual size and handles zero-size graph bounds", () => {
    const camera = fitGraph({ minX: -2, minY: -3, maxX: 2, maxY: 3 }, { width: 1200, height: 800 })!;
    expect(camera.k).toBe(1);
    expectPoint(projectPoint(camera, { x: 0, y: 0 }), { x: 600, y: 400 });
    const pointCamera = fitGraph({ minX: -20, minY: 10, maxX: -20, maxY: 10 }, { width: 1200, height: 800 })!;
    expect(pointCamera.k).toBe(1);
    expectPoint(projectPoint(pointCamera, { x: -20, y: 10 }), { x: 600, y: 400 });
  });

  it("bounds padding to a quarter of the smaller viewport dimension", () => {
    const camera = fitGraph({ minX: 0, minY: 0, maxX: 100, maxY: 100 }, { width: 20, height: 8 }, 96)!;
    expect(camera.k).toBeCloseTo(0.04, 12);
    const first = projectPoint(camera, { x: 0, y: 0 })!, last = projectPoint(camera, { x: 100, y: 100 })!;
    expectPoint(first, { x: 8, y: 2 }); expectPoint(last, { x: 12, y: 6 });
  });

  it("treats negative requested padding as zero", () => {
    const camera = fitGraph({ minX: 0, minY: 0, maxX: 100, maxY: 100 }, { width: 100, height: 100 }, -10)!;
    expect(camera).toEqual({ x: 0, y: 0, k: 1 });
  });

  it("rejects invalid viewports, bounds, and nonfinite fitting arithmetic", () => {
    const bounds = { minX: 0, minY: 0, maxX: 100, maxY: 100 }, viewport = { width: 1000, height: 800 };
    for (const invalid of [{ width: 0, height: 800 }, { width: 1000, height: -1 }, { width: NaN, height: 800 }]) expect(fitGraph(bounds, invalid)).toBeNull();
    for (const invalid of [{ ...bounds, minX: Infinity }, { ...bounds, maxY: NaN }, { ...bounds, minY: 200 }, { ...bounds, minX: -Number.MAX_VALUE, maxX: Number.MAX_VALUE }]) expect(fitGraph(invalid, viewport)).toBeNull();
    expect(fitGraph(bounds, viewport, NaN)).toBeNull();
  });

  it("round trips world and screen coordinates", () => {
    const camera = { x: 81, y: -32, k: 0.025 }, world = { x: -1000, y: 4000 };
    expectPoint(worldAt(camera, projectPoint(camera, world)!), world);
  });

  it("keeps the pointer's world anchor fixed through normal and clamped zoom", () => {
    const camera = { x: 50, y: -100, k: 0.25 }, point = { x: 520, y: 270 }, world = worldAt(camera, point)!;
    const zoomed = zoomGraphAt(camera, point, 2, 0.04);
    expect(zoomed.k).toBe(0.5); expectPoint(projectPoint(zoomed, world), point);
    const maximum = zoomGraphAt(camera, point, 1e300, 0.04);
    expect(maximum.k).toBe(4); expectPoint(projectPoint(maximum, world), point);
    const minimum = zoomGraphAt(camera, point, 1e-300, 0.04);
    expect(minimum.k).toBe(0.01); expectPoint(projectPoint(minimum, world), point);
  });

  it("derives the minimum zoom from fit scale and respects the absolute maximum", () => {
    const camera = { x: 0, y: 0, k: 1 }, point = { x: 50, y: 30 };
    expect(zoomGraphAt(camera, point, 1e-300, 1e-9).k).toBe(5e-10);
    const capped = zoomGraphAt(camera, point, 1e300, 20);
    expect(capped.k).toBe(4);
    expectPoint(worldAt(capped, point), worldAt(camera, point)!);
  });

  it.each([.29255, .69, 1])("allows manual zoom below5% for an ordinary fitted scale %s", (fitScale) => {
    const camera={x:48,y:137,k:fitScale}, point={x:520,y:270}, world=worldAt(camera,point)!;
    const zoomed=zoomGraphAt(camera,point,1e-9,fitScale);
    expect(zoomed.k).toBe(.01); expect(zoomed.k).toBeLessThan(.05);
    expectPoint(projectPoint(zoomed,world),point);
    expect(zoomGraphAt(zoomed,point,1e-9,fitScale)).toEqual(zoomed);
  });

  it("anchors a selected world point without altering scale", () => {
    const camera = { x: 1, y: 2, k: 0.75 }, world = { x: -200, y: 100 }, screen = { x: 600, y: 300 };
    const anchored = anchorCamera(camera, world, screen);
    expect(anchored.k).toBe(camera.k); expectPoint(projectPoint(anchored, world), screen);
  });

  it("pans by screen pixels while preserving scale", () => {
    const camera = { x: 20, y: 30, k: 0.1 };
    expect(panGraph(camera, -15, 25)).toEqual({ x: 5, y: 55, k: 0.1 });
    expect(camera).toEqual({ x: 20, y: 30, k: 0.1 });
  });

  it("preserves the manual world center when the drawable viewport resizes", () => {
    const camera = { x: 125, y: -40, k: 0.5 }, before = { width: 1000, height: 600 }, after = { width: 600, height: 900 };
    const world = worldAt(camera, { x: before.width / 2, y: before.height / 2 })!;
    const resized = preserveCenter(camera, before, after);
    expect(resized.k).toBe(camera.k);
    expectPoint(projectPoint(resized, world), { x: after.width / 2, y: after.height / 2 });
  });

  it("refuses invalid coordinate inputs and preserves the existing camera on invalid gestures", () => {
    const camera = { x: 20, y: 30, k: 0.1 }, point = { x: 50, y: 30 };
    for (const invalid of [{ ...camera, k: 0 }, { ...camera, x: NaN }, { ...camera, k: Infinity }]) {
      expect(worldAt(invalid, point)).toBeNull(); expect(projectPoint(invalid, point)).toBeNull();
      expect(zoomGraphAt(invalid, point, 2, 0.1)).toBe(invalid);
      expect(panGraph(invalid, 1, 1)).toBe(invalid);
    }
    expect(worldAt(camera, { x: Infinity, y: 0 })).toBeNull();
    expect(projectPoint(camera, { x: 0, y: NaN })).toBeNull();
    expect(zoomGraphAt(camera, { x: Infinity, y: 0 }, 2, 0.1)).toBe(camera);
    for (const factor of [0, -1, Infinity, NaN]) expect(zoomGraphAt(camera, point, factor, 0.1)).toBe(camera);
    for (const fit of [0, -1, Infinity, NaN, Number.MIN_VALUE]) expect(zoomGraphAt(camera, point, 2, fit)).toBe(camera);
    expect(panGraph(camera, Infinity, 0)).toBe(camera);
    expect(anchorCamera(camera, { x: NaN, y: 0 }, point)).toBe(camera);
    expect(preserveCenter(camera, { width: 0, height: 100 }, { width: 100, height: 100 })).toBe(camera);
    expect(preserveCenter(camera, { width: 100, height: 100 }, { width: 100, height: NaN })).toBe(camera);
    const huge: Camera = { x: Number.MAX_VALUE, y: 0, k: 1 };
    expect(panGraph(huge, Number.MAX_VALUE, 0)).toBe(huge);
    expect(worldAt(huge, { x: -Number.MAX_VALUE, y: 0 })).toBeNull();
    expect(projectPoint({ ...camera, k: 4 }, { x: Number.MAX_VALUE, y: 0 })).toBeNull();
    const tinyScale: Camera = { x: 0, y: 0, k: 1e-308 };
    expect(zoomGraphAt(tinyScale, { x: 1.5, y: 0 }, Number.MAX_VALUE, 0.1)).toBe(tinyScale);
  });
});

describe("owned Git camera memory", () => {
  it("keys access epoch, project, repository instance, and presentation without delimiter collisions", () => {
    const key = cameraMemoryKey(1, "repo", "instance");
    expect(JSON.parse(key)).toEqual([1, "repo", "instance", "project"]);
    expect(cameraMemoryKey(2, "repo", "instance")).not.toBe(key);
    expect(cameraMemoryKey(1, "other", "instance")).not.toBe(key);
    expect(cameraMemoryKey(1, "repo", "other")).not.toBe(key);
    expect(cameraMemoryKey(1, "repo", "instance", "fleet")).not.toBe(key);
    expect(cameraMemoryKey(1, "a:b", "c")).not.toBe(cameraMemoryKey(1, "a", "b:c"));
  });

  it("copies stored and returned values so callers cannot mutate another camera", () => {
    const memory = makeCameraMemory(), entry = { camera: { x: 1, y: 2, k: 0.5 }, intent: "manual" as const };
    memory.set("repo", entry); entry.camera.x = 999;
    const restored = memory.get("repo")!;
    expect(restored).toEqual({ camera: { x: 1, y: 2, k: 0.5 }, intent: "manual" });
    restored.camera.y = 999; restored.intent = "fit";
    expect(memory.get("repo")).toEqual({ camera: { x: 1, y: 2, k: 0.5 }, intent: "manual" });
  });

  it("bounds memory to 64 entries and evicts the least recently used key", () => {
    const memory = makeCameraMemory(), camera = { x: 0, y: 0, k: 1 };
    for (let n = 0; n < 64; n++) memory.set(String(n), { camera, intent: "fit" });
    expect(memory.get("0")).toBeDefined();
    memory.set("64", { camera, intent: "selection" });
    expect(memory.get("1")).toBeUndefined(); expect(memory.get("0")).toBeDefined(); expect(memory.get("64")?.intent).toBe("selection");
    memory.set("2", { camera, intent: "follow-main" });
    memory.set("65", { camera, intent: "follow-activity" });
    expect(memory.get("3")).toBeUndefined(); expect(memory.get("2")?.intent).toBe("follow-main");
  });

  it("has no global store and explicitly clears all previous-access entries", () => {
    const first = makeCameraMemory(), second = makeCameraMemory();
    const key = cameraMemoryKey(1, "repo", "instance");
    first.set(key, { camera: { x: 0, y: 0, k: 1 }, intent: "fit" });
    expect(second.get(key)).toBeUndefined();
    expect(first.get(cameraMemoryKey(2, "repo", "instance"))).toBeUndefined();
    first.clear(); expect(first.get(key)).toBeUndefined();
  });
});
