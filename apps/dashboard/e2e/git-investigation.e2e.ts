import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { GitCommit, GitRepository } from "../src/features/git-flow/model";
import {
  launchBrowserHarness,
  REPO_ROOT,
  startIsolatedNativeAdmiraltyServer,
  type BrowserHarness,
  type IsolatedNativeAdmiraltyServer,
} from "./browser-harness";

const oid = (n: number) => (Math.imul(n, 2654435761) >>> 0).toString(16).padStart(8, "0").padEnd(40, "0");
function sourceFixture() {
  const stamp = new Date().toISOString();
  const commit = (n: number, parents: number[], subject: string): GitCommit => ({
    oid: oid(n), parents: parents.map(oid), subject, committed_utc: stamp,
  });
  const ref = (name: string, tip: number) => ({ name, oid: oid(tip), commit_oid: oid(tip), kind: "branch" });
  const harbor: GitRepository = {
    instance_id: "investigation-harbor", node: "headless-fixture", observed_utc: stamp,
    shallow: false, truncated: false,
    commits: Array.from({ length: 80 }, (_, n) => commit(n + 1, n ? [n] : [], `Main loaded commit ${n + 1}`)),
    refs: [ref("refs/heads/main", 80), ref("refs/heads/work/unchecked", 108), ref("refs/remotes/origin/main", 204)],
    checkouts: [
      { id: "harbor-main", name: "local-main", head: oid(80), branch: "main", kind: "primary", detached: false, locked: false, prunable: false },
      { id: "harbor-newer", name: "newer-detached-tree", head: oid(204), kind: "linked", detached: true, locked: false, prunable: false, status: { staged: 0, unstaged: 1, untracked: 0, conflicts: 1 } },
    ],
  };
  for (let n = 101; n <= 108; n++) harbor.commits.push(commit(n, [n === 101 ? 10 : n - 1], `Unchecked topic ${n}`));
  for (let n = 201; n <= 204; n++) harbor.commits.push(commit(n, [n === 201 ? 5 : n - 1], `Newer checkout ${n}`));
  // Deliberately reverse observations: input order cannot choose the backbone.
  harbor.commits.reverse(); harbor.refs.reverse();
  const observatory = structuredClone(harbor);
  observatory.instance_id = "investigation-observatory";
  observatory.checkouts = [];
  return {
    generated_utc: stamp, host: "headless-fixture", simulated: true,
    order: ["harbor", "observatory"], projects: { harbor: { name: "Harbor" }, observatory: { name: "Observatory" } },
    hierarchy: { captains: [] }, agents: [], git: { harbor, observatory },
  };
}

type CameraState = {
  x: number; y: number; k: number; left: number; top: number; width: number; height: number;
  intent: string; selection: string | null;
};

describe("repository Git investigation against isolated native source", () => {
  let browser: BrowserHarness, server: IsolatedNativeAdmiraltyServer, source: ReturnType<typeof sourceFixture>;
  const evidence = join(REPO_ROOT, "target", "git-investigation-evidence");
  const observations: unknown[] = [];
  beforeAll(async () => {
    mkdirSync(evidence, { recursive: true });
    server = await startIsolatedNativeAdmiraltyServer({ legacySnapshot: sourceFixture(), allowedProjects: ["harbor", "observatory"] });
    browser = await launchBrowserHarness({ desktopInput: true });
  });
  afterAll(async () => {
    writeFileSync(join(evidence, "headless-fixture-observations.json"), JSON.stringify({
      source: "Explicitly simulated, isolated headless test fixture; not user Chrome or a live deployment", observations,
    }, null, 2));
    await browser?.close(); await server?.close();
  });
  beforeEach(async () => {
    source = sourceFixture(); server.writeLegacySnapshot(source);
    await browser.resetPage(); await browser.setViewportSize(1440, 1000);
  });

  async function settle() {
    await browser.evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
  }
  async function waitFor(predicate: string, description: string) {
    await browser.evaluate(`new Promise((resolve, reject) => {
      const deadline = performance.now() + 5000;
      function check() { if (${predicate}) return resolve(true); if (performance.now() >= deadline) return reject(Error(${JSON.stringify(description)})); requestAnimationFrame(check); }
      check();
    })`);
  }
  async function admitCurrentPage() {
    await browser.waitForSelector(".lv-feed.is-connected, .lv-error button");
    if (!await browser.evaluate(`Boolean(document.querySelector('.lv-feed.is-connected'))`)) {
      await browser.evaluate(`document.querySelector('.lv-error button').click()`);
      await browser.fillInputByLabel("Bearer token", server.ownerToken);
      await browser.clickButtonByText("Apply token");
    }
    await browser.waitForSelector(".lv-feed.is-connected");
  }
  async function connect() {
    await browser.goto(`${server.baseUrl}/#/git`); await admitCurrentPage();
    await browser.waitForSelector(".git-band");
  }
  async function selectByAriaLabel(label: string, value: string) {
    await browser.evaluate(`(() => {
      const select = [...document.querySelectorAll('select[aria-label]')].find(element => element.getAttribute('aria-label') === ${JSON.stringify(label)});
      if (!(select instanceof HTMLSelectElement) || select.disabled) throw Error('Available select not found: ' + ${JSON.stringify(label)});
      const option = [...select.options].find(candidate => candidate.value === ${JSON.stringify(value)});
      if (!option || option.disabled) throw Error('Available option not found: ' + ${JSON.stringify(value)});
      const setter = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value')?.set;
      if (!setter) throw Error('Native select value setter unavailable');
      setter.call(select, ${JSON.stringify(value)});
      if (select.value !== ${JSON.stringify(value)}) throw Error('Select value was not applied');
      select.dispatchEvent(new Event('change', { bubbles: true }));
    })()`);
    await settle();
  }
  async function expand(project = "harbor") {
    await selectByAriaLabel("Filter Git project", project);
    await browser.waitForSelector(".git-view.is-project-focused");
    await browser.clickButtonByText("Expand project");
    await browser.waitForSelector(`.git-project-viewport[data-project-id="${project}"] .git-project-bounds`);
    await settle();
  }
  async function camera(): Promise<CameraState> {
    await settle();
    return browser.evaluate(`(() => {
      const e = document.querySelector('.git-project-stage'), r = e.getBoundingClientRect();
      return { x: Number(e.dataset.cameraX), y: Number(e.dataset.cameraY), k: Number(e.dataset.cameraK), left: r.left + e.clientLeft, top: r.top + e.clientTop,
        width: e.clientWidth, height: e.clientHeight, intent: e.closest('.git-project-viewport').dataset.cameraIntent, selection: e.dataset.selectionKey ?? null };
    })()`);
  }
  async function fitBounds(label: string) {
    await browser.clickButtonByAriaLabel("Fit graph"); await settle();
    const result = await browser.evaluate<{ margins: number[]; padding: number; width: number; height: number; overflow: boolean }>(`(() => {
      const e = document.querySelector('.git-project-stage'), v = e.getBoundingClientRect(), r = document.querySelector('.git-project-bounds').getBoundingClientRect();
      const left = v.left + e.clientLeft, top = v.top + e.clientTop;
      return { margins: [r.left - left, left + e.clientWidth - r.right, r.top - top, top + e.clientHeight - r.bottom],
        padding: Math.min(e.clientWidth >= 640 ? 48 : 16, e.clientWidth / 4, e.clientHeight / 4), width: e.clientWidth, height: e.clientHeight,
        overflow: e.scrollWidth > e.clientWidth + 1 || e.scrollHeight > e.clientHeight + 1 };
    })()`);
    observations.push({ label, utc: new Date().toISOString(), ...result });
    if (!result.width || !result.height) {
      observations.push(await browser.evaluate(`[...document.querySelectorAll('.live-workspace,.lv-header,.lv-body,.git-view,.lv-toolbar,.git-ruler,.git-investigation-heading,.git-project-viewport,.git-project-camera-controls,.git-investigation-body,.git-project-main,.git-project-stage,.git-project-chat-dock,.git-project-tools,.git-branch-evidence,.lv-inspector')].map(e=>({className:e.className,rect:e.getBoundingClientRect().toJSON(),flex:getComputedStyle(e).flex}))`));
      await browser.captureScreenshot(join(evidence, 'FAILED-zero-stage.png'));
    }
    expect(result.width).toBeGreaterThan(0); expect(result.height).toBeGreaterThan(0);
    for (const margin of result.margins) expect(margin, `${label}: ${JSON.stringify(result)}`).toBeGreaterThanOrEqual(result.padding - 1);
    expect(result.overflow).toBe(false);
    return result;
  }
  async function labelsClearOfRangeControls(label: string) {
    await settle();
    const result = await browser.evaluate<{ labels: number; controls: number; overlaps: unknown[] }>(`(() => {
      const canvas = document.querySelector('.git-project-stage .git-canvas'), bounds = canvas.getBoundingClientRect();
      if (!canvas.dataset.refLabelBoxes) throw Error('Painted ref label geometry unavailable');
      const labels = JSON.parse(canvas.dataset.refLabelBoxes);
      const controls = [...document.querySelectorAll('.git-project-stage .git-commit-range')].map(button => {
        const rect = button.getBoundingClientRect(); return {x:rect.left-bounds.left,y:rect.top-bounds.top,width:rect.width,height:rect.height};
      });
      const overlaps = [];
      for (const text of labels) for (const control of controls)
        if (text.x < control.x+control.width && text.x+text.width > control.x && text.y < control.y+control.height && text.y+text.height > control.y) overlaps.push({text,control});
      return {labels:labels.length,controls:controls.length,overlaps};
    })()`);
    observations.push({ label, ...result });
    expect(result.labels, label).toBeGreaterThan(0); expect(result.controls, label).toBeGreaterThan(0);
    expect(result.overlaps, label).toEqual([]);
  }
  async function realClick(selector: string) {
    const point = await browser.evaluate<{ x: number; y: number; hit: boolean }>(`(() => {
      const b = document.querySelector(${JSON.stringify(selector)}); if (!b) throw Error('Missing click target');
      const r = b.getBoundingClientRect(), x = r.left + r.width / 2, y = r.top + r.height / 2;
      return { x, y, hit: r.width > 0 && r.height > 0 && (document.elementFromPoint(x, y) === b || b.contains(document.elementFromPoint(x, y))) };
    })()`);
    if (!point.hit) {
      observations.push({ label: 'FAILED real click', selector, point, layout: await browser.evaluate(`({elements:[...document.querySelectorAll('.live-workspace,.lv-body,.git-view,.git-investigation-heading,.git-project-viewport,.git-project-camera-controls,.git-investigation-body,.git-project-main,.git-project-stage,.git-project-chat-dock,.git-chat,.git-chat-toggle,.git-project-tools,.git-branch-evidence,.lv-inspector')].map(e=>({className:e.className,rect:e.getBoundingClientRect().toJSON(),flex:getComputedStyle(e).flex})),hits:document.elementsFromPoint(${point.x},${point.y}).map(e=>e.tagName+'.'+e.className)})`) });
      await browser.captureScreenshot(join(evidence, 'FAILED-click.png'));
    }
    expect(point.hit, `Unobstructed native click: ${selector}`).toBe(true);
    await browser.dragPointer(point, point); await settle();
  }
  async function canvasPoint() {
    return browser.evaluate<{ x: number; y: number }>(`(() => {
      const r = document.querySelector('.git-project-stage').getBoundingClientRect();
      for (const dy of [.2,.4,.7]) for (const dx of [.2,.4,.6]) { const x = r.left + r.width * dx, y = r.top + r.height * dy; if (document.elementFromPoint(x,y)?.matches('.git-canvas')) return {x,y}; }
      throw Error('No unobstructed canvas input target');
    })()`);
  }
  const distance = (a: { x: number; y: number }, b: { x: number; y: number }) => Math.hypot(a.x - b.x, a.y - b.y);

  it("expands one chosen repository with padded fit when native fullscreen is refused", async () => {
    await connect();
    expect(await browser.evaluate(`document.querySelectorAll('.git-project-viewport').length`)).toBe(0);
    await browser.evaluate(`(() => { window.__gitFullscreenRequests = 0; HTMLElement.prototype.requestFullscreen = function() { window.__gitFullscreenRequests++; return Promise.reject(new DOMException('Headless test refusal', 'NotAllowedError')); }; })()`);
    await expand();
    expect(await browser.evaluate(`document.querySelectorAll('.git-project-viewport').length`)).toBe(1);
    expect(await browser.evaluate(`Boolean(document.querySelector('.live-workspace.is-fullscreen'))`)).toBe(true);
    expect(await browser.evaluate(`document.fullscreenElement === null && window.__gitFullscreenRequests > 0`)).toBe(true);
    expect(await browser.evaluate(`new URLSearchParams(location.hash.split('?')[1]).get('scope')`)).toBe("harbor");
    await browser.waitForText("Display default: local main");
    await fitBounds("desktop selected repository");
    await selectByAriaLabel("Inspect branch or ref", "refs/heads/main");
    await browser.waitForSelector(".lv-inspector");
    expect(await browser.evaluate(`document.querySelector('.lv-inspector').textContent`)).toContain(oid(80));
    expect(await browser.evaluate(`document.querySelector('.git-branch-evidence').textContent`)).toContain("refs/heads/main");
    await browser.captureScreenshot(join(evidence, "selected-repository-fullscreen-fallback.png"));
    expect(await browser.evaluate(`document.querySelector('.sidebar').inert`)).toBe(true);
    await browser.pressKey('Escape'); await settle();
    expect(await browser.evaluate(`document.querySelector('.sidebar').inert`)).toBe(false);

  });

  it("supports native wheel pan, pointer-anchored modifier zoom, held-button drag and guarded shortcuts", async () => {
    await connect(); await expand();
    await realClick('button[aria-label="Reset project zoom to 100%"]');
    let before = await camera(); const point = await canvasPoint();
    await browser.wheelPointer(point, { x: 60, y: 30 });
    await waitFor(`Number(document.querySelector('.git-project-stage').dataset.cameraX) !== ${before.x}`, "plain wheel pan");
    let after = await camera();
    expect(after.k).toBe(before.k); expect(after.x).toBeCloseTo(before.x - 60, 1); expect(after.y).toBeCloseTo(before.y - 30, 1);
    for (const modifiers of [{ control: true }, { meta: true }]) {
      before = await camera();
      const anchor = { x: (point.x - before.left - before.x) / before.k, y: (point.y - before.top - before.y) / before.k };
      await browser.wheelPointer(point, { x: 0, y: -80 }, modifiers);
      await waitFor(`Number(document.querySelector('.git-project-stage').dataset.cameraK) !== ${before.k}`, "modifier wheel zoom");
      after = await camera(); expect(after.k).toBeGreaterThan(before.k);
      expect(distance({ x: after.left + after.x + anchor.x * after.k, y: after.top + after.y + anchor.y * after.k }, point)).toBeLessThanOrEqual(1);
    }
    before = await camera(); const start = await canvasPoint();
    await browser.dragPointer(start, { x: start.x + 76, y: start.y + 42 }); after = await camera();
    expect(after.x).toBeCloseTo(before.x + 76, 1); expect(after.y).toBeCloseTo(before.y + 42, 1); expect(after.k).toBe(before.k);
    expect(after.selection).toBeNull(); expect(await browser.evaluate(`Boolean(document.querySelector('.lv-inspector'))`)).toBe(false);
    await browser.evaluate(`document.querySelector('.git-project-stage').focus()`); await browser.pressKey("0"); await settle();
    expect((await camera()).intent).toBe("fit");
    await browser.pressKey("1"); await settle(); expect((await camera()).k).toBe(1);
    await browser.pressKey("f"); await settle(); expect((await camera()).intent).toBe("fit");
    await browser.pressKey("1");
    await realClick('.git-project-chat-dock .git-chat-toggle');
    await browser.waitForSelector('.git-project-chat-dock textarea');
    before = await camera();
    expect(await browser.evaluate(`(() => {
      const composer = document.querySelector('.git-project-chat-dock textarea');
      if (!(composer instanceof HTMLTextAreaElement)) throw Error('Chat composer unavailable');
      composer.focus(); return document.activeElement === composer;
    })()`)).toBe(true);
    for (const key of ["0", "f", "1"]) await browser.pressKey(key);
    after = await camera(); expect({ x: after.x, y: after.y, k: after.k, intent: after.intent }).toEqual({ x: before.x, y: before.y, k: before.k, intent: before.intent });
    observations.push({ label: "native pan/modifier zoom/held-button drag and focus guards", after });
  });

  it("keeps the exact range anchor fixed during expansion and fits all expanded loaded history", async () => {
    await connect(); await expand(); await fitBounds("before range expansion");
    const collapsed = await browser.evaluate<{ oid: string; x: number; y: number; width: number }>(`(() => {
      const buttons = [...document.querySelectorAll('.git-commit-range[aria-expanded="false"][data-anchor-oid]')];
      buttons.sort((a,b) => Number(b.title.match(/· (\\d+) linear/)?.[1] ?? 0) - Number(a.title.match(/· (\\d+) linear/)?.[1] ?? 0));
      const button = buttons[0], e = document.querySelector('.git-project-stage'), r = e.getBoundingClientRect(); if (!button) throw Error('No visible collapsed range');
      return { oid: button.dataset.anchorOid, x: r.left + e.clientLeft + Number(e.dataset.cameraX) + Number(button.dataset.worldX) * Number(e.dataset.cameraK),
        y: r.top + e.clientTop + Number(e.dataset.cameraY) + Number(button.dataset.worldY) * Number(e.dataset.cameraK), width: document.querySelector('.git-project-bounds').offsetWidth };
    })()`);
    const selector = `.git-commit-range[data-anchor-oid="${collapsed.oid}"]`;
    await realClick(selector);
    await browser.waitForSelector(`${selector}[aria-expanded="true"]`);
    await settle();
    const expanded = await browser.evaluate<{ x: number; y: number; width: number }>(`(() => {
      const b = document.querySelector(${JSON.stringify(selector)}), e = document.querySelector('.git-project-stage'), r = e.getBoundingClientRect();
      return { x: r.left + e.clientLeft + Number(e.dataset.cameraX) + Number(b.dataset.worldX) * Number(e.dataset.cameraK),
        y: r.top + e.clientTop + Number(e.dataset.cameraY) + Number(b.dataset.worldY) * Number(e.dataset.cameraK), width: document.querySelector('.git-project-bounds').offsetWidth };
    })()`);
    expect(distance(collapsed, expanded)).toBeLessThanOrEqual(2);
    expect(expanded.width).toBeGreaterThan(collapsed.width * 2);
    await fitBounds("after range expansion");
    observations.push({ label: "exact range first-OID screen anchor", collapsed, expanded });
    await browser.captureScreenshot(join(evidence, "expanded-loaded-history-fit.png"));
  });

  it("inspects a ref without a checkout and keeps mobile chat, inspector and camera controls separate", async () => {
    await connect(); await expand();
    expect(source.git.harbor.checkouts.some((checkout) => checkout.head === oid(108))).toBe(false);
    await selectByAriaLabel("Inspect branch or ref", "refs/heads/work/unchecked");
    await browser.waitForSelector(".lv-inspector");
    expect(await browser.evaluate(`document.querySelector('.lv-inspector').textContent`)).toContain(oid(108));
    expect(await browser.evaluate(`new URLSearchParams(location.hash.split('?')[1]).get('focus')`)).toBe(oid(108));
    await realClick('.git-investigation-heading button[aria-pressed="false"]');
    expect(await browser.evaluate(`document.querySelector('.git-investigation-heading button[aria-pressed]').getAttribute('aria-pressed')`)).toBe("true");
    expect(await browser.evaluate(`document.querySelector('.git-branch-evidence').textContent`)).toContain("Integration status requires separate source-bound evidence");
    await browser.setViewportSize(390, 1000); await fitBounds("mobile inspector closed chat");
    for (const open of [true, false]) {
      await realClick('.git-project-chat-dock .git-chat-toggle');
      expect(await browser.evaluate(`document.querySelector('.git-project-chat-dock .git-chat-toggle').getAttribute('aria-expanded')`)).toBe(String(open));
      for (const label of ["Zoom project in", "Zoom project out", "Fit graph", "Reset project zoom to 100%"]) await realClick(`button[aria-label="${label}"]`);
      const fitted = await fitBounds(`mobile inspector chat ${open ? "open" : "closed"}`);
      expect(fitted.padding).toBe(16);
      expect(fitted.height).toBeGreaterThanOrEqual(160);
      expect(await browser.evaluate(`(() => {
        const a = document.querySelector('.git-project-stage').getBoundingClientRect(), b = document.querySelector('.git-project-chat-dock').getBoundingClientRect(), i = document.querySelector('.lv-inspector').getBoundingClientRect();
        const overlap = Math.min(a.right,b.right)-Math.max(a.left,b.left) > 1 && Math.min(a.bottom,b.bottom)-Math.max(a.top,b.top) > 1;
        return !overlap && a.bottom <= i.top + 1;
      })()`)).toBe(true);
      await browser.captureScreenshot(join(evidence, `mobile-ref-inspector-chat-${open ? "open" : "closed"}.png`));
    }
  });

  it("restores per-repository manual cameras on history return and invalidates a replaced instance", async () => {
    await connect(); await expand(); await realClick('button[aria-label="Reset project zoom to 100%"]');
    const initial = await camera();
    await browser.wheelPointer(await canvasPoint(), { x: 91, y: 43 });
    await waitFor(`Number(document.querySelector('.git-project-stage').dataset.cameraX) !== ${initial.x}`, "Harbor native wheel pan");
    const harbor = await camera();
    await browser.clickButtonByText("All projects"); await browser.waitForSelector(".git-band");
    await browser.evaluate(`history.back()`); await browser.waitForSelector('.git-project-viewport[data-project-id="harbor"]');
    let restored = await camera(); expect({ x: restored.x, y: restored.y, k: restored.k }).toEqual({ x: harbor.x, y: harbor.y, k: harbor.k });
    await browser.clickButtonByText("All projects"); await browser.waitForSelector(".git-band"); await expand("observatory");
    await realClick('button[aria-label="Reset project zoom to 100%"]'); const observatoryInitial = await camera();
    await browser.wheelPointer(await canvasPoint(), { x: -61, y: -37 });
    await waitFor(`Number(document.querySelector('.git-project-stage').dataset.cameraX) !== ${observatoryInitial.x}`, "Observatory native wheel pan");
    const observatory = await camera(); expect(observatory.x).not.toBe(harbor.x);
    await browser.clickButtonByText("All projects"); await browser.waitForSelector(".git-band"); await expand("harbor");
    restored = await camera(); expect({ x: restored.x, y: restored.y, k: restored.k }).toEqual({ x: harbor.x, y: harbor.y, k: harbor.k });
    const replacement = structuredClone(source), stamp = new Date().toISOString();
    replacement.generated_utc = stamp; replacement.git.harbor.observed_utc = stamp; replacement.git.harbor.instance_id = "investigation-harbor-replaced";
    replacement.projects.harbor.name = "Harbor replacement observation"; server.writeLegacySnapshot(replacement);
    await browser.waitForText("Harbor replacement observation"); await browser.waitForSelector('.git-project-viewport[data-fitted="true"]');
    expect((await camera()).intent).toBe("fit"); await fitBounds("replacement repository instance");
    observations.push({ label: "per-repository return and instance invalidation", harbor, observatory, restored });
  });

  it("clears prior camera state when session access is removed and admitted again", async () => {
    await connect(); await expand(); await realClick('button[aria-label="Reset project zoom to 100%"]');
    const initial = await camera();
    await browser.wheelPointer(await canvasPoint(), { x: 77, y: 33 });
    await waitFor(`Number(document.querySelector('.git-project-stage').dataset.cameraX) !== ${initial.x}`, "camera before access removal");
    expect((await camera()).intent).toBe("manual");
    await realClick('button[aria-label="Exit fullscreen"]');
    await realClick('button[title="Your profile"]'); await browser.waitForSelector(".profile-dialog[open]");
    await realClick(".profile-access-row button"); await browser.waitForSelector(".access-modal");
    await realClick(".access-modal .modal-actions .secondary-button"); await browser.waitForSelector(".lv-error button");
    expect(await browser.evaluate(`Boolean(document.querySelector('.git-project-stage'))`)).toBe(false);
    await admitCurrentPage(); await browser.waitForSelector('.git-project-viewport[data-fitted="true"]');
    expect((await camera()).intent).toBe("fit"); await fitBounds("session re-admission");
  });

  it("keeps a missing explicit display backbone selected instead of silently adopting main", async () => {
    await connect(); await expand();
    await realClick('.git-display-options summary');
    await selectByAriaLabel('Display backbone', 'refs/heads/work/unchecked');
    await browser.waitForSelector('.git-project-stage .git-canvas[data-backbone-ref="refs/heads/work/unchecked"][data-backbone-oid]');
    expect(await browser.evaluate(`Number(document.querySelector('.git-project-stage .git-canvas').dataset.backboneY)`)).toBe(0);
    const replacement = structuredClone(source), stamp = new Date().toISOString();
    replacement.generated_utc = stamp; replacement.git.harbor.observed_utc = stamp;
    replacement.git.harbor.refs = replacement.git.harbor.refs.filter((ref) => ref.name !== 'refs/heads/work/unchecked');
    replacement.projects.harbor.name = 'Harbor missing selected ref'; server.writeLegacySnapshot(replacement);
    await browser.waitForText('Harbor missing selected ref');
    await browser.waitForSelector('.git-project-stage .git-canvas[data-backbone-ref="refs/heads/work/unchecked"]:not([data-backbone-oid])');
    expect(await browser.evaluate(`document.querySelector('select[aria-label="Display backbone"]').value`)).toBe('refs/heads/work/unchecked');
    await browser.waitForText('no longer observed');
    expect(await browser.evaluate(`document.querySelector('.git-graph-status').textContent`)).toContain('Selected display ref refs/heads/work/unchecked is missing');
  });

  it("shows a topic leaving straight main and returning at its actual two-parent merge", async () => {
    const repo = source.git.harbor, template = repo.commits[0];
    for (let n = 301; n <= 304; n++) repo.commits.push({ ...template, oid: oid(n), parents: [oid(n === 301 ? 20 : n - 1)], subject: `Merged topic ${n}` });
    repo.commits.find((commit) => commit.oid === oid(60))!.parents = [oid(59), oid(304)];
    repo.commits.find((commit) => commit.oid === oid(60))!.subject = 'Merge feature/merged at actual main junction';
    repo.refs.push({ name: 'refs/heads/feature/merged', oid: oid(304), commit_oid: oid(304), kind: 'branch' });
    server.writeLegacySnapshot(source); await connect(); await expand();
    await browser.waitForSelector('.git-project-stage .git-canvas[data-backbone-return-count="1"]');
    expect(await browser.evaluate(`Number(document.querySelector('.git-project-stage .git-canvas').dataset.backboneY)`)).toBe(0);
    await realClick('.git-display-options summary');
    await browser.clickButtonByText('Merge feature/merged at actual main junction');
    await browser.waitForSelector('.git-merge-evidence');
    const evidenceText = await browser.evaluate<string>(`document.querySelector('.git-merge-evidence').textContent`);
    expect(evidenceText).toContain('Actual Git merge · 2 parents');
    expect(evidenceText).toContain(oid(59)); expect(evidenceText).toContain(oid(304));
    expect(evidenceText).toContain('refs/heads/feature/merged');
    expect(evidenceText).toContain('First parent'); expect(evidenceText).toContain('incoming history');
    await fitBounds('actual fork and merge on selected main');
    await labelsClearOfRangeControls('fitted merge view labels avoid actual DOM controls');
    await browser.captureScreenshot(join(evidence, 'actual-main-fork-and-return-merge.png'));
    await realClick('button[aria-label="Reset project zoom to 100%"]');
    await labelsClearOfRangeControls('actual-size merge view labels avoid actual DOM controls');
    await browser.wheelPointer(await canvasPoint(), { x: 70, y: -25 });
    await labelsClearOfRangeControls('panned merge view labels avoid actual DOM controls');
    await browser.setViewportSize(1200, 1000); await settle();
    await labelsClearOfRangeControls('resized merge inspector labels avoid actual DOM controls');
    await fitBounds('actual merge after label-obstacle checks');
    await browser.clickButtonByText(oid(304));
    await browser.waitForText('Merged topic 304');
    expect(await browser.evaluate(`new URLSearchParams(location.hash.split('?')[1]).get('focus')`)).toBe(oid(304));
    observations.push({ label: 'actual main merge; source parent order and clickable incoming parent', mergeOid: oid(60), parents: [oid(59), oid(304)], evidenceText });
  });
});
