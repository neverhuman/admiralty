import { spawn, type ChildProcess } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync } from 'node:fs';
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { homedir, tmpdir } from 'node:os';
import { extname, join, normalize, resolve } from 'node:path';
import type { Duplex, Readable, Writable } from 'node:stream';
import { fileURLToPath } from 'node:url';
import { build } from 'vite';
import { IN_PAGE_AXE_SCRIPT, type AxeAuditReport } from './axe-audit';
import {
  createFleetFixture,
  createLegacyFixture,
  createModelsFixture,
  type FleetFixture,
  type ModelsFixture,
} from './fixtures';

const E2E_DIR = fileURLToPath(new URL('.', import.meta.url));
export const DASHBOARD_ROOT = resolve(E2E_DIR, '..');
export const DIST_DIR = resolve(DASHBOARD_ROOT, 'dist');

const MIME_TYPES: Record<string, string> = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'application/javascript; charset=utf-8',
  '.mjs': 'application/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.png': 'image/png',
  '.svg': 'image/svg+xml',
  '.ico': 'image/x-icon',
};

export async function ensureBuiltDashboard(): Promise<{ indexHtml: string; assets: string[] }> {
  const indexPath = join(DIST_DIR, 'index.html');
  const assetsDir = join(DIST_DIR, 'assets');
  if (!existsSync(indexPath) || !existsSync(assetsDir)) {
    await build({ root: DASHBOARD_ROOT, logLevel: 'error' });
  }
  const indexHtml = readFileSync(indexPath, 'utf8');
  const assets = existsSync(assetsDir) ? readdirSync(assetsDir) : [];
  return { indexHtml, assets };
}

export interface RecordedRequest {
  method: string;
  url: string;
  pathname: string;
  search: string;
  authorization: string | null;
  body: string;
  timestampMs: number;
}

export interface DashboardTestServer {
  baseUrl: string;
  requests: RecordedRequest[];
  clearRequests(): void;
  setFleet(fleet: FleetFixture): void;
  setFleetStatus(status: number, errorDetail?: string): void;
  setLegacy(legacy: Record<string, unknown> | null, status?: number): void;
  setModels(models: ModelsFixture, status?: number): void;
  setModelsPutConflict(conflict: boolean): void;
  setRequireBearerToken(token: string | null): void;
  sendSseFrame(rawFrame: string): void;
  closeActiveSseStreams(): void;
  activeSseCount(): number;
  close(): Promise<void>;
}

export async function startDashboardTestServer(): Promise<DashboardTestServer> {
  await ensureBuiltDashboard();

  let fleet = createFleetFixture(4);
  let fleetStatus = 200;
  let fleetErrorDetail = 'Service unavailable';
  let legacy: Record<string, unknown> | null = createLegacyFixture(4);
  let legacyStatus = 200;
  let models = createModelsFixture('1');
  let modelsStatus = 200;
  let forcePutConflict = false;
  let requiredToken: string | null = null;

  const requests: RecordedRequest[] = [];
  const sseStreams = new Set<ServerResponse>();

  function checkAuthorized(req: IncomingMessage, res: ServerResponse): boolean {
    if (!requiredToken) return true;
    const auth = req.headers.authorization ?? '';
    if (auth !== `Bearer ${requiredToken}`) {
      res.writeHead(401, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' });
      res.end(JSON.stringify({ error: 'Unauthorized', detail: 'Bearer token required' }));
      return false;
    }
    return true;
  }

  const server = createServer((req, res) => {
    const parsedUrl = new URL(req.url ?? '/', 'http://127.0.0.1');
    const chunks: Buffer[] = [];
    req.on('data', (chunk: Buffer) => chunks.push(chunk));
    req.on('end', () => {
      const body = Buffer.concat(chunks).toString('utf8');
      requests.push({
        method: req.method ?? 'GET',
        url: req.url ?? '/',
        pathname: parsedUrl.pathname,
        search: parsedUrl.search,
        authorization: typeof req.headers.authorization === 'string' ? req.headers.authorization : null,
        body,
        timestampMs: Date.now(),
      });

      if (parsedUrl.pathname === '/v1/fleet') {
        if (!checkAuthorized(req, res)) return;
        if (fleetStatus !== 200) {
          res.writeHead(fleetStatus, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' });
          res.end(JSON.stringify({ detail: fleetErrorDetail }));
          return;
        }
        res.writeHead(200, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' });
        res.end(JSON.stringify(fleet));
        return;
      }

      if (parsedUrl.pathname === '/v1/events') {
        if (!checkAuthorized(req, res)) return;
        res.writeHead(200, {
          'Content-Type': 'text/event-stream',
          'Cache-Control': 'no-store',
          Connection: 'keep-alive',
        });
        res.write(': connected\n\n');
        sseStreams.add(res);
        res.on('close', () => sseStreams.delete(res));
        return;
      }

      if (parsedUrl.pathname === '/v1/legacy') {
        if (!checkAuthorized(req, res)) return;
        if (legacyStatus !== 200 || !legacy) {
          res.writeHead(legacyStatus === 200 ? 404 : legacyStatus, {
            'Content-Type': 'application/json',
            'Cache-Control': 'no-store',
          });
          res.end(JSON.stringify({ detail: 'Legacy snapshot unavailable' }));
          return;
        }
        res.writeHead(200, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' });
        res.end(JSON.stringify(legacy));
        return;
      }

      if (parsedUrl.pathname === '/v1/models') {
        if (!checkAuthorized(req, res)) return;
        if (modelsStatus !== 200) {
          res.writeHead(modelsStatus, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' });
          res.end(JSON.stringify({ detail: 'Model registry unavailable' }));
          return;
        }
        if (req.method === 'GET') {
          res.writeHead(200, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' });
          res.end(JSON.stringify(models));
          return;
        }
        if (req.method === 'PUT') {
          const payload = JSON.parse(body) as {
            expected_revision: string;
            models: Array<Record<string, unknown>>;
          };
          if (forcePutConflict || payload.expected_revision !== models.revision) {
            forcePutConflict = false;
            models = { ...models, revision: String(BigInt(models.revision) + 1n) };
            res.writeHead(409, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' });
            res.end(JSON.stringify({ detail: 'Policy revision conflict' }));
            return;
          }
          const nextRevision = String(BigInt(models.revision) + 1n);
          models = {
            schema_version: 1,
            revision: nextRevision,
            native_execution_qualified: false,
            models: payload.models.map((item) => ({
              id: String(item.id),
              label: String(item.label),
              provider: item.provider as ModelsFixture['models'][number]['provider'],
              provider_model: String(item.provider_model),
              tier: Number(item.tier),
              enabled: Boolean(item.enabled),
              node_id: item.node_id === null ? null : String(item.node_id),
              max_concurrency: Number(item.max_concurrency),
              capabilities: (item.capabilities as string[]).map(String),
              task_max_attempts: Number(item.task_max_attempts),
              escalate_to: item.escalate_to === null ? null : String(item.escalate_to),
              usage_cost: { status: 'unknown' as const },
            })),
          };
          res.writeHead(200, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' });
          res.end(JSON.stringify(models));
          return;
        }
      }

      // Serve built static files from dist/
      const safeRelPath = normalize(parsedUrl.pathname).replace(/^(\.\.(\/|\\|$))+/, '');
      let filePath = join(DIST_DIR, safeRelPath === '/' ? 'index.html' : safeRelPath);
      if (!existsSync(filePath) || statSync(filePath).isDirectory()) {
        if (parsedUrl.pathname.startsWith('/v1/')) {
          res.writeHead(404, { 'Content-Type': 'application/json' });
          res.end(JSON.stringify({ error: 'Not found' }));
          return;
        }
        filePath = join(DIST_DIR, 'index.html');
      }
      const ext = extname(filePath);
      const contentType = MIME_TYPES[ext] ?? 'application/octet-stream';
      res.writeHead(200, { 'Content-Type': contentType, 'Cache-Control': 'no-store' });
      res.end(readFileSync(filePath));
    });
  });

  await new Promise<void>((resolveListen, rejectListen) => {
    server.once('error', rejectListen);
    server.listen(0, '127.0.0.1', () => resolveListen());
  });

  const addr = server.address();
  if (!addr || typeof addr === 'string') throw new Error('Failed to bind ephemeral loopback server');
  const baseUrl = `http://127.0.0.1:${addr.port}`;

  return {
    baseUrl,
    requests,
    clearRequests() {
      requests.length = 0;
    },
    setFleet(next) {
      fleet = next;
      fleetStatus = 200;
    },
    setFleetStatus(status, errorDetail = 'Service unavailable') {
      fleetStatus = status;
      fleetErrorDetail = errorDetail;
    },
    setLegacy(next, status = 200) {
      legacy = next;
      legacyStatus = status;
    },
    setModels(next, status = 200) {
      models = next;
      modelsStatus = status;
    },
    setModelsPutConflict(conflict) {
      forcePutConflict = conflict;
    },
    setRequireBearerToken(token) {
      requiredToken = token;
    },
    sendSseFrame(rawFrame) {
      for (const stream of sseStreams) {
        stream.write(rawFrame);
      }
    },
    closeActiveSseStreams() {
      for (const stream of sseStreams) {
        stream.end();
      }
      sseStreams.clear();
    },
    activeSseCount() {
      return sseStreams.size;
    },
    async close() {
      for (const stream of sseStreams) {
        stream.destroy();
      }
      sseStreams.clear();
      await new Promise<void>((resolveClose) => server.close(() => resolveClose()));
    },
  };
}

export function discoverChromiumBinary(): string {
  if (process.env.ADMIRALTY_BROWSER_BINARY && existsSync(process.env.ADMIRALTY_BROWSER_BINARY)) {
    return process.env.ADMIRALTY_BROWSER_BINARY;
  }
  const playwrightCacheRoots = [
    process.env.PLAYWRIGHT_BROWSERS_PATH,
    join(homedir(), '.cache', 'ms-playwright'),
  ].filter((item): item is string => Boolean(item));

  for (const root of playwrightCacheRoots) {
    if (!existsSync(root)) continue;
    for (const entry of readdirSync(root).sort().reverse()) {
      if (entry.startsWith('chromium-')) {
        const candidate = join(root, entry, 'chrome-linux64', 'chrome');
        if (existsSync(candidate)) return candidate;
      }
      if (entry.startsWith('chromium_headless_shell-')) {
        const candidate = join(root, entry, 'chrome-headless-shell-linux64', 'chrome-headless-shell');
        if (existsSync(candidate)) return candidate;
      }
    }
  }

  for (const candidate of [
    '/usr/bin/google-chrome',
    '/usr/bin/google-chrome-stable',
    '/usr/bin/chromium',
    '/usr/bin/chromium-browser',
  ]) {
    if (existsSync(candidate)) return candidate;
  }

  throw new Error('No Chromium or Chrome binary found for dashboard E2E qualification');
}

export interface BrowserHarness {
  binaryPath: string;
  resetPage(): Promise<void>;
  goto(url: string): Promise<void>;
  setViewportSize(width: number, height: number): Promise<void>;
  emulateReducedMotion(reduce: boolean): Promise<void>;
  evaluate<T>(expression: string): Promise<T>;
  waitForText(expectedText: string, timeoutMs?: number): Promise<void>;
  waitForSelector(selector: string, timeoutMs?: number): Promise<void>;
  clickNav(viewLabel: string): Promise<void>;
  clickButtonByText(labelText: string): Promise<void>;
  clickButtonByAriaLabel(ariaLabel: string): Promise<void>;
  fillInputByLabel(labelText: string, value: string): Promise<void>;
  selectByLabel(labelText: string, value: string): Promise<void>;
  pressKey(key: string): Promise<void>;
  runAxeAudit(): Promise<AxeAuditReport>;
  close(): Promise<void>;
}

export async function launchBrowserHarness(): Promise<BrowserHarness> {
  const binaryPath = discoverChromiumBinary();
  const baseTmp = process.env.TMPDIR && existsSync(process.env.TMPDIR) ? process.env.TMPDIR : tmpdir();
  mkdirSync(baseTmp, { recursive: true, mode: 0o700 });
  const profileDir = mkdtempSync(join(baseTmp, 'adm-e2e-browser-'));

  const child: ChildProcess = spawn(
    binaryPath,
    [
      '--headless=new',
      '--no-sandbox',
      '--disable-setuid-sandbox',
      '--disable-gpu',
      '--disable-dev-shm-usage',
      '--disable-background-networking',
      '--disable-default-apps',
      '--disable-extensions',
      '--disable-sync',
      '--no-first-run',
      `--user-data-dir=${profileDir}`,
      '--window-size=1440,900',
      '--remote-debugging-pipe',
    ],
    { stdio: ['ignore', 'pipe', 'pipe', 'pipe', 'pipe'] },
  );

  const cdpWrite = child.stdio[3] as Writable;
  const cdpRead = child.stdio[4] as Readable | Duplex;

  let nextId = 1;
  let buffer = '';
  const pending = new Map<
    number,
    { resolve: (value: Record<string, unknown>) => void; reject: (err: Error) => void }
  >();

  cdpRead.on('data', (chunk: Buffer) => {
    buffer += chunk.toString('utf8');
    let nullIdx: number;
    while ((nullIdx = buffer.indexOf('\0')) !== -1) {
      const raw = buffer.slice(0, nullIdx);
      buffer = buffer.slice(nullIdx + 1);
      if (!raw.trim()) continue;
      const msg = JSON.parse(raw) as {
        id?: number;
        result?: Record<string, unknown>;
        error?: { message?: string };
      };
      if (msg.id !== undefined && pending.has(msg.id)) {
        const entry = pending.get(msg.id)!;
        pending.delete(msg.id);
        if (msg.error) {
          entry.reject(new Error(msg.error.message ?? 'CDP command failed'));
        } else {
          entry.resolve(msg.result ?? {});
        }
      }
    }
  });

  function sendCdp(
    method: string,
    params: Record<string, unknown> = {},
    sessionId?: string,
  ): Promise<Record<string, unknown>> {
    const id = nextId++;
    return new Promise((resolveCmd, rejectCmd) => {
      pending.set(id, { resolve: resolveCmd, reject: rejectCmd });
      const payload = sessionId ? { id, method, params, sessionId } : { id, method, params };
      cdpWrite.write(`${JSON.stringify(payload)}\0`);
    });
  }

  const created = await sendCdp('Target.createTarget', { url: 'about:blank' });
  const targetId = String(created.targetId);
  const attached = await sendCdp('Target.attachToTarget', { targetId, flatten: true });
  const sessionId = String(attached.sessionId);

  await sendCdp('Page.enable', {}, sessionId);
  await sendCdp('Runtime.enable', {}, sessionId);
  await sendCdp('DOM.enable', {}, sessionId);
  await sendCdp('Accessibility.enable', {}, sessionId);
  await sendCdp(
    'Emulation.setDeviceMetricsOverride',
    { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false },
    sessionId,
  );

  async function evaluate<T>(expression: string): Promise<T> {
    const res = await sendCdp(
      'Runtime.evaluate',
      {
        expression,
        returnByValue: true,
        awaitPromise: true,
      },
      sessionId,
    );
    const exceptionDetails = res.exceptionDetails as { text?: string; exception?: { description?: string } } | undefined;
    if (exceptionDetails) {
      throw new Error(
        `Browser evaluation failed: ${exceptionDetails.exception?.description ?? exceptionDetails.text ?? 'unknown error'}`,
      );
    }
    const resultObj = res.result as { value?: T } | undefined;
    return resultObj?.value as T;
  }

  async function waitForCondition(
    predicateExpr: string,
    description: string,
    timeoutMs = 10_000,
  ): Promise<void> {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      const ok = await evaluate<boolean>(`Boolean(${predicateExpr})`);
      if (ok) return;
      await new Promise((r) => setTimeout(r, 40));
    }
    const bodySnippet = await evaluate<string>(`(document.body?.innerText || '').slice(0, 600)`).catch(
      () => '<unavailable>',
    );
    throw new Error(`Timed out waiting for ${description}. Current page text: ${bodySnippet}`);
  }

  return {
    binaryPath,
    async resetPage() {
      await sendCdp('Page.navigate', { url: 'about:blank' }, sessionId);
      await waitForCondition(
        `window.location.href === 'about:blank' && document.readyState === 'complete'`,
        'reset to about:blank',
      );
    },
    async goto(url: string) {
      await sendCdp('Page.navigate', { url }, sessionId);
      await waitForCondition(
        `document.readyState === 'complete' && document.querySelector('#root')?.children.length > 0`,
        `navigation to ${url}`,
      );
    },
    async setViewportSize(width: number, height: number) {
      await sendCdp(
        'Emulation.setDeviceMetricsOverride',
        { width, height, deviceScaleFactor: 1, mobile: width <= 600 },
        sessionId,
      );
    },
    async emulateReducedMotion(reduce: boolean) {
      await sendCdp(
        'Emulation.setEmulatedMedia',
        {
          features: [{ name: 'prefers-reduced-motion', value: reduce ? 'reduce' : 'no-preference' }],
        },
        sessionId,
      );
    },
    evaluate,
    async waitForText(expectedText: string, timeoutMs = 10_000) {
      const escaped = JSON.stringify(expectedText);
      await waitForCondition(
        `(document.body?.textContent || '').includes(${escaped})`,
        `text ${escaped}`,
        timeoutMs,
      );
    },
    async waitForSelector(selector: string, timeoutMs = 10_000) {
      const escaped = JSON.stringify(selector);
      await waitForCondition(
        `document.querySelector(${escaped}) !== null`,
        `selector ${escaped}`,
        timeoutMs,
      );
    },
    async clickNav(viewLabel: string) {
      const escaped = JSON.stringify(viewLabel);
      const clicked = await evaluate<boolean>(`(() => {
        const buttons = [...document.querySelectorAll('nav button.nav-item')];
        const target = buttons.find(b => (b.textContent || '').includes(${escaped}));
        if (!target) return false;
        target.click();
        return true;
      })()`);
      if (!clicked) throw new Error(`Navigation button not found: ${viewLabel}`);
    },
    async clickButtonByText(labelText: string) {
      const escaped = JSON.stringify(labelText);
      const clicked = await evaluate<boolean>(`(() => {
        const buttons = [...document.querySelectorAll('button, [role="tab"], summary')];
        const target = buttons.find(b => (b.textContent || '').replace(/\\s+/g, ' ').trim().includes(${escaped}));
        if (!target) return false;
        target.click();
        return true;
      })()`);
      if (!clicked) throw new Error(`Button with text not found: ${labelText}`);
    },
    async clickButtonByAriaLabel(ariaLabel: string) {
      const escaped = JSON.stringify(ariaLabel);
      const clicked = await evaluate<boolean>(`(() => {
        const target = [...document.querySelectorAll('[aria-label]')].find(
          el => el.getAttribute('aria-label') === ${escaped}
        );
        if (!target || !(target instanceof HTMLElement)) return false;
        target.click();
        return true;
      })()`);
      if (!clicked) throw new Error(`Control with aria-label not found: ${ariaLabel}`);
    },
    async fillInputByLabel(labelText: string, value: string) {
      const labelEsc = JSON.stringify(labelText);
      const valEsc = JSON.stringify(value);
      const filled = await evaluate<boolean>(`(() => {
        const byAria = [...document.querySelectorAll('input[aria-label], textarea[aria-label]')].find(
          el => el.getAttribute('aria-label') === ${labelEsc}
        );
        let input = byAria;
        if (!input) {
          const labels = [...document.querySelectorAll('label')];
          const match = labels.find(l => (l.textContent || '').includes(${labelEsc}));
          if (match) {
            const forId = match.getAttribute('for');
            input = forId ? document.getElementById(forId) : match.querySelector('input, textarea');
          }
        }
        if (!input) return false;
        const proto = input instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
        const setter = Object.getOwnPropertyDescriptor(proto, 'value')?.set;
        setter?.call(input, ${valEsc});
        input.dispatchEvent(new Event('input', { bubbles: true }));
        input.dispatchEvent(new Event('change', { bubbles: true }));
        return true;
      })()`);
      if (!filled) throw new Error(`Input for label not found: ${labelText}`);
    },
    async selectByLabel(labelText: string, value: string) {
      const labelEsc = JSON.stringify(labelText);
      const valEsc = JSON.stringify(value);
      const selected = await evaluate<boolean>(`(() => {
        const labels = [...document.querySelectorAll('label')];
        const match = labels.find(l => (l.textContent || '').includes(${labelEsc}));
        const select = match?.querySelector('select');
        if (!select) return false;
        const setter = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, 'value')?.set;
        setter?.call(select, ${valEsc});
        select.dispatchEvent(new Event('change', { bubbles: true }));
        return true;
      })()`);
      if (!selected) throw new Error(`Select for label not found: ${labelText}`);
    },
    async pressKey(key: string) {
      await sendCdp('Input.dispatchKeyEvent', { type: 'keyDown', key }, sessionId);
      await sendCdp('Input.dispatchKeyEvent', { type: 'keyUp', key }, sessionId);
    },
    async runAxeAudit(): Promise<AxeAuditReport> {
      const report = await evaluate<AxeAuditReport>(IN_PAGE_AXE_SCRIPT);
      const axTree = await sendCdp('Accessibility.getFullAXTree', {}, sessionId);
      const nodes = Array.isArray(axTree.nodes)
        ? (axTree.nodes as Array<{
            ignored?: boolean;
            role?: { value?: string };
            name?: { value?: string };
          }>)
        : [];
      const unlabeledButtons: string[] = [];
      const unlabeledInputs: string[] = [];
      for (const node of nodes) {
        if (node.ignored) continue;
        const role = node.role?.value ?? '';
        const name = (node.name?.value ?? '').trim();
        if (role === 'button' && !name) unlabeledButtons.push(role);
        if ((role === 'textbox' || role === 'combobox' || role === 'searchbox') && !name) {
          unlabeledInputs.push(role);
        }
      }
      report.axTreeSummary = {
        totalNodes: nodes.length,
        unlabeledButtons,
        unlabeledInputs,
      };
      return report;
    },
    async close() {
      pending.clear();
      child.kill('SIGTERM');
      await new Promise<void>((resolveExit) => {
        const timer = setTimeout(() => {
          child.kill('SIGKILL');
          resolveExit();
        }, 2_000);
        child.once('exit', () => {
          clearTimeout(timer);
          resolveExit();
        });
      });
      rmSync(profileDir, { recursive: true, force: true });
    },
  };
}
