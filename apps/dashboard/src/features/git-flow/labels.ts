export type GitLabelCandidate = {
  id: string;
  x: number;
  y: number;
  names: string[];
  color: string;
  priority: number;
};

export type GitPlacedLabel = {
  id: string;
  x: number;
  y: number;
  width: number;
  height: 20;
  text: string;
  fullText: string;
  color: string;
  refCount: number;
};

export type GitLabelObstacle = { x: number; y: number; width: number; height: number };

const HEIGHT = 20;
const HORIZONTAL_PADDING = 16;
const GAP = 4;
const CELL_WIDTH = 48;
const CELL_HEIGHT = 24;
const compare = (a: string, b: string) => a < b ? -1 : a > b ? 1 : 0;
const graphemes = new Intl.Segmenter("en", { granularity: "grapheme" });

/** Truncate the first observed ref, retaining both ends whenever they can fit.
 * Measure the whole display string so kerning and the +N group suffix count.
 */
function fitText(first: string, suffix: string, available: number, measure: (text: string) => number) {
  const width = (text: string) => {
    try {
      const measured = measure(text);
      return Number.isFinite(measured) && measured >= 0 ? measured : Infinity;
    } catch { return Infinity; }
  };
  const full = first + suffix, fullWidth = width(full);
  if (fullWidth <= available) return { text: full, width: fullWidth };
  const parts = Array.from(graphemes.segment(first), (part) => part.segment);
  const shorten = (kept: number) => {
    const prefix = Math.ceil(kept / 2), tail = Math.floor(kept / 2);
    return parts.slice(0, prefix).join("") + "…" + (tail ? parts.slice(parts.length - tail).join("") : "") + suffix;
  };
  if (width(shorten(0)) > available) return null;
  let low = 0, high = Math.max(0, parts.length - 1);
  while (low < high) {
    const middle = Math.ceil((low + high) / 2);
    if (width(shorten(middle)) <= available) low = middle;
    else high = middle - 1;
  }
  // The final measurement is authoritative even for unusual font metrics.
  while (low >= 0) {
    const text = shorten(low), measured = width(text);
    if (measured <= available) return { text, width: measured };
    low--;
  }
  return null;
}

/** Screen-space label placement only: this never changes graph objects or
 * relationship evidence. Hidden references remain exactly countable.
 */
export function placeGitLabels(
  candidates: readonly GitLabelCandidate[],
  viewport: { width: number; height: number },
  padding: number,
  measure: (text: string) => number,
  limit = 200,
  obstacles: readonly GitLabelObstacle[] = [],
): { labels: GitPlacedLabel[]; hiddenRefCount: number } {
  const labels: GitPlacedLabel[] = [];
  let totalRefs = 0;
  for (const candidate of candidates) if (Array.isArray(candidate?.names)) totalRefs += candidate.names.length;
  const result = () => ({ labels, hiddenRefCount: totalRefs - labels.reduce((count, label) => count + label.refCount, 0) });
  const inset = Number.isFinite(padding) ? Math.max(0, padding) : 0;
  const available = Math.min(270, viewport.width - inset * 2);
  const maximum = Math.min(200, Math.max(0, Number.isFinite(limit) ? Math.floor(limit) : 200));
  if (!Number.isFinite(viewport.width) || !Number.isFinite(viewport.height) || available <= HORIZONTAL_PADDING || viewport.height - inset * 2 < HEIGHT || !maximum) return result();
  const valid = candidates.filter((candidate) => candidate && typeof candidate.id === "string" && candidate.id.length > 0 &&
    Number.isFinite(candidate.x) && Number.isFinite(candidate.y) && Number.isFinite(candidate.priority) &&
    Array.isArray(candidate.names) && candidate.names.length > 0 && candidate.names.every((name) => typeof name === "string" && name.trim().length > 0))
    .sort((a, b) => b.priority - a.priority || a.x - b.x || a.y - b.y || compare(a.id, b.id) || compare(a.names.join("\0"), b.names.join("\0")));
  const grid = new Map<string, GitLabelObstacle[]>();
  const cells = (x: number, y: number, width: number, height: number, visit: (key: string) => void) => {
    const left = Math.floor(x / CELL_WIDTH), right = Math.floor((x + width) / CELL_WIDTH);
    const top = Math.floor(y / CELL_HEIGHT), bottom = Math.floor((y + height) / CELL_HEIGHT);
    for (let row = top; row <= bottom; row++) for (let column = left; column <= right; column++) visit(`${column}:${row}`);
  };
  const insert = (rectangle: GitLabelObstacle) => cells(rectangle.x, rectangle.y, rectangle.width, rectangle.height, (key) => {
    const values = grid.get(key) ?? [];
    values.push(rectangle); grid.set(key, values);
  });
  // A range control paints above the canvas. Reserve its actual screen box
  // before placing labels, using the same geometry as the DOM control.
  for (const obstacle of obstacles) {
    if (![obstacle.x, obstacle.y, obstacle.width, obstacle.height].every(Number.isFinite) || obstacle.width <= 0 || obstacle.height <= 0) continue;
    const x = Math.max(0, obstacle.x), y = Math.max(0, obstacle.y);
    const width = Math.min(viewport.width, obstacle.x + obstacle.width) - x;
    const height = Math.min(viewport.height, obstacle.y + obstacle.height) - y;
    if (width > 0 && height > 0) insert({ x, y, width, height });
  }
  const collides = (x: number, y: number, width: number) => {
    const checked = new Set<GitLabelObstacle>();
    let overlap = false;
    cells(x - GAP, y - GAP, width + GAP * 2, HEIGHT + GAP * 2, (key) => {
      for (const label of grid.get(key) ?? []) {
        if (overlap || checked.has(label)) continue;
        checked.add(label);
        if (x < label.x + label.width + GAP && x + width + GAP > label.x && y < label.y + label.height + GAP && y + HEIGHT + GAP > label.y) overlap = true;
      }
    });
    return overlap;
  };
  const usedIds = new Set<string>();
  for (const candidate of valid) {
    if (labels.length >= maximum) break;
    if (usedIds.has(candidate.id)) continue;
    const suffix = candidate.names.length > 1 ? ` +${candidate.names.length - 1}` : "";
    const fitted = fitText(candidate.names[0], suffix, available - HORIZONTAL_PADDING, measure);
    if (!fitted) continue;
    const width = fitted.width + HORIZONTAL_PADDING;
    const alternatives = [
      [candidate.x - 8, candidate.y - 30],
      [candidate.x - 8, candidate.y + 14],
      [candidate.x - width - 12, candidate.y - HEIGHT / 2],
      [candidate.x + 12, candidate.y - HEIGHT / 2],
      [candidate.x - width / 2, candidate.y - 54],
      [candidate.x - width / 2, candidate.y + 38],
    ];
    const tried = new Set<string>();
    for (const [proposedX, proposedY] of alternatives) {
      const x = Math.max(inset, Math.min(viewport.width - inset - width, proposedX));
      const y = Math.max(inset, Math.min(viewport.height - inset - HEIGHT, proposedY));
      const location = `${x}:${y}`;
      if (tried.has(location)) continue;
      tried.add(location);
      if (collides(x, y, width)) continue;
      const label: GitPlacedLabel = { id: candidate.id, x, y, width, height: HEIGHT, text: fitted.text,
        fullText: candidate.names.join(" · "), color: candidate.color, refCount: candidate.names.length };
      labels.push(label); usedIds.add(candidate.id);
      insert(label);
      break;
    }
  }
  return result();
}
