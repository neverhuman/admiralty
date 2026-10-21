import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import type { ViewProject } from "./feed";
import { gitRangeWidth, type Camera, type GitLayout, type Point } from "./geometry";
import { useSize } from "./LiveViews";
import { fitGitOverview } from "./git-viewport";

type Viewport = { width: number; height: number };
type RememberedView = { camera: Camera; fitted: boolean };
const remembered = new Map<string, RememberedView>();

/** Include the trailing reference label/range envelope in the fitted graph. */
export function gitProjectWorld(layout: GitLayout): Viewport {
  return {
    width: Math.max(layout.width, ...layout.marks.map((mark) => mark.x + 270),
      ...layout.ranges.map((range) => range.x + gitRangeWidth(range.commits.length, 1) / 2 + 16)),
    height: layout.height,
  };
}

/** Keep the world point under the pointer fixed while changing scale. */
export function zoomGitProjectAt(camera: Camera, point: Point, factor: number): Camera {
  if (!Number.isFinite(factor) || factor <= 0 || !Number.isFinite(camera.k) || camera.k <= 0) return camera;
  // Numerical limits protect transforms without imposing the detail graph's
  // semantic lane-size floor. Full-content fit itself has no such lower limit.
  const k = Math.max(0.000001, Math.min(64, camera.k * factor));
  return { k, x: point.x - (point.x - camera.x) * k / camera.k, y: point.y - (point.y - camera.y) * k / camera.k };
}

const controlTarget = (target: EventTarget | null) => target instanceof Element &&
  !!target.closest("button, input, textarea, select, a, [contenteditable=true]");

export default function GitProjectViewport({ project, layout, render, selectionKey }: {
  project: ViewProject;
  layout: GitLayout;
  render: (camera: Camera, viewport: Viewport) => ReactNode;
  selectionKey?: string;
}) {
  const key = JSON.stringify([project.id, project.git?.instance_id ?? null]);
  const size = useSize<HTMLDivElement>();
  const world = gitProjectWorld(layout);
  const [view, setView] = useState(() => ({ key, ...(remembered.get(key) ?? { camera: { k: 1, x: 24, y: 24 }, fitted: true }) }));
  const active = view.key === key ? view : remembered.get(key) ?? { camera: { k: 1, x: 24, y: 24 }, fitted: true };
  const camera = active.fitted ? fitGitOverview(world, size) ?? active.camera : active.camera;
  const latest = useRef({ key, camera, fitted: active.fitted, width: size.width, height: size.height, world });
  latest.current = { key, camera, fitted: active.fitted, width: size.width, height: size.height, world };
  const drag = useRef<{ pointerId: number; x: number; y: number; camera: Camera; dragging: boolean } | null>(null);
  const suppressClick = useRef(false);

  const changeCamera = (next: Camera, fitted = false) => {
    const current = latest.current;
    latest.current = { ...current, camera: next, fitted };
    remembered.set(current.key, { camera: next, fitted });
    setView({ key: current.key, camera: next, fitted });
  };
  const fit = () => {
    const current = latest.current;
    const next = fitGitOverview(current.world, current);
    if (next) changeCamera(next, true);
  };
  const zoom = (factor: number) => {
    const current = latest.current;
    changeCamera(zoomGitProjectAt(current.camera, { x: current.width / 2, y: current.height / 2 }, factor));
  };
  const actualSize = () => zoom(1 / latest.current.camera.k);

  useLayoutEffect(() => {
    remembered.set(key, { camera, fitted: active.fitted });
    if (view.key !== key) {
      drag.current = null;
      suppressClick.current = false;
      setView({ key, camera, fitted: active.fitted });
    }
  }, [key, camera.k, camera.x, camera.y, active.fitted, view.key]);

  useEffect(() => {
    const element = size.element;
    if (!element) return;
    const wheel = (event: WheelEvent) => {
      if (controlTarget(event.target)) return;
      event.preventDefault();
      const bounds = element.getBoundingClientRect();
      const current = latest.current;
      const factor = Math.exp(Math.max(-2, Math.min(2, -event.deltaY * 0.0015)));
      const next = zoomGitProjectAt(current.camera, { x: event.clientX - bounds.left, y: event.clientY - bounds.top }, factor);
      latest.current = { ...current, camera: next, fitted: false };
      remembered.set(current.key, { camera: next, fitted: false });
      setView({ key: current.key, camera: next, fitted: false });
    };
    element.addEventListener("wheel", wheel, { passive: false });
    return () => element.removeEventListener("wheel", wheel);
  }, [size.element]);

  const percent = camera.k * 100;
  const zoomLabel = `${percent >= 1 ? Math.round(percent) : Number(percent.toPrecision(2))}%`;
  return <div className="git-project-viewport" data-project-id={project.id} data-fitted={active.fitted}>
    <div className="git-project-camera-controls" role="group" aria-label="Project Git viewport controls">
      <button className="lv-button" aria-label="Zoom project out" onClick={() => zoom(1 / 1.2)}>−</button>
      <output aria-label="Current project Git zoom">{zoomLabel}</output>
      <button className="lv-button" aria-label="Zoom project in" onClick={() => zoom(1.2)}>+</button>
      <button className="lv-button" aria-label="Fit graph" title="Fit complete loaded graph · 0" onClick={fit}>Fit graph</button>
      <button className="lv-button" aria-label="Reset project zoom to 100%" title="Actual size · 1" onClick={actualSize}>[ ] 100%</button>
      <span>Drag to pan · Scroll to zoom · 0 Fit · 1 Actual size</span>
    </div>
    <div ref={size.ref} className="git-project-stage" tabIndex={0} role="group"
      aria-label={`${project.name} Git graph. Drag to pan; scroll to zoom. Press 0 to fit, 1 for actual size, plus or minus to zoom.`}
      data-selection-key={selectionKey} data-camera-x={camera.x} data-camera-y={camera.y} data-camera-k={camera.k}
      style={{ position: "relative", overflow: "hidden", touchAction: "none" }}
      onPointerDown={(event) => {
        suppressClick.current = false;
        if (event.button !== 0 || controlTarget(event.target)) return;
        drag.current = { pointerId: event.pointerId, x: event.clientX, y: event.clientY, camera: latest.current.camera, dragging: false };
      }}
      onPointerMove={(event) => {
        const start = drag.current;
        if (!start || start.pointerId !== event.pointerId) return;
        if (!(event.buttons & 1)) { drag.current = null; return; }
        const dx = event.clientX - start.x, dy = event.clientY - start.y;
        if (!start.dragging && Math.hypot(dx, dy) <= 4) return;
        if (!start.dragging) {
          start.dragging = true;
          event.currentTarget.setPointerCapture(event.pointerId);
        }
        event.preventDefault();
        changeCamera({ ...start.camera, x: start.camera.x + dx, y: start.camera.y + dy });
      }}
      onPointerLeave={() => { if (drag.current && !drag.current.dragging) drag.current = null; }}
      onPointerUp={(event) => {
        const start = drag.current;
        if (!start || start.pointerId !== event.pointerId) return;
        if (start.dragging) {
          suppressClick.current = true;
          event.preventDefault();
          event.stopPropagation();
          if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
        }
        drag.current = null;
      }}
      onPointerCancel={() => { drag.current = null; }}
      onLostPointerCapture={() => { drag.current = null; }}
      onClickCapture={(event) => {
        if (suppressClick.current && event.detail > 0) {
          event.preventDefault();
          event.stopPropagation();
          suppressClick.current = false;
        }
      }}
      onKeyDown={(event) => {
        if (event.defaultPrevented || event.ctrlKey || event.metaKey || event.altKey || controlTarget(event.target)) return;
        if (!["0", "1", "+", "=", "-"].includes(event.key)) return;
        event.preventDefault();
        event.stopPropagation();
        if (event.key === "0") fit();
        if (event.key === "1") actualSize();
        if (event.key === "+" || event.key === "=") zoom(1.2);
        if (event.key === "-") zoom(1 / 1.2);
      }}
    >
      <div className="git-project-bounds" aria-hidden="true" style={{
        position: "absolute", left: 0, top: 0, width: world.width, height: world.height,
        transform: `translate(${camera.x}px, ${camera.y}px) scale(${camera.k})`, transformOrigin: "0 0", pointerEvents: "none",
      }} />
      {render(camera, { width: size.width, height: size.height })}
    </div>
  </div>;
}
