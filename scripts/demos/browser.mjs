// Small, dependency-free CDP client for a disposable capture browser (Node 24+).
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

export async function launchBrowser({ width, height, scale, origin }) {
  const executable = process.env.ADMIRALTY_CAPTURE_CHROME || [
    '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    '/usr/bin/google-chrome', '/usr/bin/chromium', '/usr/bin/chromium-browser',
  ].find(existsSync);
  if (!executable) throw new Error('Chrome not found; set ADMIRALTY_CAPTURE_CHROME to its executable.');
  const profile = await mkdtemp(join(tmpdir(), 'admiralty-capture-'));
  const child = spawn(executable, [
    '--headless=new', '--remote-debugging-port=0', `--user-data-dir=${profile}`,
    '--no-first-run', '--no-default-browser-check', '--disable-background-networking',
    '--disable-component-update', '--disable-sync', '--disable-extensions',
    '--force-color-profile=srgb', '--hide-scrollbars', '--window-size=1440,900', 'about:blank',
  ], { stdio: 'ignore' });
  let launchError;
  child.on('error', error => { launchError = error; });
  let cdp;
  const close = async () => {
    if (cdp) await cdp.send('Browser.close').catch(() => {});
    if (child.exitCode === null && !child.signalCode) {
      await Promise.race([new Promise(resolve => child.once('exit', resolve)), delay(2000)]);
      if (child.exitCode === null && !child.signalCode) child.kill('SIGKILL');
    }
    await rm(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
  };
  try {
    let port;
    for (let attempt = 0; attempt < 150; attempt++) {
      if (launchError) throw launchError;
      if (child.exitCode !== null) throw new Error('Capture browser exited during startup.');
      try { port = Number((await readFile(join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0]); } catch {}
      if (port) break;
      await delay(100);
    }
    if (!port) throw new Error('Capture browser startup timed out.');
    const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
    const target = targets.find(item => item.type === 'page');
    cdp = await connect(target.webSocketDebuggerUrl);
    const blocked = [], requests = new Set();
    cdp.on('Fetch.requestPaused', async ({ requestId, request }) => {
      const url = new URL(request.url);
      if (url.origin === origin && ['GET', 'HEAD'].includes(request.method)) {
        requests.add(`${request.method} ${url.pathname}`);
        await cdp.send('Fetch.continueRequest', { requestId });
      } else {
        blocked.push(`${request.method} ${url.origin}${url.pathname}`);
        await cdp.send('Fetch.failRequest', { requestId, errorReason: 'BlockedByClient' });
      }
    });
    await cdp.send('Fetch.enable', { patterns: [{ urlPattern: '*' }] });
    await cdp.send('Page.enable');
    await cdp.send('Runtime.enable');
    await cdp.send('Emulation.setDeviceMetricsOverride', {
      width, height, deviceScaleFactor: scale, mobile: false,
    });
    // Match normal dark desktop rendering; preserve application motion and source updates.
    await cdp.send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: 'dark' }] });
    return { cdp, close, blocked, requests };
  } catch (error) { await close(); throw error; }
}

async function connect(url) {
  const socket = new WebSocket(url), pending = new Map(), handlers = new Map();
  let sequence = 0;
  await new Promise((resolve, reject) => {
    socket.addEventListener('open', resolve, { once: true });
    socket.addEventListener('error', reject, { once: true });
  });
  socket.addEventListener('message', ({ data }) => {
    const message = JSON.parse(data);
    if (message.id) {
      const task = pending.get(message.id);
      if (!task) return;
      clearTimeout(task.timer); pending.delete(message.id);
      message.error ? task.reject(new Error(message.error.message)) : task.resolve(message.result);
    } else {
      for (const handler of handlers.get(message.method) ?? []) {
        // Fetch interception errors must fail the next command, not leak an unhandled rejection.
        Promise.resolve(handler(message.params)).catch(error => { connection.error = error; });
      }
    }
  });
  socket.addEventListener('close', () => {
    for (const task of pending.values()) { clearTimeout(task.timer); task.reject(new Error('CDP connection closed.')); }
    pending.clear();
  });
  const connection = {
    error: null,
    on(name, handler) { handlers.set(name, [...(handlers.get(name) ?? []), handler]); },
    send(method, params = {}) {
      if (connection.error) return Promise.reject(connection.error);
      return new Promise((resolve, reject) => {
        const id = ++sequence;
        const timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timeout: ${method}`)); }, 20000);
        pending.set(id, { resolve, reject, timer });
        socket.send(JSON.stringify({ id, method, params }));
      });
    },
    async evaluate(expression) {
      const result = await connection.send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
      if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
      return result.result.value;
    },
    async wait(expression, description = expression) {
      const deadline = Date.now() + 30000;
      while (Date.now() < deadline) {
        if (await connection.evaluate(expression)) return;
        await delay(100);
      }
      throw new Error(`Page readiness timed out: ${description}`);
    },
  };
  return connection;
}
