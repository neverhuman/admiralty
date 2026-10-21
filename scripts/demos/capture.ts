#!/usr/bin/env node
// Record actual read-only production UI interactions as original sRGB PNGs.
import { createHash } from 'node:crypto';
import { lstat, mkdir, readFile, writeFile } from 'node:fs/promises';
import { parseArgs } from 'node:util';
import { resolve, join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { launchBrowser } from './browser.ts';
import { isProducerFeed, parseChecked } from './shapes.ts';

interface Point { x: number; y: number }
interface Frame { file: string; duration_ms: number; sha256: string }
interface Step { frame: number; label: string }

const { values } = parseArgs({ options: {
  url: { type: 'string', default: 'http://127.0.0.1:18890' },
  out: { type: 'string', default: '.capture/production' },
  project: { type: 'string', default: 'admiralty' },
  'atlas-project': { type: 'string' },
  scene: { type: 'string', default: 'all' },
  width: { type: 'string', default: '1440' }, height: { type: 'string', default: '900' },
  scale: { type: 'string', default: '2' }, 'token-file': { type: 'string' },
  'allow-stale': { type: 'boolean', default: false }, help: { type: 'boolean' },
} });
const option = (name: string): string | undefined => {
  const value = values[name];
  return typeof value === 'string' ? value : undefined;
};
const project = option('project') ?? 'admiralty', scene = option('scene') ?? 'all', tokenFile = option('token-file');
const allowStale = values['allow-stale'] === true;
if (values.help) {
  console.log('Usage: node scripts/demos/capture.ts [--url http://127.0.0.1:18890] [--project admiralty] [--atlas-project project-id] [--out .capture/production] [--scene all|fleet-console|git-flow|fleet-atlas] [--token-file /private/token] [--width 1440 --height 900 --scale 2] [--allow-stale]');
  process.exit(0);
}
const base = new URL(option('url') ?? '');
if (base.username || base.password || base.search || base.hash || base.pathname !== '/') throw new Error('--url must be an origin without credentials, query, hash or path.');
if (base.protocol !== 'https:' && !(base.protocol === 'http:' && ['127.0.0.1', 'localhost', '[::1]'].includes(base.hostname))) throw new Error('Use HTTPS or an HTTP loopback tunnel.');
const width = Number(option('width')), height = Number(option('height')), scale = Number(option('scale'));
if (![width, height, scale].every(Number.isInteger) || width < 1280 || height < 800 || width > 1920 || height > 1200 || scale < 1 || scale > 2) throw new Error('Use width 1280–1920, height 800–1200 and scale 1–2 (integers).');
const sceneNames = ['fleet-console', 'git-flow', 'fleet-atlas'];
if (scene !== 'all' && !sceneNames.includes(scene)) throw new Error('Unknown scene.');
let token = '';
if (tokenFile) {
  const info = await lstat(tokenFile);
  if (!info.isFile() || info.mode & 0o077 || (process.getuid && info.uid !== process.getuid())) throw new Error('Token must be an owned private regular file (0600).');
  token = (await readFile(tokenFile, 'utf8')).trim();
  if (token.length < 32 || /[\s\x00-\x1f\x7f]/.test(token)) throw new Error('Token must contain at least 32 characters without whitespace or control characters.');
}
const headers: Record<string, string> = token ? { Authorization: `Bearer ${token}` } : {};
if (token) {
  const access = await fetch(new URL('/v2/views', base), { headers, signal: AbortSignal.timeout(15000) });
  await access.body?.cancel();
  if (!access.ok) throw new Error(`Native view access failed (${access.status}); check the supplied token before recording.`);
}
const response = await fetch(new URL('/v1/legacy', base), { headers, signal: AbortSignal.timeout(15000) });
if (!response.ok) throw new Error(`Production preflight failed (${response.status}). Supply --token-file if needed.`);
const sourceBytes = await response.text(), source = parseChecked(sourceBytes, isProducerFeed, 'Producer feed');
if (source.simulated || !source.generated_utc || !source.host || !Object.keys(source.git ?? {}).length) throw new Error('Expected a real producer feed with Git observations; synthetic/demo captures are refused.');
const git = source.git ?? {};
if (!git[project]?.commits?.length) throw new Error('--project must name a producer project with observed commits.');
const sourceAge = Date.now() - Date.parse(source.generated_utc);
if (!Number.isFinite(sourceAge) || sourceAge < -60000 || (sourceAge > 180000 && !allowStale)) throw new Error('Producer time is invalid/stale; inspect the source or explicitly use --allow-stale.');
// A small project makes individual command relationships readable after the fleet overview.
const atlasProject = option('atlas-project') ?? (source.order ?? Object.keys(git)).find(candidate => {
  const count = (source.agents ?? []).filter(agent => agent.project === candidate).length;
  return count > 0 && count <= 4;
}) ?? project;
const atlasWorkers = (source.agents ?? []).filter(agent => agent.project === atlasProject).flatMap(agent => [agent.screen, agent.id]).filter(Boolean);
if ((scene === 'all' || scene === 'fleet-atlas') && (!git[atlasProject] || !atlasWorkers.length)) throw new Error('--atlas-project needs a real project with observed workers.');

const out = resolve(option('out') ?? '');
await mkdir(out, { recursive: true, mode: 0o700 });
const browser = await launchBrowser({ width, height, scale, origin: base.origin });
const { cdp } = browser;
let frames: Frame[] = [], steps: Step[] = [], directory = '', cursor: Point = { x: 40, y: 40 };
const evaluate = <T = unknown>(expression: string) => cdp.evaluate<T>(expression);
const waitFor = (selector: string) => cdp.wait(`!!document.querySelector(${JSON.stringify(selector)})`, selector);
const rect = (selector: string) => evaluate<Point>(`(() => {
  const element = document.querySelector(${JSON.stringify(selector)});
  if (!element) throw Error('Capture target missing');
  const r = element.getBoundingClientRect();
  if (!r.width || !r.height || r.bottom <= 0 || r.top >= innerHeight || r.right <= 0 || r.left >= innerWidth) throw Error('Capture target outside viewport');
  return {x:(Math.max(0,r.left)+Math.min(innerWidth,r.right))/2,y:(Math.max(0,r.top)+Math.min(innerHeight,r.bottom))/2};
})()`);

async function overlay(): Promise<void> {
  await evaluate(`(() => {
    if (document.querySelector('#capture-pointer')) return;
    const style = document.createElement('style');
    style.textContent = '#capture-pointer{position:fixed;top:0;left:0;width:23px;height:30px;z-index:2147483647;pointer-events:none;filter:drop-shadow(0 2px 3px #000)}#capture-ring{position:fixed;width:38px;height:38px;border:3px solid #56dfff;border-radius:50%;z-index:2147483646;pointer-events:none;display:none;box-shadow:0 0 0 5px #56dfff33}';
    document.head.append(style);
    const pointer=document.createElement('div');pointer.id='capture-pointer';
    pointer.innerHTML='<svg viewBox="0 0 23 30"><path d="M2 2V24L8 18L13 28L18 25L13 16H22Z" fill="white" stroke="#111827" stroke-width="2"/></svg>';
    const ring=document.createElement('div');ring.id='capture-ring';document.body.append(pointer,ring);
  })()`);
}
async function position(point: Point, ring = false): Promise<void> {
  cursor = point;
  await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', ...point });
  await evaluate(`(() => {
    document.querySelector('#capture-pointer').style.transform='translate(${point.x}px,${point.y}px)';
    const ring=document.querySelector('#capture-ring');ring.style.left='${point.x - 22}px';ring.style.top='${point.y - 22}px';ring.style.display=${JSON.stringify(ring ? 'block' : 'none')};
  })()`);
}
async function frame(duration = 1000, label?: string): Promise<void> {
  if (browser.blocked.length) throw new Error('Capture blocked an external request or a write; inspect the route before continuing.');
  if (token && await evaluate(`document.body.innerText.includes(${JSON.stringify(token)})`)) throw new Error('Credential visible; capture refused.');
  const file = `${String(frames.length).padStart(5, '0')}.png`;
  const { data } = await cdp.send('Page.captureScreenshot', { format: 'png', fromSurface: true, captureBeyondViewport: false });
  const bytes = Buffer.from(String(data), 'base64');
  await writeFile(join(directory, file), bytes);
  frames.push({ file, duration_ms: duration, sha256: createHash('sha256').update(bytes).digest('hex') });
  if (label) steps.push({ frame: frames.length - 1, label });
}
async function move(point: Point): Promise<void> {
  const start = cursor;
  for (let step = 1; step <= 5; step++) {
    const t = step / 5, ease = t * t * (3 - 2 * t);
    await position({ x: start.x + (point.x - start.x) * ease, y: start.y + (point.y - start.y) * ease });
    await frame(80);
  }
}
async function click(selector: string, label: string, duration = 1200): Promise<void> {
  await evaluate(`document.querySelector(${JSON.stringify(selector)})?.scrollIntoView({block:'nearest',inline:'nearest',behavior:'instant'})`);
  await delay(100);
  await move(await rect(selector));
  await position(cursor, true); await frame(160);
  await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', ...cursor, button: 'left', clickCount: 1 });
  await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', ...cursor, button: 'left', clickCount: 1 });
  await delay(350); await overlay(); await position(cursor); await frame(duration, label);
}
async function button(text: string, label: string, duration?: number): Promise<void> {
  const selector = await evaluate<string>(`(() => {
    const e=[...document.querySelectorAll('button')].find(e=>e.textContent.trim()===${JSON.stringify(text)});
    if(!e) throw Error('Button missing: '+${JSON.stringify(text)});e.dataset.captureTarget='button';return '[data-capture-target="button"]';
  })()`);
  await click(selector, label, duration);
  await evaluate(`document.querySelector('[data-capture-target="button"]')?.removeAttribute('data-capture-target')`);
}
async function prepareButton(text: string): Promise<void> {
  const point = await evaluate<Point>(`(() => {const e=[...document.querySelectorAll('button')].find(e=>e.textContent.trim()===${JSON.stringify(text)});if(!e)throw Error('Preparation button missing');const r=e.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2};})()`);
  await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', ...point, button: 'left', clickCount: 1 });
  await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', ...point, button: 'left', clickCount: 1 });
  await delay(500);
}
async function scroll(selector: string, delta: number, label: string): Promise<void> {
  await evaluate(`document.querySelector(${JSON.stringify(selector)})?.scrollIntoView({block:'nearest',inline:'nearest',behavior:'instant'})`);
  await delay(100);
  await move(await rect(selector));
  for (let step = 0; step < 6; step++) {
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseWheel', ...cursor, deltaY: delta / 6, deltaX: 0 });
    await delay(90); await frame(100);
  }
  await frame(1400, label);
}
async function navigate(route: string, ready: string): Promise<void> {
  if (await evaluate(`location.origin===${JSON.stringify(base.origin)}`)) {
    // Hash navigation retains the in-memory owner token across all scenes.
    await evaluate(`location.hash=${JSON.stringify(`#/${route}`)}`);
  } else await cdp.send('Page.navigate', { url: new URL(`/#/${route}`, base).href });
  await waitFor(ready); await evaluate('document.fonts.ready.then(()=>true)'); await delay(1200); await overlay(); await position({ x: 44, y: 44 });
}

try {
  // Authenticate before any frames exist. The browser is disposable, with no persistent storage.
  if (token) {
    await navigate('detail', '.legacy-toolbar');
    await evaluate(`document.querySelector('[aria-label="Access settings"]').click()`);
    await waitFor('#bearer-token');
    await evaluate(`(() => { const e=document.querySelector('#bearer-token');Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(e,${JSON.stringify(token)});e.dispatchEvent(new Event('input',{bubbles:true})); })()`);
    await evaluate(`[...document.querySelectorAll('button')].find(e=>e.textContent==='Apply token').click()`);
    await cdp.wait(`!document.querySelector('[role="dialog"]')`);
  }
  for (const name of sceneNames.filter(name => scene === 'all' || scene === name)) {
    directory = join(out, name);
    // Refuse overwrite: old frames must never contaminate a new production run.
    await mkdir(directory, { mode: 0o700 });
    frames = []; steps = [];
    console.log(`Capturing ${name} (${width * scale}×${height * scale})…`);
    if (name === 'fleet-console') {
      await navigate('detail', '.legacy-project');
      await frame(1600, 'Real fleet projects, model badges, history and source freshness');
      const projectTab = await evaluate<string>(`(() => {
        const tab=[...document.querySelectorAll('[role="tab"]')].find(e=>e.querySelector('.legacy-tab-title')?.textContent.toLowerCase().startsWith(${JSON.stringify(project.toLowerCase())}));
        if(!tab) throw Error('Project tab missing');tab.dataset.captureProject='true';return '[data-capture-project="true"]';
      })()`);
      await click(projectTab, 'Select the real project and its reported task stages', 1800);
      await click('.temporal-plot:nth-child(3) .temporal-toggle', 'Expand measured task burn-up history', 1600);
      await click('.temporal-plot:nth-child(3) .temporal-toggle', 'Return to the compact history strip', 400);
      await scroll('.legacy-project', 590, 'Inspect project evidence and observed worker models');
      await frame(1600, 'Source claims and uncertainty remain visible');
    } else if (name === 'git-flow') {
      await navigate(`git?project=${encodeURIComponent(project)}`, '.git-canvas');
      // Prime the actual viewport through UI controls before the opening shot.
      await prepareButton('Fit'); await prepareButton('Latest →');
      await frame(1800, 'Real repository ancestry, refs, checkouts and working changes');
      await button('Fit', 'Fit observed ancestry into the viewport', 1300);
      await button('Latest →', 'Follow the actual observed commit frontier', 1600);
      await button('☷ Inventory', 'Open exact commit and checkout inventory', 1400);
      await click('.git-checkout-row', 'Inspect an actual checkout and working-state coverage', 2200);
      await click('[aria-label="Close inspector"]', 'Return to repository inventory', 300);
      await click('.git-inventory tbody button', 'Inspect the exact commit, parent IDs and source time', 2200);
      await click('[aria-label="Close inspector"]', 'Return to commit inventory', 300);
      await button('⑂ Graph', 'Return to live ancestry', 1800);
    } else {
      await navigate('atlas', '.atlas-agent');
      await frame(1800, 'Owner, Admiral, Commodores and discovered workers');
      await button('Fit fleet', 'Fit the complete observed fleet', 1200);
      const focus = await evaluate<string>(`(() => {
        const board=[...document.querySelectorAll('.atlas-board')].find(e=>new URLSearchParams(e.querySelector('a')?.hash.split('?')[1]).get('project')===${JSON.stringify(atlasProject)});
        const e=board?.querySelector('header button');
        if(!e) throw Error('Project board target missing');e.dataset.captureFocus='true';return '[data-capture-focus="true"]';
      })()`);
      await click(focus, 'Focus the project command structure', 2000);
      const worker = await evaluate<string>(`(() => {
        const names=${JSON.stringify(atlasWorkers)};
        const e=[...document.querySelectorAll('.atlas-agent.worker')].find(e=>names.includes(e.querySelector('strong')?.textContent));
        if(!e)throw Error('Observed project worker missing');e.dataset.captureWorker='true';return '[data-capture-worker="true"]';
      })()`);
      await click(worker, 'Inspect an observed worker, reported model and source time', 2200);
      await click('[aria-label="Close inspector"]', 'Return to the fleet map', 300);
      await button('☷ Roster', 'Open the searchable real agent roster', 1800);
      await button('✦ Map', 'Return to the fleet map', 500);
      await button('Fit fleet', 'Return to the complete fleet', 1800);
    }
    const manifest = {
      schema: 1, scene: name, captured_utc: new Date().toISOString(),
      source: { kind: 'live-production', origin: base.origin, generated_utc: source.generated_utc,
        host: source.host, snapshot_sha256: createHash('sha256').update(sourceBytes).digest('hex'),
        projects: Object.keys(git), observed_agents: source.agents?.length ?? null,
        stale_explicitly_allowed: allowStale, authenticated: Boolean(token) },
      focus_project: name === 'fleet-atlas' ? atlasProject : project,
      viewport: { width, height, scale, pixel_width: width * scale, pixel_height: height * scale },
      editing: 'Original browser PNGs; cursor/click overlay only. Holds and transitions use authored durations; this is an edited walkthrough, not a real-time recording.',
      frames, steps, allowed_requests: [...browser.requests].sort(), blocked_requests: browser.blocked,
    };
    await writeFile(join(directory, 'capture.json'), JSON.stringify(manifest, null, 2) + '\n');
    console.log(`${name}: ${frames.length} frames, ${frames.reduce((n, f) => n + f.duration_ms, 0) / 1000}s`);
  }
} finally { await browser.close(); }
