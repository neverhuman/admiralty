import { memo, useEffect, useMemo, useRef, useState } from "react";
import { useRoute, routeHref } from "../../router";
import { ModelBadge, modelFamily } from "../../ui/ModelBadge";
import { ProfileAvatar } from "../../ui/Profile";
import { useProfile } from "../../profile";
import { age, isStale, type ViewAgent, type ViewsSnapshot } from "./feed";
import {
  fitCamera,
  layoutAtlas,
  visible,
  zoomAt,
  type AtlasNode,
  type Camera,
  type SlotRegistry,
} from "./geometry";
import { useSize, type Selection } from "./LiveViews";

export const palette: Record<string, string> = {
  luna: "#58d68d",
  sol: "#e4bc66",
  astra: "#ff837a",
  claude: "#c7a1ff",
  gemini: "#70b8ff",
  local: "#6bdce5",
  grok: "#dbe8f8",
  unknown: "#9ba9ba",
};
export function changeParams(values: Record<string, string | undefined>) {
  const route =
    window.location.hash.split("?")[0].replace(/^#\/?/, "") === "git"
      ? "git"
      : "atlas";
  const params = new URLSearchParams(window.location.hash.split("?")[1]);
  for (const [k, v] of Object.entries(values)) {
    if (v) params.set(k, v);
    else params.delete(k);
  }
  history.replaceState(null, "", routeHref(route, Object.fromEntries(params)));
  window.dispatchEvent(new HashChangeEvent("hashchange"));
}
function matches(
  agent: ViewAgent,
  q: string,
  family: string,
  state: string,
  project: string,
) {
  return (
    (!q ||
      [
        agent.name,
        agent.model,
        agent.requested_model,
        agent.node,
        agent.task,
        agent.project_id,
        agent.id,
        agent.branch,
      ]
        .join(" ")
        .toLowerCase()
        .includes(q.toLowerCase())) &&
    (!family || modelFamily(agent.model ?? "") === family) &&
    (!state ||
      (state === "observed" && agent.presence === "observed") ||
      (state === "unobserved" && agent.presence === "unobserved") ||
      (state === "attention" &&
        /blocked|quota|waiting|error/i.test(agent.state))) &&
    (!project || agent.project_id === project)
  );
}

const cameraMemory = new Map<string, Camera>();
const AgentCard = memo(function AgentCard({
  node,
  selected,
  dim,
  compact,
  onSelect,
  now,
}: {
  node: AtlasNode;
  selected: boolean;
  dim: boolean;
  compact: boolean;
  onSelect: (a: ViewAgent) => void;
  now: number;
}) {
  const a = node.agent,
    roleLabel =
      a.role === "commodore"
        ? "COMMODORE / PROJECT CAPTAIN"
        : a.role === "assignment"
          ? "TASK ASSIGNMENT"
          : a.role.toUpperCase(),
    family = modelFamily(a.model ?? ""),
    stale = isStale(a.observed_utc, now),
    attention = /blocked|quota|error|waiting/i.test(a.state);
  return (
    <button
      className={`atlas-agent family-${family} ${selected ? "selected" : ""} ${dim ? "dim" : ""} ${compact ? "compact-mark" : ""} ${!compact && node.width < 300 ? "worker-card" : ""} ${a.role} ${stale ? "stale" : ""}`}
      style={
        {
          left: node.x,
          top: node.y,
          width: node.width,
          height: node.height,
          "--agent-color": palette[family],
        } as React.CSSProperties
      }
      onClick={() => onSelect(a)}
      aria-label={`${roleLabel} ${a.name} ${a.model ?? "Reported model unknown"} ${a.effort ?? ""} ${a.requested_model && a.model && a.requested_model !== a.model ? "mismatch" : ""} ${a.task ?? (a.presence === "unobserved" ? "Presence unknown" : a.state)} ${a.node ?? "Node unknown"} · ${a.presence} · ${a.state}`}
      title={`${a.name} · ${a.role} · ${a.state} · ${age(a.observed_utc, now)}`}
    >
      <div className="atlas-agent-top">
        <span className="atlas-agent-role">
          {a.role === "commodore"
            ? "COMMODORE / PROJECT CAPTAIN"
            : a.role === "assignment"
              ? "TASK ASSIGNMENT"
              : a.role.toUpperCase()}
        </span>
        <i
          className={`atlas-presence ${a.presence} ${attention ? "attention" : ""}`}
          aria-hidden="true"
        />
      </div>
      <strong>{a.name}</strong>
      <div className="atlas-model">
        <ModelBadge
          model={a.model ?? "Reported model unknown"}
          effort={a.effort ?? undefined}
        />
        {a.requested_model && a.model && a.requested_model !== a.model && (
          <span className="atlas-mismatch">mismatch</span>
        )}
      </div>
      <div className="atlas-agent-bottom">
        <span>
          {a.task ??
            (a.presence === "unobserved" ? "Presence unknown" : a.state)}
        </span>
        <span>{a.node ?? "Node unknown"}</span>
      </div>
    </button>
  );
});
export default function FleetAtlas({
  slots,
  snapshot,
  selection,
  onSelect,
  now,
}: {
  slots: SlotRegistry;
  snapshot: ViewsSnapshot;
  selection: Selection | null;
  onSelect: (s: Selection | null) => void;
  now: number;
}) {
  const route = useRoute(),
    q = route.params.get("q") ?? "",
    family = route.params.get("family") ?? "",
    state = route.params.get("state") ?? "",
    project = route.params.get("project") ?? "";
  const display = route.params.get("display");
  const [outline, setOutlineState] = useState(() =>
      display
        ? display === "roster"
        : window.matchMedia("(max-width: 600px)").matches,
    ),
    [shown, setShown] = useState(120),
    [camera, setCamera] = useState<Camera>(
      () => cameraMemory.get("atlas") ?? { x: 24, y: 24, k: 0.6 },
    );
  const setOutline = (value: boolean) => {
    setOutlineState(value);
    changeParams({ display: value ? "roster" : "map" });
  };
  useEffect(() => {
    setOutlineState(
      display
        ? display === "roster"
        : window.matchMedia("(max-width: 600px)").matches,
    );
  }, [display]);
  useEffect(() => {
    cameraMemory.set("atlas", camera);
  }, [camera]);
  const search = useRef<HTMLInputElement>(null),
    drag = useRef<{
      x: number;
      y: number;
      camera: Camera;
      moved: boolean;
    } | null>(null);
  const viewport = useSize<HTMLDivElement>();
  useEffect(() => {
    const element = viewport.element;
    if (!element) return;
    const wheel = (event: WheelEvent) => {
      event.preventDefault();
      const unit =
        event.deltaMode === 1
          ? 16
          : event.deltaMode === 2
            ? element.clientHeight
            : 1;
      const dx = event.deltaX * unit,
        dy = event.deltaY * unit;
      if (event.ctrlKey || event.metaKey) {
        const r = element.getBoundingClientRect();
        setCamera((c) =>
          zoomAt(
            c,
            { x: event.clientX - r.left, y: event.clientY - r.top },
            Math.exp(-Math.max(-100, Math.min(100, dy)) * 0.008),
          ),
        );
      } else setCamera((c) => ({ ...c, x: c.x - dx, y: c.y - dy }));
    };
    element.addEventListener("wheel", wheel, { passive: false });
    return () => element.removeEventListener("wheel", wheel);
  }, [viewport.element]);
  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      if (
        event.key === "/" &&
        !event.ctrlKey &&
        !event.metaKey &&
        !event.altKey &&
        !(
          event.target instanceof HTMLElement &&
          event.target.closest("input,textarea,select,[contenteditable=true]")
        )
      ) {
        event.preventDefault();
        search.current?.focus();
      }
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, []);
  const profile = useProfile();
  const layout = useMemo(() => layoutAtlas(snapshot, slots), [snapshot, slots]);
  const matching = useMemo(
    () => snapshot.agents.filter((a) => matches(a, q, family, state, project)),
    [snapshot, q, family, state, project],
  );
  const matchIds = useMemo(
    () => new Set(matching.map((a) => a.id)),
    [matching],
  );
  const fit = () =>
    setCamera(
      fitCamera(
        { x: 0, y: 0, width: layout.width, height: layout.height },
        viewport,
      ),
    );
  const initialized = useRef(cameraMemory.has("atlas"));
  useEffect(() => {
    if (viewport.width > 0 && !initialized.current) {
      initialized.current = true;
      setCamera(
        fitCamera(
          { x: 0, y: 0, width: layout.width, height: layout.height },
          viewport,
        ),
      );
    }
  }, [viewport.width, viewport.height, layout]);
  const focused = selection?.kind === "agent" ? selection.agent.id : "";
  useEffect(() => {
    if (!focused) return;
    const node = layout.nodes.find((n) => n.agent.id === focused);
    if (node)
      setCamera({
        k: 1,
        x: viewport.width / 2 - node.x - node.width / 2,
        y: viewport.height / 2 - node.y - node.height / 2,
      });
  }, [focused]); // Routine source updates never move the camera.
  const zoom = (factor: number) =>
    setCamera((c) =>
      zoomAt(c, { x: viewport.width / 2, y: viewport.height / 2 }, factor),
    );
  const select = (agent: ViewAgent) => onSelect({ kind: "agent", agent });
  const byId = useMemo(
    () => new Map(layout.nodes.map((n) => [n.agent.id, n])),
    [layout],
  );
  const rendered = layout.nodes.filter((n) =>
    visible(n, camera, viewport.width, viewport.height),
  );
  const seen = snapshot.agents.filter(
      (a) => a.presence === "observed" && a.role !== "owner",
    ).length,
    unobserved = snapshot.agents.filter(
      (a) => a.presence === "unobserved" && a.role !== "owner",
    ).length;
  const projectFocus = layout.boards.find((b) => b.id === project);
  const focusFirst = () => {
    const agent = matching.find((a) => a.role === "worker") ?? matching[0];
    if (agent) select(agent);
  };
  return (
    <section className="atlas-view" aria-label="Fleet Atlas">
      <div className="lv-toolbar">
        <label className="lv-search">
          <span aria-hidden="true" data-icon="⌕" />
          <input
            ref={search}
            aria-label="Search all agents"
            placeholder="Find agent, model, task or node…"
            value={q}
            onChange={(e) => {
              changeParams({ q: e.target.value });
              setShown(120);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") focusFirst();
            }}
          />
          <kbd>/</kbd>
        </label>
        <label className="lv-select-label">
          <span className="sr-only">Model family</span>
          <select
            aria-label="Filter model family"
            value={family}
            onChange={(e) => changeParams({ family: e.target.value })}
          >
            <option value="">All models</option>
            {Object.keys(palette).map((f) => (
              <option key={f} value={f}>
                {f}
              </option>
            ))}
          </select>
        </label>
        <select
          aria-label="Filter agent state"
          value={state}
          onChange={(e) => changeParams({ state: e.target.value })}
        >
          <option value="">All presence</option>
          <option value="observed">Observed</option>
          <option value="unobserved">Unobserved</option>
          <option value="attention">Needs attention</option>
        </select>
        {project && (
          <button
            className="lv-chip"
            onClick={() =>
              changeParams({ project: undefined, focus: undefined })
            }
          >
            {snapshot.projects.find((p) => p.id === project)?.name ?? project} ×
          </button>
        )}
        <span className="lv-toolbar-spacer" />
        <button
          className={`lv-button ${outline ? "active" : ""}`}
          aria-pressed={outline}
          onClick={() => setOutline(!outline)}
        >
          {outline ? "✦ Map" : "☷ Roster"}
        </button>
        <button className="lv-button" onClick={fit}>
          Fit fleet
        </button>
      </div>
      <div className="atlas-summary">
        <div>
          <b>{String(seen).padStart(2, "0")}</b>
          <span>observed agents</span>
        </div>
        <div>
          <b>{snapshot.projects.length}</b>
          <span>projects</span>
        </div>
        <div>
          <b>{unobserved}</b>
          <span>presence unknown</span>
        </div>
        <span className="atlas-summary-note">
          {q || family || state || project
            ? `${matching.length} matching identities`
            : "Every discovered identity. Stable positions. Live evidence."}
        </span>
        <div className="atlas-legend">
          {["luna", "sol", "astra", "claude", "gemini", "local"].map((f) => (
            <span key={f}>
              <i style={{ background: palette[f] }} />
              {f}
            </span>
          ))}
        </div>
      </div>
      {outline ? (
        <div
          className="atlas-roster"
          role="region"
          aria-label="Complete agent roster"
        >
          <p>
            {matching.length} matching / {snapshot.agents.length} total
            identities. Roles and assignments include unobserved presence.
          </p>
          <table>
            <thead>
              <tr>
                <th>Agent / role</th>
                <th>Project / node</th>
                <th>Reported model</th>
                <th>Presence / activity</th>
                <th>Source time</th>
              </tr>
            </thead>
            <tbody>
              {matching.slice(0, shown).map((a) => (
                <tr key={a.id}>
                  <td>
                    <button
                      className="lv-text-button"
                      onClick={() => select(a)}
                    >
                      {a.name}
                    </button>
                    <small>{a.role}</small>
                  </td>
                  <td>
                    {a.project_id ?? "Fleet / unassigned"}
                    <small>{a.node ?? "Node unknown"}</small>
                  </td>
                  <td>
                    <ModelBadge
                      model={a.model ?? "Unknown"}
                      effort={a.effort ?? undefined}
                    />
                  </td>
                  <td>
                    {a.presence}
                    <small>{a.state}</small>
                  </td>
                  <td>{age(a.observed_utc, now)}</td>
                </tr>
              ))}
            </tbody>
          </table>
          {!matching.length && (
            <div className="lv-empty">
              <h2>No matching identities</h2>
              <p>Clear search or filters to restore the full roster.</p>
            </div>
          )}
          {matching.length > shown && (
            <button
              className="lv-button"
              onClick={() => setShown((n) => n + 200)}
            >
              Show next {Math.min(200, matching.length - shown)} identities
            </button>
          )}
        </div>
      ) : (
        <div
          className="atlas-viewport"
          ref={viewport.ref}
          role="region"
          aria-label="Fleet map. Drag background to pan. Plus and minus zoom. Zero fits fleet. O opens roster."
          tabIndex={0}
          onKeyDown={(e) => {
            if ((e.target as HTMLElement).matches("input,select,button,a"))
              return;
            if (e.key === "/") {
              e.preventDefault();
              search.current?.focus();
            }
            if (e.key === "0") {
              e.preventDefault();
              fit();
            }
            if (e.key === "+" || e.key === "=") {
              e.preventDefault();
              zoom(1.2);
            }
            if (e.key === "-") {
              e.preventDefault();
              zoom(1 / 1.2);
            }
            if (e.key.toLowerCase() === "o") setOutline(true);
            if (
              ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(
                e.key,
              )
            ) {
              e.preventDefault();
              setCamera((c) => ({
                ...c,
                x:
                  c.x +
                  (e.key === "ArrowLeft"
                    ? 64
                    : e.key === "ArrowRight"
                      ? -64
                      : 0),
                y:
                  c.y +
                  (e.key === "ArrowUp" ? 64 : e.key === "ArrowDown" ? -64 : 0),
              }));
            }
          }}
          onPointerDown={(e) => {
            if ((e.target as HTMLElement).closest("button,a,input,select"))
              return;
            drag.current = { x: e.clientX, y: e.clientY, camera, moved: false };
            e.currentTarget.setPointerCapture(e.pointerId);
          }}
          onPointerMove={(e) => {
            if (!drag.current) return;
            const d = drag.current;
            d.moved ||=
              Math.abs(e.clientX - d.x) + Math.abs(e.clientY - d.y) > 4;
            setCamera({
              ...d.camera,
              x: d.camera.x + e.clientX - d.x,
              y: d.camera.y + e.clientY - d.y,
            });
          }}
          onPointerUp={(e) => {
            if (drag.current && e.currentTarget.hasPointerCapture(e.pointerId))
              e.currentTarget.releasePointerCapture(e.pointerId);
            drag.current = null;
          }}
          onPointerCancel={() => {
            drag.current = null;
          }}
        >
          <div
            className={`atlas-world ${camera.k < 0.65 ? "summary-cards" : ""}`}
            style={
              {
                width: layout.width,
                height: layout.height,
                transform: `translate(${camera.x}px,${camera.y}px) scale(${camera.k})`,
                "--atlas-text-scale": 1 / camera.k,
              } as React.CSSProperties
            }
          >
            <svg
              className="atlas-connections"
              width={layout.width}
              height={layout.height}
              aria-hidden="true"
            >
              <defs>
                <linearGradient
                  id="atlas-command-line"
                  x1="0"
                  y1="0"
                  x2="0"
                  y2="1"
                >
                  <stop stopColor="#599ff5" stopOpacity=".8" />
                  <stop offset="1" stopColor="#599ff5" stopOpacity=".2" />
                </linearGradient>
              </defs>
              {layout.nodes
                .filter((n) => n.agent.parent_id)
                .map((node) => {
                  const parent = byId.get(node.agent.parent_id ?? "");
                  if (!parent) return null;
                  const x = parent.x + parent.width / 2,
                    y = parent.y + parent.height,
                    tx = node.x + node.width / 2,
                    ty = node.y;
                  return (
                    <path
                      key={node.agent.id}
                      className={
                        node.agent.role === "watcher" ? "advisory" : ""
                      }
                      d={`M${x} ${y} V${y + (ty - y) / 2} H${tx} V${ty}`}
                      fill="none"
                    />
                  );
                })}
            </svg>
            {layout.boards
              .filter((b) =>
                visible(b, camera, viewport.width, viewport.height, 240),
              )
              .map((b) => (
                <div
                  className={`atlas-board ${project && b.id !== project ? "dim" : ""}`}
                  key={b.id}
                  style={{
                    left: b.x,
                    top: b.y,
                    width: b.width,
                    height: b.height,
                  }}
                >
                  <header>
                    <span className="atlas-project-number">
                      {String(layout.boards.indexOf(b) + 1).padStart(2, "0")}
                    </span>
                    <button
                      onClick={() => setCamera(fitCamera(b, viewport, 64))}
                    >
                      {b.name}
                    </button>
                    <span>
                      {b.nodes.filter((n) => n.agent.role === "worker").length}{" "}
                      workers
                    </span>
                    <a
                      href={routeHref("git", { project: b.id })}
                      aria-label={`Open ${b.name} in Git Flow`}
                    >
                      <span data-icon="⑂" aria-hidden="true" />
                    </a>
                  </header>
                </div>
              ))}
            {rendered.map((node) => (
              <AgentCard
                key={node.agent.id}
                node={node}
                selected={focused === node.agent.id}
                dim={!matchIds.has(node.agent.id)}
                compact={camera.k < 0.35}
                onSelect={select}
                now={now}
              />
            ))}
            <div className="atlas-owner-avatar" style={{ left: 54, top: 65 }}>
              <ProfileAvatar
                name={
                  snapshot.agents.find((a) => a.role === "owner")?.name ??
                  "Owner"
                }
                profile={profile}
              />
            </div>
          </div>
          <div className="atlas-map-caption">
            <span>COMMAND RELATIONSHIPS</span>
            <p>
              Solid: configured supervision
              <br />
              Dashed: watcher / advisory
            </p>
          </div>
          {projectFocus && !focused && (
            <button
              className="atlas-focus-project lv-button"
              onClick={() => setCamera(fitCamera(projectFocus, viewport, 64))}
            >
              Focus {projectFocus.name}
            </button>
          )}
          <div className="atlas-camera-controls">
            <button aria-label="Zoom out" onClick={() => zoom(1 / 1.2)}>
              <span data-icon="−" aria-hidden="true" />
            </button>
            <span>{Math.round(camera.k * 100)}%</span>
            <button aria-label="Zoom in" onClick={() => zoom(1.2)}>
              <span data-icon="+" aria-hidden="true" />
            </button>
            <button aria-label="Reset map to fit fleet" onClick={fit}>
              <span data-icon="⌖" aria-hidden="true" />
            </button>
          </div>
          <div className="atlas-minimap">
            <svg
              viewBox={`0 0 ${layout.width} ${layout.height}`}
              role="group"
              aria-label="Fleet minimap"
            >
              <rect
                width={layout.width}
                height={layout.height}
                fill="transparent"
              />
              {layout.boards.map((b) => (
                <g
                  key={b.id}
                  role="button"
                  tabIndex={0}
                  aria-label={`Focus project ${b.name}`}
                  onClick={() => setCamera(fitCamera(b, viewport, 64))}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      setCamera(fitCamera(b, viewport, 64));
                    }
                  }}
                >
                  <rect
                    x={b.x}
                    y={b.y}
                    width={b.width}
                    height={b.height}
                    fill="#5077a455"
                    stroke="#7499c4"
                  />
                </g>
              ))}
              <rect
                x={-camera.x / camera.k}
                y={-camera.y / camera.k}
                width={viewport.width / camera.k}
                height={viewport.height / camera.k}
                fill="#78b3ff14"
                stroke="#a6cfff"
                strokeWidth={Math.max(4, layout.width / 250)}
              />
            </svg>
            <span>FLEET NAVIGATOR</span>
          </div>
        </div>
      )}
      <div className="lv-view-note">
        {snapshot.simulated ? "Synthetic scene" : "Observed inventory"} ·{" "}
        {outline
          ? "Select any row to inspect evidence."
          : "Drag to pan · ⌘/Ctrl + wheel to zoom · 0 fit · O roster"}
        <span>Missing provider-child coverage stays unknown.</span>
      </div>
    </section>
  );
}
