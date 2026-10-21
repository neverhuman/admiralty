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
  let fixtureSequence = 0;
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
    source = sourceFixture();
    const sequence = ++fixtureSequence;
    source.projects.harbor.name = `Harbor initial fixture ${sequence}`;
    source.git.harbor.instance_id = `investigation-harbor-fixture-${sequence}`;
    source.git.observatory.instance_id = `investigation-observatory-fixture-${sequence}`;
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
    // Publish only the final per-test fixture, including pre-connect changes.
    server.writeLegacySnapshot(source);
    await browser.goto(`${server.baseUrl}/#/git`); await admitCurrentPage();
    await browser.waitForSelector(".git-band");
    // The native producer polls asynchronously. Do not interact with a cached
    // prior test snapshot while the newly written fixture is still in flight.
    await browser.waitForText(source.projects.harbor.name);
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
  const rangeControlGeometry=(selector = ".git-project-stage")=>browser.evaluate<{count:number;overlaps:unknown[];blocked:unknown[]}>(`(() => {
      const stage=document.querySelector(${JSON.stringify(selector)}),v=stage.getBoundingClientRect();
      const controls=[...stage.querySelectorAll('.git-commit-range')].map(e=>({e,r:e.getBoundingClientRect()}));
      const overlaps=[],blocked=[];
      for(let i=0;i<controls.length;i++) for(let j=i+1;j<controls.length;j++) {
        const a=controls[i].r,b=controls[j].r;
        if(Math.min(a.right,b.right)-Math.max(a.left,b.left)>.1 && Math.min(a.bottom,b.bottom)-Math.max(a.top,b.top)>.1)
          overlaps.push({a:controls[i].e.title,b:controls[j].e.title,aRect:a.toJSON(),bRect:b.toJSON()});
      }
      for(const {e,r} of controls) {
        const left=Math.max(r.left,v.left)+1,right=Math.min(r.right,v.right)-1,top=Math.max(r.top,v.top)+1,bottom=Math.min(r.bottom,v.bottom)-1;
        if(left>=right||top>=bottom) continue;
        const radius=Math.min(parseFloat(getComputedStyle(e).borderTopLeftRadius)||0,r.width/2,r.height/2);
        let probes=0;
        for(const [x,y] of [[(left+right)/2,(top+bottom)/2],[left,(top+bottom)/2],[right,(top+bottom)/2],[(left+right)/2,top],[(left+right)/2,bottom]]) {
          // Clipping a rounded end can put a bounding-box axis outside paint.
          // Probe only the actual rounded rectangle; require a visible probe.
          const cx=Math.max(r.left+radius,Math.min(r.right-radius,x)),cy=Math.max(r.top+radius,Math.min(r.bottom-radius,y));
          if((x-cx)**2+(y-cy)**2>radius**2)continue;
          probes++;
          const hit=document.elementFromPoint(x,y);if(hit!==e&&!e.contains(hit)) blocked.push({title:e.title,x,y,hit:hit?.className});
        }
        if(!probes)blocked.push({title:e.title,error:'No visible painted hit probe'});
      }
      return {count:controls.length,overlaps,blocked};
    })()`);

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

  it("zooms below5% with native controls and restores actual size and padded Fit", async () => {
    await connect(); await expand(); const fitted=await camera(); expect(fitted.k).toBeGreaterThan(.1);
    for(let n=0;n<32&&(await camera()).k>=.05;n++) await realClick('button[aria-label="Zoom project out"]');
    const small=await camera(); expect(small.k).toBeGreaterThan(0); expect(small.k).toBeLessThan(.05); expect(small.intent).toBe('manual');
    await realClick('button[aria-label="Reset project zoom to 100%"]'); expect((await camera()).k).toBe(1);
    await fitBounds('Fit restored after native below5% zoom'); const restored=await camera();
    expect(restored.k).toBeCloseTo(fitted.k,12); expect(restored.intent).toBe('fit');
    observations.push({label:'native below5% manual zoom,100% and Fit restore',fitted,small,restored});
  });

  async function pointerSequence(type: string, id: number, point: {x:number;y:number}) {
    await browser.evaluate(`(() => {
      const e=document.querySelector('.git-project-stage');
      if (!e.__testCaptures) {
        e.__testCaptures=new Set();
        // Synthetic PointerEvents have no browser-owned pointer ID. Only capture
        // custody is adapted; React's real handlers and camera renders execute.
        e.setPointerCapture=id=>e.__testCaptures.add(id);
        e.hasPointerCapture=id=>e.__testCaptures.has(id);
        e.releasePointerCapture=id=>{if(e.__testCaptures.delete(id)) e.dispatchEvent(new PointerEvent('lostpointercapture',{pointerId:id,bubbles:true}));};
      }
      e.dispatchEvent(new PointerEvent(${JSON.stringify(type)},{pointerId:${id},pointerType:'touch',button:0,buttons:${type === 'pointerup' || type === 'pointercancel' ? 0 : 1},clientX:${point.x},clientY:${point.y},bubbles:true,cancelable:true}));
    })()`);
    await settle();
  }

  it.each([101, 202])("continues an in-browser pinch as a drag when finger %i remains", async (remainingId) => {
    await connect(); await expand(); await realClick('button[aria-label="Reset project zoom to 100%"]');
    const point = await canvasPoint(), initial = await camera();
    const a = { id: 101, x: point.x, y: point.y }, b = { id: 202, x: point.x + 120, y: point.y };
    await pointerSequence('pointerdown', a.id, a); await pointerSequence('pointerdown', b.id, b);
    b.x += 80; await pointerSequence('pointermove', b.id, b);
    const pinched = await camera(); expect(pinched.k).toBeGreaterThan(initial.k);
    const remaining = remainingId === a.id ? a : b;
    const liftedPointer = remainingId === a.id ? b : a; await pointerSequence('pointerup', liftedPointer.id, liftedPointer);
    const lifted = await camera();
    expect({x:lifted.x,y:lifted.y,k:lifted.k}).toEqual({x:pinched.x,y:pinched.y,k:pinched.k});
    remaining.x += 36; remaining.y += 24; await pointerSequence('pointermove', remaining.id, remaining);
    const continued = await camera();
    expect(continued.k).toBe(pinched.k);
    expect(continued.x).toBeCloseTo(pinched.x + 36, 1); expect(continued.y).toBeCloseTo(pinched.y + 24, 1);
    await pointerSequence('pointerup', remaining.id, remaining);
    expect((await camera()).selection).toBeNull();
    observations.push({label:'in-browser pointer sequence; adapted capture custody; pinch to one-finger continuation',remainingId,initial,pinched,continued});
  });

  it.each(['cancel', 'lost capture'])("ends a pinch safely after %s and accepts a fresh drag", async (ending) => {
    await connect(); await expand(); await realClick('button[aria-label="Reset project zoom to 100%"]');
    const point = await canvasPoint(), a = {id:101,...point}, b = {id:202,x:point.x+120,y:point.y};
    await pointerSequence('pointerdown',a.id,a); await pointerSequence('pointerdown',b.id,b);
    b.x += 80; await pointerSequence('pointermove',b.id,b); const pinched = await camera();
    if (ending === 'cancel') await pointerSequence('pointercancel',b.id,b);
    else {
      expect(await browser.evaluate(`(() => { const e=document.querySelector('.git-project-stage'), id=202; if (!e.hasPointerCapture(id)) return false; e.releasePointerCapture(id); return true; })()`)).toBe(true);
      b.x += 10; await pointerSequence('pointermove',b.id,b);
      a.x += 30; await pointerSequence('pointermove',a.id,a);
      await pointerSequence('pointerup',a.id,a); await pointerSequence('pointerup',b.id,b);
    }
    const ended = await camera(); expect({x:ended.x,y:ended.y,k:ended.k}).toEqual({x:pinched.x,y:pinched.y,k:pinched.k});
    const next = {id:303,x:point.x,y:point.y}; await pointerSequence('pointerdown',next.id,next);
    next.x += 41; next.y += 27; await pointerSequence('pointermove',next.id,next); await pointerSequence('pointerup',next.id,next);
    const dragged = await camera(); expect(dragged.k).toBe(pinched.k);
    expect(dragged.x).toBeCloseTo(pinched.x+41,1); expect(dragged.y).toBeCloseTo(pinched.y+27,1);
    expect(dragged.selection).toBeNull();
  });

  it("suppresses a completed pinch click and permits a subsequent deliberate tap", async () => {
    await connect(); await expand();
    const point = await canvasPoint(), a = {id:101,...point}, b = {id:202,x:point.x+120,y:point.y};
    await browser.evaluate(`(() => { window.__gitStageClicks=0; document.addEventListener('click',event=>{if(event.target.matches('.git-project-stage')) window.__gitStageClicks++;}); })()`);
    await pointerSequence('pointerdown',a.id,a); await pointerSequence('pointerdown',b.id,b);
    b.x+=80; await pointerSequence('pointermove',b.id,b);
    await pointerSequence('pointerup',b.id,b); await pointerSequence('pointerup',a.id,a);
    const click = () => browser.evaluate(`document.querySelector('.git-project-stage').dispatchEvent(new MouseEvent('click',{detail:1,bubbles:true,cancelable:true}))`);
    await click(); expect(await browser.evaluate('window.__gitStageClicks')).toBe(0);
    await pointerSequence('pointerdown',303,point); await pointerSequence('pointerup',303,point);
    await click(); expect(await browser.evaluate('window.__gitStageClicks')).toBe(1);
  });

  it("discards an active pinch when the repository instance is replaced", async () => {
    await connect(); await expand();
    const point=await canvasPoint(), a={id:101,...point}, b={id:202,x:point.x+120,y:point.y};
    await pointerSequence('pointerdown',a.id,a); await pointerSequence('pointerdown',b.id,b);
    b.x+=80; await pointerSequence('pointermove',b.id,b);
    const replacement=structuredClone(source), stamp=new Date().toISOString();
    replacement.generated_utc=stamp; replacement.git.harbor.observed_utc=stamp; replacement.git.harbor.instance_id='pinch-replacement';
    replacement.projects.harbor.name='Harbor replaced during pinch'; server.writeLegacySnapshot(replacement);
    await browser.waitForText('Harbor replaced during pinch'); await browser.waitForSelector('.git-project-viewport[data-fitted="true"]');
    const before=await camera(); a.x+=36; await pointerSequence('pointermove',a.id,a); await pointerSequence('pointerup',a.id,a);
    const after=await camera(); expect({x:after.x,y:after.y,k:after.k,intent:after.intent}).toEqual({x:before.x,y:before.y,k:before.k,intent:before.intent});
    const next={id:303,...point}; await pointerSequence('pointerdown',next.id,next); next.x+=41; next.y+=27;
    await pointerSequence('pointermove',next.id,next); await pointerSequence('pointerup',next.id,next);
    const dragged=await camera(); expect(dragged.k).toBe(before.k); expect(dragged.x).toBeCloseTo(before.x+41,1); expect(dragged.y).toBeCloseTo(before.y+27,1);
  });

  it.each([1, 2, 3])("keeps ordinary-band ranges disjoint after %i native zoom-out steps", async (steps) => {
    await connect(); await selectByAriaLabel('Filter Git project', 'harbor');
    expect(await browser.evaluate(`document.querySelector('.git-canvas').getAttribute('aria-label')`)).toContain('92 commits');
    await realClick('button[aria-label="Reset Git zoom to 100% size"]');
    for(let i=0;i<steps;i++) await realClick('button[aria-label="Zoom Git out"]');
    const scale=await browser.evaluate<number>(`Number(document.querySelector('.git-canvas').dataset.zoom)`);
    expect(scale).toBeCloseTo(1 / 1.2 ** steps, 6);
    for(const phase of ['collapsed','expanded','panned']) {
      if(phase==='expanded') {
        const anchor=await browser.evaluate<string>(`(() => {const e=[...document.querySelectorAll('.git-scroll .git-commit-range[aria-expanded="false"]')].find(e=>{const r=e.getBoundingClientRect(),hit=document.elementFromPoint(r.left+r.width/2,r.top+r.height/2);return hit===e||e.contains(hit);});if(!e)throw Error('No hittable ordinary range');return e.dataset.anchorOid;})()`);
        await realClick(`.git-scroll .git-commit-range[data-anchor-oid="${anchor}"]`);
      }
      if(phase==='panned') {
        const point=await browser.evaluate<{x:number;y:number}>(`(() => {const r=document.querySelector('.git-scroll').getBoundingClientRect();return{x:r.left+r.width/2,y:r.bottom-40};})()`);
        await browser.wheelPointer(point,{x:180,y:0}); await settle();
      }
      const result=await rangeControlGeometry('.git-scroll');
      observations.push({label:'ordinary-band actual DOM geometry and native zoom',steps,scale,phase,...result});
      expect(result.count).toBeGreaterThanOrEqual(3); expect(result.overlaps).toEqual([]); expect(result.blocked).toEqual([]);
    }
  });

  it.each(["pointer", "keyboard"])("keeps a narrow ordinary-band range below closed chat and accessible by %s at low zoom", async (input) => {
    const repo=source.git.harbor,template=repo.commits[0];
    repo.commits=Array.from({length:201},(_,n)=>({...template,oid:oid(n+1),parents:n?[oid(n)]:[],subject:`Caption chain ${n+1}`}));
    repo.refs=[{name:'refs/heads/main',oid:oid(201),commit_oid:oid(201),kind:'branch'}]; repo.checkouts=[];
    source.git.observatory.commits=[];source.git.observatory.refs=[];source.git.observatory.checkouts=[];
    await browser.setViewportSize(640,844);await connect();
    expect(await browser.evaluate(`document.querySelector('.git-canvas').getAttribute('aria-label')`)).toContain('201 commits');
    await realClick('button[aria-label="Reset Git zoom to 100% size"]');
    for(let i=0;i<5;i++) await realClick('button[aria-label="Zoom Git out"]');
    const result=await rangeControlGeometry('.git-scroll');
    const caption=await browser.evaluate<{overlaps:unknown[]}>(`(() => {
      const controls=[...document.querySelectorAll('.git-scroll .git-commit-range')], obstacles=[...document.querySelectorAll('.git-chat-toggle,.git-band-caption')],overlaps=[];
      for(const e of controls)for(const o of obstacles){const a=e.getBoundingClientRect(),b=o.getBoundingClientRect();if(Math.min(a.right,b.right)-Math.max(a.left,b.left)>.1&&Math.min(a.bottom,b.bottom)-Math.max(a.top,b.top)>.1)overlaps.push({control:a.toJSON(),obstacle:b.toJSON(),className:o.className});}
      return {overlaps};
    })()`);
    observations.push({label:'ordinary narrow collapsed range versus actual chat and caption',scale:1/1.2**5,...result,caption});
    expect(result.count).toBe(1);expect(result.overlaps).toEqual([]);expect(result.blocked).toEqual([]);expect(caption.overlaps).toEqual([]);
    await browser.captureScreenshot(join(evidence,`ordinary-narrow-range-chat-clearance-${input}.png`));
    const anchor=await browser.evaluate<{x:number;y:number}>(`(() => {const e=document.querySelector('.git-commit-range');return{x:Number(e.dataset.worldX),y:Number(e.dataset.worldY)};})()`);
    for(let i=0;i<5;i++)await realClick('button[aria-label="Zoom Git out"]');
    expect(await browser.evaluate(`document.querySelectorAll('.git-commit-range').length`)).toBe(0);
    expect(await browser.evaluate(`document.querySelector('.git-canvas').getAttribute('aria-label')`)).toContain('1 collapsed ranges');
    if(input==='keyboard') {
      await browser.evaluate(`document.querySelector('.git-canvas').focus()`);await browser.pressKey('ArrowLeft');
    } else {
      const point=await browser.evaluate<{x:number;y:number;hit:boolean}>(`(() => {const c=document.querySelector('.git-canvas'),r=c.getBoundingClientRect(),scroll=document.querySelector('.git-scroll'),k=Number(c.dataset.zoom),x=r.left+${anchor.x}*k-scroll.scrollLeft,y=r.top+${anchor.y}*k;return{x,y,hit:document.elementFromPoint(x,y)===c};})()`);
      expect(point.hit).toBe(true);await browser.dragPointer(point,point);
    }
    await settle();
    expect(await browser.evaluate(`document.querySelector('.git-canvas').getAttribute('aria-label')`)).toContain('0 collapsed ranges');
  });

  it.each([
    {view:'ordinary',scale:1,steps:0}, {view:'ordinary',scale:1/1.2**2,steps:2},
    {view:'expanded',scale:1,steps:0}, {view:'expanded',scale:.69,steps:0},
  ])("inspects revealed commits without the collapse control intercepting clicks: $view at $scale", async ({view,scale,steps}) => {
    const repo=source.git.harbor,template=repo.commits[0];
    repo.commits=Array.from({length:15},(_,n)=>({...template,oid:oid(n+1),parents:n?[oid(n)]:[],subject:`Revealed chain ${n+1}`}));
    repo.refs=[{name:'refs/heads/main',oid:oid(15),commit_oid:oid(15),kind:'branch'}];repo.checkouts=[];
    await connect();
    if(view==='expanded')await expand();else await selectByAriaLabel('Filter Git project','harbor');
    await realClick(`button[aria-label="${view==='expanded'?'Reset project zoom to 100%':'Reset Git zoom to 100% size'}"]`);
    const selector=`.git-commit-range[data-anchor-oid="${oid(2)}"]`;
    await realClick(selector);await browser.waitForSelector(`${selector}[aria-expanded="true"]`);
    if(view==='expanded'&&scale!==1)await browser.wheelPointer(await canvasPoint(),{x:0,y:-Math.log(scale/(await camera()).k)/.002},{control:true});
    for(let i=0;i<steps;i++)await realClick('button[aria-label="Zoom Git out"]');
    expect(await browser.evaluate(`Number(document.querySelector('.git-canvas').dataset.zoom)`)).toBeCloseTo(scale,6);
    for(const n of [2,3]) {
      const point=await browser.evaluate<{x:number;y:number;hit:boolean;hitClass:string}>(`(() => {
        const c=document.querySelector('.git-canvas'),r=c.getBoundingClientRect(),b=document.querySelector(${JSON.stringify(selector)}),k=Number(c.dataset.zoom),stage=document.querySelector('.git-project-stage'),scroll=document.querySelector('.git-scroll');
        const x=r.left+(Number(b.dataset.worldX)+${(n-2)*112})*k+(stage?Number(stage.dataset.cameraX):-scroll.scrollLeft),y=r.top+Number(b.dataset.worldY)*k+(stage?Number(stage.dataset.cameraY):0),hit=document.elementFromPoint(x,y);
        return{x,y,hit:hit===c,hitClass:hit?.className??''};
      })()`);
      observations.push({label:'actual revealed commit native pick versus collapse control',view,scale,oid:oid(n),point});
      await browser.dragPointer(point,point);await settle();
      expect(await browser.evaluate(`document.querySelector('.git-canvas').getAttribute('aria-label')`)).toContain('0 collapsed ranges');
      expect(point.hit,JSON.stringify({view,scale,n,point})).toBe(true);
      await browser.waitForSelector('.lv-inspector');
      expect(await browser.evaluate(`document.querySelector('.lv-inspector').textContent`)).toContain(oid(n));
      expect(await browser.evaluate(`new URLSearchParams(location.hash.split('?')[1]).get('focus')`)).toBe(oid(n));
      expect(await browser.evaluate(`document.querySelector('.git-canvas').getAttribute('aria-label')`)).toContain('0 collapsed ranges');
    }
    const geometry=await rangeControlGeometry(view==='expanded'?'.git-project-stage':'.git-scroll');expect(geometry.overlaps).toEqual([]);expect(geometry.blocked).toEqual([]);
    await realClick(selector);await browser.waitForSelector(`${selector}[aria-expanded="false"]`);
    expect(await browser.evaluate(`document.querySelector('.git-canvas').getAttribute('aria-label')`)).toContain('1 collapsed ranges');
  });

  it.each([.65,.69,.9])("keeps actual range controls disjoint and hittable at scale %s", async (scale) => {
    await connect(); await expand(); const before=await camera();
    await browser.wheelPointer(await canvasPoint(),{x:0,y:-Math.log(scale/before.k)/.002},{control:true});
    expect((await camera()).k).toBeCloseTo(scale,6);
    for(const phase of ['collapsed','expanded','panned']) {
      if(phase==='expanded') {
        const anchor=await browser.evaluate<string>(`(() => {const e=[...document.querySelectorAll('.git-project-stage .git-commit-range[aria-expanded="false"]')].find(e=>{const r=e.getBoundingClientRect(),hit=document.elementFromPoint(r.left+r.width/2,r.top+r.height/2);return hit===e||e.contains(hit);});if(!e)throw Error('No hittable collapsed range');return e.dataset.anchorOid;})()`);
        await realClick(`.git-project-stage .git-commit-range[data-anchor-oid="${anchor}"]`);
      }
      if(phase==='panned') await browser.wheelPointer(await canvasPoint(),{x:240,y:0});
      const result=await rangeControlGeometry(); observations.push({label:'actual range-control intersection and hit test',scale,phase,...result});
      expect(result.count).toBeGreaterThanOrEqual(3); expect(result.overlaps).toEqual([]); expect(result.blocked).toEqual([]);
    }
  });

  it.each(['left','right'])("keeps clipped same-lane range controls apart at the %s edge", async(edge) => {
    const repo=source.git.harbor, template=repo.commits[0];
    repo.commits=Array.from({length:15},(_,n)=>({...template,oid:oid(n+1),parents:n?[oid(n)]:[],subject:`Edge chain ${n+1}`}));
    repo.refs=[{name:'refs/heads/main',oid:oid(15),commit_oid:oid(15),kind:'branch'},{name:'refs/tags/midpoint',oid:oid(8),commit_oid:oid(8),kind:'tag'}];
    repo.checkouts=[]; server.writeLegacySnapshot(source); await connect(); await expand();
    const start=await camera(); await browser.wheelPointer(await canvasPoint(),{x:0,y:-Math.log(.69/start.k)/.002},{control:true});
    expect((await camera()).k).toBeCloseTo(.69,6);
    for(const phase of ['collapsed','mixed']) {
      if(phase==='mixed') {
        await browser.clickButtonByAriaLabel('Fit graph');
        const anchor=await browser.evaluate<string>(`[...document.querySelectorAll('.git-project-stage .git-commit-range')].sort((a,b)=>Number(a.dataset.worldX)-Number(b.dataset.worldX))[1].dataset.anchorOid`);
        await realClick(`.git-project-stage .git-commit-range[data-anchor-oid="${anchor}"]`);
        const fitted=await camera(); await browser.wheelPointer(await canvasPoint(),{x:0,y:-Math.log(.69/fitted.k)/.002},{control:true});
      }
      const positions=await browser.evaluate<{oid:string;center:number}[]>(`(() => {const stage=document.querySelector('.git-project-stage'),v=stage.getBoundingClientRect();return[...stage.querySelectorAll('.git-commit-range')].sort((a,b)=>Number(a.dataset.worldX)-Number(b.dataset.worldX)).map(e=>{const r=e.getBoundingClientRect();return{oid:e.dataset.anchorOid,center:r.left+r.width/2-v.left-stage.clientLeft};});})()`);
      expect(positions).toHaveLength(2); const current=await camera(), clipped=positions[edge==='left'?0:1];
      // Place the actual control partly across the edge. Compact expanded
      // controls intentionally sit between commits, not at data-world-x.
      const desired=current.x+(edge==='left'?-5:current.width+5)-clipped.center;
      await browser.wheelPointer(await canvasPoint(),{x:current.x-desired,y:0});
      expect(await browser.evaluate(`(() => {const stage=document.querySelector('.git-project-stage'),v=stage.getBoundingClientRect(),r=document.querySelector('.git-commit-range[data-anchor-oid="${clipped.oid}"]').getBoundingClientRect(),edge=${JSON.stringify(edge==='left')}?v.left+stage.clientLeft:v.left+stage.clientLeft+stage.clientWidth;return r.left<edge&&r.right>edge;})()`)).toBe(true);
      const result=await rangeControlGeometry(); observations.push({label:'same-lane clipped control geometry',edge,phase,...result});
      expect(result.count).toBe(2); expect(result.overlaps).toEqual([]); expect(result.blocked).toEqual([]);
    }
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
    await browser.evaluate(`(() => { window.__gitMobileInput=[]; for(const type of ['pointerdown','pointerup','click']) document.addEventListener(type,event=>window.__gitMobileInput.push({type,target:event.target.className,x:event.clientX,y:event.clientY,open:document.querySelector('.git-project-chat-dock .git-chat-toggle')?.getAttribute('aria-expanded')}),true); })()`);
    for (const open of [true, false]) {
      const beforeToggle=await browser.evaluate(`document.querySelector('.git-project-chat-dock .git-chat-toggle').getAttribute('aria-expanded')`);
      await realClick('.git-project-chat-dock .git-chat-toggle');
      const afterToggle=await browser.evaluate(`document.querySelector('.git-project-chat-dock .git-chat-toggle').getAttribute('aria-expanded')`);
      if(afterToggle!==String(open)) {
        observations.push({label:'FAILED mobile native chat toggle',expected:open,beforeToggle,afterToggle,input:await browser.evaluate('window.__gitMobileInput')});
        await browser.captureScreenshot(join(evidence,'FAILED-mobile-chat-toggle.png'));
      }
      expect(afterToggle).toBe(String(open));
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
