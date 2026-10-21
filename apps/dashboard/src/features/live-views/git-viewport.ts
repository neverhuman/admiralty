import type { Camera, GitLayout } from "./geometry";

export type GitViewportBand = {
  projectId: string;
  top: number;
  height: number;
  layout: GitLayout | null;
};

export type GitViewportAnchor = {
  projectId: string;
  oid: string;
  screenX: number;
  screenY: number;
};

type Viewport = { width: number; height: number };
type Scroll = { left: number; top: number };

// Project headings remain at their native size while the ancestry below scales.
const GIT_CANVAS_HEADER = 48;

/** Uniform full-content overview fit. Readable detail controls live outside the
 * transformed scene, so very large histories may fit below the detail zoom floor.
 */
export function fitGitOverview(
  world: Viewport,
  viewport: Viewport,
  padding = 24,
): Camera | null {
  if (![world.width, world.height, viewport.width, viewport.height].every((value) => Number.isFinite(value) && value > 0)) return null;
  // Preserve a positive drawing area even when a pane is smaller than padding.
  const inset = Math.min(Number.isFinite(padding) ? Math.max(0, padding) : 24, viewport.width / 4, viewport.height / 4);
  const k = Math.min((viewport.width - inset * 2) / world.width, (viewport.height - inset * 2) / world.height);
  return { k, x: (viewport.width - world.width * k) / 2, y: (viewport.height - world.height * k) / 2 };
}

/** Retain an observed commit, rather than a world coordinate that reflows on zoom.
 * The viewport contains only the graph: its width excludes the project gutter.
 */
export function captureGitAnchor({
  bands,
  zoom,
  scroll,
  viewport,
  selected,
}: {
  bands: readonly GitViewportBand[];
  zoom: number;
  scroll: Scroll;
  viewport: Viewport;
  selected?: { projectId: string; oid: string } | null;
}): GitViewportAnchor | null {
  if (!Number.isFinite(zoom) || zoom <= 0 ||
    !Number.isFinite(viewport.width) || !Number.isFinite(viewport.height) ||
    viewport.width <= 0 || viewport.height <= 0 ||
    !Number.isFinite(scroll.left) || !Number.isFinite(scroll.top)) return null;

  const anchorFor = (band: GitViewportBand, oid: string) => {
    const mark = band.layout?.byOid.get(oid);
    if (!mark) return null;
    const anchor = {
      projectId: band.projectId,
      oid,
      screenX: mark.x * zoom - scroll.left,
      screenY: band.top + GIT_CANVAS_HEADER + mark.y * zoom - scroll.top,
    };
    return Number.isFinite(anchor.screenX) && Number.isFinite(anchor.screenY) ? anchor : null;
  };
  if (selected) {
    const band = bands.find((candidate) => candidate.projectId === selected.projectId);
    const anchor = band ? anchorFor(band, selected.oid) : null;
    if (anchor && anchor.screenX >= 0 && anchor.screenX <= viewport.width &&
      anchor.screenY >= 0 && anchor.screenY <= viewport.height) return anchor;
  }

  const populated = bands.filter((band) => band.layout?.marks.length);
  const centerY = scroll.top + viewport.height / 2;
  const centered = populated.filter((band) => band.top <= centerY && band.top + band.height > centerY);
  const visible = populated.filter((band) => band.top < scroll.top + viewport.height && band.top + band.height > scroll.top);
  const candidates = centered.length ? centered : visible.length ? visible : populated;
  let nearest: GitViewportAnchor | null = null;
  let distance = Infinity;
  for (const band of candidates) {
    for (const mark of band.layout!.marks) {
      const anchor = anchorFor(band, mark.commit.oid);
      if (!anchor) continue;
      const next = (anchor.screenX - viewport.width / 2) ** 2 +
        (anchor.screenY - viewport.height / 2) ** 2;
      if (next < distance) {
        nearest = anchor;
        distance = next;
      }
    }
  }
  return nearest;
}

/** Resolve the same OID against the newly measured layout, including range aliases.
 * The browser applies the final maximum scroll clamp after updating its extent.
 */
export function restoreGitAnchor(
  anchor: GitViewportAnchor | null,
  bands: readonly GitViewportBand[],
  zoom: number,
): Scroll | null {
  if (!anchor || !Number.isFinite(zoom) || zoom <= 0) return null;
  const band = bands.find((candidate) => candidate.projectId === anchor.projectId);
  const mark = band?.layout?.byOid.get(anchor.oid);
  if (!band || !mark) return null;
  const left = mark.x * zoom - anchor.screenX;
  const top = band.top + GIT_CANVAS_HEADER + mark.y * zoom - anchor.screenY;
  return Number.isFinite(left) && Number.isFinite(top)
    ? { left: Math.max(0, left), top: Math.max(0, top) }
    : null;
}

/** Follow the observed frontier in one repository's independent coordinates.
 * Prefer marks on the current visible lanes, so a wider offscreen project or
 * lane cannot pull the viewport into empty canvas.
 */
export function gitFrontierScroll({
  bands,
  zoom,
  scroll,
  viewport,
  projectId,
}: {
  bands: readonly GitViewportBand[];
  zoom: number;
  scroll: Scroll;
  viewport: Viewport;
  projectId?: string;
}): Scroll | null {
  if (!Number.isFinite(zoom) || zoom <= 0 ||
    !Number.isFinite(viewport.width) || !Number.isFinite(viewport.height) ||
    viewport.width <= 0 || viewport.height <= 0 ||
    !Number.isFinite(scroll.left) || !Number.isFinite(scroll.top)) return null;
  const populated = bands.filter((band) => band.layout?.marks.length);
  const centerY = scroll.top + viewport.height / 2;
  const distanceFromCenter = (candidate: GitViewportBand) => Math.max(candidate.top - centerY, centerY - candidate.top - candidate.height, 0);
  const band = projectId !== undefined
    ? populated.find((candidate) => candidate.projectId === projectId)
    : populated.find((candidate) => candidate.top <= centerY && candidate.top + candidate.height > centerY) ??
      populated.find((candidate) => candidate.top < scroll.top + viewport.height && candidate.top + candidate.height > scroll.top) ??
      populated.reduce<GitViewportBand | undefined>((nearest, candidate) =>
        !nearest || distanceFromCenter(candidate) < distanceFromCenter(nearest) ? candidate : nearest, undefined);
  if (!band?.layout) return null;
  const marks = band.layout.marks.filter((mark) => Number.isFinite(mark.x) && Number.isFinite(mark.y));
  const worldY = (mark: { y: number }) => band.top + GIT_CANVAS_HEADER + mark.y * zoom;
  const visible = marks.filter((mark) => worldY(mark) >= scroll.top && worldY(mark) <= scroll.top + viewport.height);
  const candidates = visible.length ? visible : marks;
  const frontier = candidates.reduce<(typeof candidates)[number] | null>(
    (rightmost, mark) => !rightmost || mark.x > rightmost.x ? mark : rightmost, null,
  );
  if (!frontier) return null;
  // At the repository's final mark, include its real trailing layout padding.
  // Intermediate visible lanes retain their own context instead of jumping to
  // the end of a wider offscreen lane or another project's global extent.
  const atRightEdge = marks.every((mark) => mark.x <= frontier.x);
  const right = atRightEdge ? band.layout.width * zoom : frontier.x * zoom + 96;
  const left = Math.max(0, right - viewport.width);
  const top = visible.length
    ? Math.max(0, scroll.top)
    : Math.max(0, band.top, worldY(frontier) - viewport.height / 2);
  return Number.isFinite(left) && Number.isFinite(top) ? { left, top } : null;
}

/** Find the largest fitting scale using rendered extents at each candidate zoom.
 * Layout keeps labels/ranges readable at small scales, so dimensions do not scale
 * linearly. Fixed extents can make fitting impossible; then retain the minimum.
 */
export function fitGitZoom(
  measure: (zoom: number) => Viewport,
  viewport: Viewport,
  minZoom = 0.05,
  maxZoom = 1,
): number {
  const minimum = Number.isFinite(minZoom) && minZoom > 0 ? minZoom : 0.05;
  const maximum = Math.max(minimum, Number.isFinite(maxZoom) && maxZoom > 0 ? maxZoom : 1);
  if (!Number.isFinite(viewport.width) || !Number.isFinite(viewport.height) ||
    viewport.width <= 0 || viewport.height <= 0) return minimum;
  const fits = (zoom: number) => {
    const measured = measure(zoom);
    return Number.isFinite(measured.width) && Number.isFinite(measured.height) &&
      measured.width >= 0 && measured.height >= 0 &&
      measured.width <= viewport.width && measured.height <= viewport.height;
  };
  if (fits(maximum)) return maximum;
  if (!fits(minimum)) return minimum;
  let low = minimum, high = maximum;
  for (let iteration = 0; iteration < 32; iteration++) {
    const midpoint = (low + high) / 2;
    if (fits(midpoint)) low = midpoint;
    else high = midpoint;
  }
  return low;
}
