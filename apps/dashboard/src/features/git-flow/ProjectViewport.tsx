import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import type { ViewProject } from "../live-views/feed";
import type { Camera, Point } from "../live-views/geometry";
import { useSize } from "../live-views/LiveViews";
import { anchorCamera, cameraMemoryKey, defaultGitPadding, fitGraph, panGraph, preserveCenter, projectPoint, zoomGraphAt, type CameraIntent, type CameraMemoryEntry, type PaddingPreference, type Viewport, type makeCameraMemory } from "./camera";
import type { GitGraphLayout } from "./model";
import Minimap from "./Minimap";

type Memory = ReturnType<typeof makeCameraMemory>;
type Controller = { preserveAnchor: (oid: string, action: () => void) => void; padding: number };
const controls = (target: EventTarget | null) => target instanceof Element && !!target.closest("button,input,textarea,select,a,[contenteditable=true],[data-camera-control]");

/** A camera belongs to an admitted repository instance, never to a global zoom. */
export default function ProjectViewport({ project, layout, accessEpoch, memory, selectionOid, render, dock }: {
  project: ViewProject;
  layout: GitGraphLayout;
  accessEpoch: number;
  memory: Memory;
  selectionOid?: string;
  render: (camera: Camera, viewport: Viewport, controller: Controller) => ReactNode;
  dock?: ReactNode;
}) {
  const key = cameraMemoryKey(accessEpoch, project.id, project.git!.instance_id, "project");
  const size = useSize<HTMLDivElement>();
  const [showMinimap, setShowMinimap] = useState(false);
  const [padding, setPadding] = useState<PaddingPreference | null>(null);
  const inset = padding ?? defaultGitPadding(size);
  const fitCamera = fitGraph(layout.bounds, size, inset);
  const fallback: CameraMemoryEntry = { camera: { k: 1, x: inset, y: inset }, intent: "fit" };
  const [view, setView] = useState(() => ({ key, ...(memory.get(key) ?? fallback) }));
  const active = view.key === key ? view : memory.get(key) ?? fallback;
  const camera = active.intent === "fit" ? fitCamera ?? active.camera : active.camera;
  const latest = useRef({ key, camera, intent: active.intent, size: { width: size.width, height: size.height }, layout, inset, fitScale: fitCamera?.k ?? 1 });
  latest.current = { key, camera, intent: active.intent, size: { width: size.width, height: size.height }, layout, inset, fitScale: fitCamera?.k ?? 1 };
  const oldSize = useRef({ key, width: size.width, height: size.height });
  const pendingAnchor = useRef<{ oid: string; screen: Point; layout: GitGraphLayout } | null>(null);
  const drag = useRef<{ id: number; start: Point; camera: Camera; moved: boolean } | null>(null);
  const pointers = useRef(new Map<number, Point>());
  const pinch = useRef<{ distance: number; middle: Point; camera: Camera } | null>(null);
  const suppressClick = useRef(false);
  const frame = useRef<number | null>(null);
  const queued = useRef<{ key: string; camera: Camera; intent: CameraIntent } | null>(null);

  const change = (next: Camera, intent: CameraIntent = "manual", immediate = false) => {
    const current = latest.current;
    latest.current = { ...current, camera: next, intent };
    memory.set(current.key, { camera: next, intent });
    queued.current = { key: current.key, camera: next, intent };
    if (immediate) {
      if (frame.current !== null) cancelAnimationFrame(frame.current);
      frame.current = null;
      setView(queued.current);
      queued.current = null;
    } else if (frame.current === null) frame.current = requestAnimationFrame(() => {
      frame.current = null;
      const nextView = queued.current;
      queued.current = null;
      if (nextView && nextView.key === latest.current.key) setView(nextView);
    });
  };
  const fit = () => {
    const current = latest.current;
    const next = fitGraph(current.layout.bounds, current.size, current.inset);
    if (next) { pendingAnchor.current = null; change(next, "fit", true); }
  };
  const zoom = (factor: number, point?: Point) => {
    const current = latest.current;
    change(zoomGraphAt(current.camera, point ?? { x: current.size.width / 2, y: current.size.height / 2 }, factor, current.fitScale));
  };
  const centerSelection = () => {
    const current = latest.current, mark = current.layout.byOid.get(selectionOid ?? "");
    if (!mark) return;
    const context = [mark, ...mark.commit.parents.flatMap((oid) => current.layout.byOid.get(oid) ?? [])];
    const bounds = { minX: Infinity, minY: Infinity, maxX: -Infinity, maxY: -Infinity };
    for (const point of context) { bounds.minX = Math.min(bounds.minX, point.x - 24); bounds.maxX = Math.max(bounds.maxX, point.x + 24); bounds.minY = Math.min(bounds.minY, point.y - 24); bounds.maxY = Math.max(bounds.maxY, point.y + 24); }
    const next = fitGraph(bounds, current.size, current.inset);
    if (next) change(next, "selection");
  };
  const latestOn = (backbone: boolean) => {
    const current = latest.current;
    const mark = backbone ? current.layout.byOid.get(current.layout.backbone.tipOid ?? "") : current.layout.marks.reduce<(typeof current.layout.marks)[number] | undefined>((last, next) => !last || next.rank > last.rank ? next : last, undefined);
    if (mark) change(anchorCamera(current.camera, mark, { x: current.size.width - current.inset - 40, y: current.size.height / 2 }), backbone ? "follow-main" : "follow-activity");
  };
  const preserveAnchor = (oid: string, action: () => void) => {
    const current = latest.current, mark = current.layout.byOid.get(oid), screen = mark && projectPoint(current.camera, mark);
    if (screen) pendingAnchor.current = { oid, screen, layout: current.layout };
    change(current.camera, "manual", true);
    action();
  };

  useLayoutEffect(() => {
    const previous = oldSize.current;
    oldSize.current = { key, width: size.width, height: size.height };
    if (view.key !== key) {
      pointers.current.clear(); pinch.current = null; drag.current = null; pendingAnchor.current = null; suppressClick.current = false;
      if (frame.current !== null) cancelAnimationFrame(frame.current);
      frame.current = null; queued.current = null;
      setView({ key, ...(memory.get(key) ?? fallback) });
      return;
    }
    const anchor = pendingAnchor.current;
    if (anchor && anchor.layout !== layout) {
      pendingAnchor.current = null;
      const mark = layout.byOid.get(anchor.oid);
      if (mark) change(anchorCamera(camera, mark, anchor.screen), "manual", true);
    } else if (active.intent !== "fit" && previous.key === key && previous.width > 0 && previous.height > 0 &&
      (previous.width !== size.width || previous.height !== size.height)) {
      change(preserveCenter(camera, previous, size), active.intent, true);
    } else memory.set(key, { camera, intent: active.intent });
  }, [key, layout, size.width, size.height, camera.x, camera.y, camera.k, active.intent, view.key, memory]);
  useEffect(() => () => { if (frame.current !== null) cancelAnimationFrame(frame.current); }, []);
  useLayoutEffect(() => {
    if (active.intent === "follow-main") latestOn(true);
    if (active.intent === "follow-activity") latestOn(false);
  }, [layout, size.width, size.height, active.intent]);
  const previousSelection = useRef(selectionOid);
  const selectionTarget = useRef<string | null>(null);
  useLayoutEffect(() => {
    if (previousSelection.current !== selectionOid) { previousSelection.current = selectionOid; selectionTarget.current = selectionOid ?? null; }
    const oid = selectionTarget.current, mark = oid && layout.byOid.get(oid);
    if (!mark || mark.range || mark.commit.oid !== oid || !size.width || !size.height) return;
    selectionTarget.current = null;
    const point = projectPoint(latest.current.camera, mark);
    if (point && (point.x < inset || point.x > size.width - inset || point.y < inset || point.y > size.height - inset))
      change(anchorCamera(latest.current.camera, mark, { x: size.width / 2, y: size.height / 2 }), "selection", true);
  }, [selectionOid, layout, size.width, size.height, inset]);
  useEffect(() => {
    const element = size.element;
    if (!element) return;
    const wheel = (event: WheelEvent) => {
      if (controls(event.target)) return;
      event.preventDefault();
      const current = latest.current;
      const unit = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? current.size.height : 1;
      if (event.ctrlKey || event.metaKey) {
        const rect = element.getBoundingClientRect();
        const point = { x: event.clientX - rect.left - element.clientLeft, y: event.clientY - rect.top - element.clientTop };
        change(zoomGraphAt(current.camera, point, Math.exp(Math.max(-2, Math.min(2, -event.deltaY * unit * .002))), current.fitScale));
      } else change(panGraph(current.camera, -(event.shiftKey && !event.deltaX ? event.deltaY : event.deltaX) * unit, -(event.shiftKey && !event.deltaX ? 0 : event.deltaY) * unit));
    };
    element.addEventListener("wheel", wheel, { passive: false });
    return () => element.removeEventListener("wheel", wheel);
  }, [size.element, memory]);

  const finishPointer = (element: HTMLElement, pointerId: number, cancelled = false) => {
    // A release can also emit lostpointercapture. Ignore that second notification
    // after custody has already moved to the remaining finger.
    if (!pointers.current.has(pointerId)) return;
    const wasPinching = pinch.current !== null;
    suppressClick.current ||= wasPinching || !!drag.current?.moved;
    pointers.current.delete(pointerId);
    pinch.current = null;
    drag.current = null;
    const release = [pointerId];
    if (cancelled) {
      release.push(...pointers.current.keys());
      pointers.current.clear();
    } else if (wasPinching && pointers.current.size === 1) {
      const [id, point] = pointers.current.entries().next().value!;
      // Continue from the last pinch camera, including any queued animation frame.
      drag.current = { id, start: point, camera: latest.current.camera, moved: true };
      if (!element.hasPointerCapture(id)) element.setPointerCapture(id);
    }
    for (const id of release) if (element.hasPointerCapture(id)) element.releasePointerCapture(id);
  };
  const percent = camera.k * 100;
  const label = `${percent >= 1 ? Math.round(percent) : Number(percent.toPrecision(2))}%`;
  return <div className="git-project-viewport" data-project-id={project.id} data-fitted={active.intent === "fit"} data-camera-intent={active.intent}>
    <div className="git-project-camera-controls" role="group" aria-label="Project Git viewport controls">
      <button className="lv-button" aria-label="Zoom project out" onClick={() => zoom(1 / 1.2)}><span aria-hidden="true" data-icon="−" /></button>
      <output aria-label="Current project Git zoom">{label}</output>
      <button className="lv-button" aria-label="Zoom project in" onClick={() => zoom(1.2)}><span aria-hidden="true" data-icon="+" /></button>
      <button className="lv-button" aria-label="Fit graph" title="Fit complete loaded graph · 0 or F" onClick={fit}>Fit graph</button>
      <button className="lv-button" aria-label="Reset project zoom to 100%" title="Actual size · 1" onClick={() => zoom(1 / latest.current.camera.k)}>[ ] 100%</button>
      <details className="git-camera-options"><summary>More</summary><div>
      <button className="lv-button" aria-pressed={showMinimap} onClick={() => setShowMinimap(!showMinimap)}>Map</button>
      <button className="lv-button" disabled={!layout.byOid.has(selectionOid ?? "")} onClick={centerSelection}>Fit selection</button>
      <button className="lv-button" disabled={layout.backbone.kind !== "resolved"} onClick={() => latestOn(true)}>Latest on backbone</button>
      <button className="lv-button" onClick={() => latestOn(false)}>Latest activity</button>
      <label>Padding <select aria-label="Graph fit padding" value={padding ?? "auto"} onChange={(event) => setPadding(event.target.value === "auto" ? null : Number(event.target.value) as PaddingPreference)}>
        <option value="auto">Auto · {defaultGitPadding(size)} px</option>{[16, 32, 48, 64, 96].map((value) => <option key={value} value={value}>{value} px</option>)}
      </select></label>
      <span>Drag / scroll to pan · Ctrl / ⌘ scroll to zoom · 0 / F Fit · 1 Actual size</span>
      <span>● Commit · ─ Actual parent · ◇ Checkout HEAD · ▫ Working changes · ⋯ Missing ancestry · [＋] Linear range</span>
      </div></details>
    </div>
    <div className="git-investigation-body">
      <div className="git-project-main">
      <div ref={size.ref} className="git-project-stage" tabIndex={0} role="group" aria-label={`${project.name} Git graph. Drag or scroll to pan; Control or Command scroll to zoom. Press 0 or F to fit, 1 for actual size.`}
        data-selection-key={selectionOid || undefined} data-camera-x={camera.x} data-camera-y={camera.y} data-camera-k={camera.k}
        onPointerDown={(event) => {
          if (event.button !== 0 || controls(event.target) || pointers.current.size >= 2) return;
          suppressClick.current = false;
          pointers.current.set(event.pointerId, { x: event.clientX, y: event.clientY });
          if (pointers.current.size === 1) drag.current = { id: event.pointerId, start: { x: event.clientX, y: event.clientY }, camera: latest.current.camera, moved: false };
          if (pointers.current.size === 2) {
            const [a, b] = [...pointers.current.values()], rect = event.currentTarget.getBoundingClientRect();
            pinch.current = { distance: Math.hypot(a.x - b.x, a.y - b.y), middle: { x: (a.x + b.x) / 2 - rect.left, y: (a.y + b.y) / 2 - rect.top }, camera: latest.current.camera };
            if (drag.current) drag.current.moved = true;
            suppressClick.current = true;
            for (const id of pointers.current.keys()) event.currentTarget.setPointerCapture(id);
          }
        }}
        onPointerMove={(event) => {
          if (!pointers.current.has(event.pointerId)) return;
          pointers.current.set(event.pointerId, { x: event.clientX, y: event.clientY });
          const pinching = pinch.current;
          if (pinching && pointers.current.size === 2) {
            const [a, b] = [...pointers.current.values()], rect = event.currentTarget.getBoundingClientRect();
            const middle = { x: (a.x + b.x) / 2 - rect.left, y: (a.y + b.y) / 2 - rect.top };
            const scaled = zoomGraphAt(pinching.camera, pinching.middle, pinching.distance > 0 ? Math.hypot(a.x - b.x, a.y - b.y) / pinching.distance : 1, latest.current.fitScale);
            change(panGraph(scaled, middle.x - pinching.middle.x, middle.y - pinching.middle.y));
            suppressClick.current = true; event.preventDefault(); return;
          }
          const start = drag.current;
          if (!start || start.id !== event.pointerId) return;
          if (event.pointerType === "mouse" && !(event.buttons & 1)) { finishPointer(event.currentTarget, event.pointerId); return; }
          const dx = event.clientX - start.start.x, dy = event.clientY - start.start.y;
          if (!start.moved && Math.hypot(dx, dy) <= 4) return;
          if (!start.moved) { start.moved = true; event.currentTarget.setPointerCapture(event.pointerId); }
          change(panGraph(start.camera, dx, dy)); event.preventDefault();
        }}
        onPointerUp={(event) => finishPointer(event.currentTarget, event.pointerId)}
        onPointerCancel={(event) => finishPointer(event.currentTarget, event.pointerId, true)}
        onLostPointerCapture={(event) => finishPointer(event.currentTarget, event.pointerId, true)}
        onPointerLeave={(event) => { if (!drag.current?.moved && !pinch.current) pointers.current.delete(event.pointerId); }}
        onClickCapture={(event) => { if (suppressClick.current && event.detail > 0) { event.preventDefault(); event.stopPropagation(); suppressClick.current = false; } }}
        onKeyDown={(event) => {
          if (event.defaultPrevented || event.ctrlKey || event.metaKey || event.altKey || controls(event.target)) return;
          const key = event.key.toLowerCase(), current = latest.current;
          if (!["0", "f", "1", "+", "=", "-", "arrowleft", "arrowright", "arrowup", "arrowdown"].includes(key)) return;
          event.preventDefault(); event.stopPropagation();
          if (key === "0" || key === "f") fit();
          else if (key === "1") zoom(1 / current.camera.k);
          else if (key === "+" || key === "=") zoom(1.2);
          else if (key === "-") zoom(1 / 1.2);
          else change(panGraph(current.camera, key === "arrowleft" ? current.size.width * .15 : key === "arrowright" ? -current.size.width * .15 : 0, key === "arrowup" ? current.size.height * .15 : key === "arrowdown" ? -current.size.height * .15 : 0));
        }}>
        <div className="git-project-bounds" aria-hidden="true" style={{ left: layout.bounds.minX, top: layout.bounds.minY, width: layout.bounds.maxX - layout.bounds.minX, height: layout.bounds.maxY - layout.bounds.minY, transform: `translate(${camera.x}px, ${camera.y}px) scale(${camera.k})`, transformOrigin: `${-layout.bounds.minX}px ${-layout.bounds.minY}px` }} />
        {render(camera, size, { preserveAnchor, padding: inset })}
      </div>
      {showMinimap && <div className="git-minimap-dock"><span>Loaded history · drag the map to navigate</span><Minimap layout={layout} camera={camera} viewport={size} onNavigate={(point) => change(anchorCamera(latest.current.camera, point, { x: size.width / 2, y: size.height / 2 }))} /></div>}
      </div>
      {dock && <div className="git-project-chat-dock">{dock}</div>}
    </div>
  </div>;
}
