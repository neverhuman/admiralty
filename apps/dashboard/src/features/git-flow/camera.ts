import type { Camera, Point } from "../live-views/geometry";
import type { GraphBounds } from "./model";

export type Viewport = { width: number; height: number };
export type PaddingPreference = 16 | 32 | 48 | 64 | 96;
export type CameraIntent = "fit" | "manual" | "selection" | "follow-main" | "follow-activity";
export type CameraMemoryEntry = { camera: Camera; intent: CameraIntent };
export type CameraMemory = {
  get(key: string): CameraMemoryEntry | undefined;
  set(key: string, value: CameraMemoryEntry): void;
  clear(): void;
};

const finitePoint = (point: Point) => Number.isFinite(point.x) && Number.isFinite(point.y);
const validCamera = (camera: Camera) => finitePoint(camera) && Number.isFinite(camera.k) && camera.k > 0;
const validViewport = (viewport: Viewport) => Number.isFinite(viewport.width) && Number.isFinite(viewport.height) && viewport.width > 0 && viewport.height > 0;

export function defaultGitPadding(viewport: Viewport): PaddingPreference {
  return viewport.width >= 640 ? 48 : 16;
}

/** Fit the actual world bounds, including negative origins, without a zoom floor. */
export function fitGraph(
  bounds: GraphBounds,
  viewport: Viewport,
  padding: number = defaultGitPadding(viewport),
): Camera | null {
  if (!validViewport(viewport) || !Number.isFinite(padding) ||
    !Number.isFinite(bounds.minX) || !Number.isFinite(bounds.maxX) ||
    !Number.isFinite(bounds.minY) || !Number.isFinite(bounds.maxY) ||
    bounds.maxX < bounds.minX || bounds.maxY < bounds.minY) return null;
  const width = bounds.maxX - bounds.minX, height = bounds.maxY - bounds.minY;
  if (!Number.isFinite(width) || !Number.isFinite(height)) return null;
  const inset = Math.min(Math.max(0, padding), viewport.width / 4, viewport.height / 4);
  const k = Math.min(1, (viewport.width - 2 * inset) / Math.max(1, width), (viewport.height - 2 * inset) / Math.max(1, height));
  const camera = {
    k,
    x: viewport.width / 2 - (bounds.minX + width / 2) * k,
    y: viewport.height / 2 - (bounds.minY + height / 2) * k,
  };
  return validCamera(camera) ? camera : null;
}

export function worldAt(camera: Camera, point: Point): Point | null {
  if (!validCamera(camera) || !finitePoint(point)) return null;
  const world = { x: (point.x - camera.x) / camera.k, y: (point.y - camera.y) / camera.k };
  return finitePoint(world) ? world : null;
}

export function projectPoint(camera: Camera, world: Point): Point | null {
  if (!validCamera(camera) || !finitePoint(world)) return null;
  const point = { x: world.x * camera.k + camera.x, y: world.y * camera.k + camera.y };
  return finitePoint(point) ? point : null;
}

export function anchorCamera(camera: Camera, world: Point, screen: Point): Camera {
  if (!validCamera(camera) || !finitePoint(world) || !finitePoint(screen)) return camera;
  const anchored = { k: camera.k, x: screen.x - world.x * camera.k, y: screen.y - world.y * camera.k };
  return validCamera(anchored) ? anchored : camera;
}

/** Preserve the world point under the pointer, including when a scale limit applies. */
export function zoomGraphAt(camera: Camera, point: Point, factor: number, fitScale: number): Camera {
  if (!validCamera(camera) || !finitePoint(point) || !Number.isFinite(factor) || factor <= 0 || !Number.isFinite(fitScale) || fitScale <= 0) return camera;
  const minScale = Math.min(4, fitScale / 2);
  if (!(minScale > 0)) return camera;
  const world = worldAt(camera, point);
  if (!world) return camera;
  const k = Math.max(minScale, Math.min(4, camera.k * factor));
  const zoomed = { k, x: point.x - world.x * k, y: point.y - world.y * k };
  return validCamera(zoomed) ? zoomed : camera;
}

export function panGraph(camera: Camera, dx: number, dy: number): Camera {
  if (!validCamera(camera) || !Number.isFinite(dx) || !Number.isFinite(dy)) return camera;
  const moved = { ...camera, x: camera.x + dx, y: camera.y + dy };
  return validCamera(moved) ? moved : camera;
}

export function preserveCenter(camera: Camera, beforeViewport: Viewport, afterViewport: Viewport): Camera {
  if (!validViewport(beforeViewport) || !validViewport(afterViewport)) return camera;
  const world = worldAt(camera, { x: beforeViewport.width / 2, y: beforeViewport.height / 2 });
  return world ? anchorCamera(camera, world, { x: afterViewport.width / 2, y: afterViewport.height / 2 }) : camera;
}

export function cameraMemoryKey(accessEpoch: number, project: string, instance: string, presentation = "project"): string {
  return JSON.stringify([accessEpoch, project, instance, presentation]);
}

/** Each view owns its memory; access transitions explicitly clear its prior entries. */
export function makeCameraMemory(): CameraMemory {
  const entries = new Map<string, CameraMemoryEntry>();
  const copy = (entry: CameraMemoryEntry): CameraMemoryEntry => ({ camera: { ...entry.camera }, intent: entry.intent });
  return {
    get(key) {
      const entry = entries.get(key);
      if (!entry) return undefined;
      entries.delete(key);
      entries.set(key, entry);
      return copy(entry);
    },
    set(key, value) {
      entries.delete(key);
      entries.set(key, copy(value));
      if (entries.size > 64) entries.delete(entries.keys().next().value!);
    },
    clear() { entries.clear(); },
  };
}
