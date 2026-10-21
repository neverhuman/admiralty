import { useEffect, useRef } from "react";
import type { Camera, Point } from "../live-views/geometry";
import type { GitGraphLayout } from "./model";
import type { Viewport } from "./camera";

const width = 168, height = 84, inset = 6;
export default function Minimap({ layout, camera, viewport, onNavigate }: { layout: GitGraphLayout; camera: Camera; viewport: Viewport; onNavigate: (point: Point) => void }) {
  const ref = useRef<HTMLCanvasElement>(null);
  const scale = Math.min((width - inset * 2) / Math.max(1, layout.bounds.maxX - layout.bounds.minX), (height - inset * 2) / Math.max(1, layout.bounds.maxY - layout.bounds.minY));
  const x = width / 2 - (layout.bounds.minX + layout.bounds.maxX) * scale / 2;
  const y = height / 2 - (layout.bounds.minY + layout.bounds.maxY) * scale / 2;
  useEffect(() => {
    const canvas = ref.current, context = canvas?.getContext("2d");
    if (!canvas || !context) return;
    const ratio = Math.min(2, window.devicePixelRatio || 1);
    canvas.width = width * ratio; canvas.height = height * ratio;
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.clearRect(0, 0, width, height);
    for (const edge of layout.edges) {
      if (!edge.parent || edge.parent === edge.child) continue;
      context.strokeStyle = edge.kind === "backbone" ? "#70b8ff" : "#95a7bb";
      context.globalAlpha = edge.kind === "backbone" ? 1 : .5;
      context.lineWidth = edge.kind === "backbone" ? 2 : 1;
      context.beginPath(); context.moveTo(edge.parent.x * scale + x, edge.parent.y * scale + y);
      context.lineTo(edge.child.x * scale + x, edge.child.y * scale + y); context.stroke();
    }
  }, [layout, scale, x, y]);
  const navigate = (element: HTMLElement, clientX: number, clientY: number) => {
    const rect = element.getBoundingClientRect();
    onNavigate({ x: (Math.max(0, Math.min(width, (clientX - rect.left) * width / rect.width)) - x) / scale, y: (Math.max(0, Math.min(height, (clientY - rect.top) * height / rect.height)) - y) / scale });
  };
  return <div className="git-minimap" data-camera-control="true" role="group" tabIndex={0} aria-label="Loaded Git history minimap. Drag to navigate; arrow keys pan the camera."
    onPointerDown={(event) => { if (event.button !== 0) return; event.stopPropagation(); event.preventDefault(); event.currentTarget.setPointerCapture(event.pointerId); navigate(event.currentTarget, event.clientX, event.clientY); }}
    onPointerMove={(event) => { if (event.currentTarget.hasPointerCapture(event.pointerId)) { event.stopPropagation(); navigate(event.currentTarget, event.clientX, event.clientY); } }}
    onPointerUp={(event) => { event.stopPropagation(); if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId); }}
    onPointerCancel={(event) => { if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId); }}
    onClick={(event) => event.stopPropagation()}
    onKeyDown={(event) => {
      if (!["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(event.key)) return;
      event.preventDefault(); event.stopPropagation();
      const center = { x: (viewport.width / 2 - camera.x) / camera.k, y: (viewport.height / 2 - camera.y) / camera.k };
      onNavigate({ x: center.x + (event.key === "ArrowLeft" ? -1 : event.key === "ArrowRight" ? 1 : 0) * viewport.width / camera.k * .15,
        y: center.y + (event.key === "ArrowUp" ? -1 : event.key === "ArrowDown" ? 1 : 0) * viewport.height / camera.k * .15 });
    }}>
    <canvas ref={ref} width={width} height={height} aria-hidden="true" style={{ width, height }} />
    <div className="git-minimap-window" aria-hidden="true" style={{ left: -camera.x / camera.k * scale + x, top: -camera.y / camera.k * scale + y, width: viewport.width / camera.k * scale, height: viewport.height / camera.k * scale }} />
    <span>Loaded history</span>
  </div>;
}
