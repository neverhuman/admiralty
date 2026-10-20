// Small, dependency-free CDP client for a disposable capture browser (Node 24+).
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

const COMMAND_TIMEOUT_MS = 20_000;
const CLOSE_COMMAND_TIMEOUT_MS = 750;
const GRACEFUL_EXIT_MS = 1_000;
const TERM_GRACE_MS = 1_000;
const KILL_REAP_MS = 3_000;

function observedExit(child, exitObserved) {
  return exitObserved() || typeof child.exitCode === 'number' || typeof child.signalCode === 'string';
}

function waitForExit(child, isExited, timeoutMs) {
  if (isExited()) return Promise.resolve(true);
  return new Promise(resolve => {
    let done = false;
    const finish = value => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      child.removeListener('exit', onExit);
      resolve(value || isExited());
    };
    const onExit = () => finish(true);
    const timer = setTimeout(() => finish(false), timeoutMs);
    child.once('exit', onExit);
    if (isExited()) finish(true);
  });
}

function processGroupExists(pid) {
  try {
    process.kill(-pid, 0);
    return true;
  } catch (error) {
    if (error?.code === 'ESRCH') return false;
    if (error?.code === 'EPERM') return true;
    throw error;
  }
}

function signalOwnedProcess(child, pid, platform, signal, exitObserved) {
  if (observedExit(child, exitObserved)) return false;
  if (platform !== 'win32' && Number.isInteger(pid) && pid > 0) {
    try {
      process.kill(-pid, signal);
      return true;
    } catch (error) {
      if (error?.code === 'ESRCH') return false;
      throw error;
    }
  }
  try {
    return child.kill(signal);
  } catch (error) {
    if (error?.code === 'ESRCH') return false;
    throw error;
  }
}

/** Narrow adapter seam for finite lifecycle tests; launchBrowser uses OS defaults. */
export function createCleanupLifecycle({
  child, profile, cdp = null, spawnFailedWithoutPid = false, platform = process.platform, adapter = {},
}) {
  let exitSeen = false;
  child.once?.('exit', () => { exitSeen = true; });
  const pid = child.pid;
  const isExited = () => observedExit(child, () => exitSeen);
  const wait = adapter.waitForExit ?? ((timeoutMs) => waitForExit(child, isExited, timeoutMs));
  const signal = adapter.signalOwned ?? (sig => signalOwnedProcess(child, pid, platform, sig, () => exitSeen));
  const groupExists = adapter.groupExists ?? (() => platform !== 'win32' && Number.isInteger(pid) && pid > 0
    ? processGroupExists(pid) : false);
  const removeProfile = adapter.removeProfile ?? (path => rm(path, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 }));
  let activeCdp = cdp;
  let cleanupPromise;

  const cleanup = async () => {
    const errors = [];
    let browserCloseError;
    const noPidSpawnFailure = typeof spawnFailedWithoutPid === 'function' ? spawnFailedWithoutPid() : spawnFailedWithoutPid;
    if (!pid && !noPidSpawnFailure) {
      throw new Error(`Browser child PID was never assigned; private profile retained at ${profile}.`);
    }
    if (pid && activeCdp && !isExited()) {
      try {
        await activeCdp.send('Browser.close', {}, CLOSE_COMMAND_TIMEOUT_MS);
      } catch (error) {
        browserCloseError = error;
      }
    }
    if (activeCdp) {
      try { await activeCdp.dispose('Browser cleanup started.'); }
      catch (error) { errors.push(new Error(`CDP disposal failed: ${error.message}`, { cause: error })); }
    }

    if (pid && !isExited() && !await wait(GRACEFUL_EXIT_MS)) {
      try { await signal('SIGTERM'); }
      catch (error) { errors.push(new Error(`Could not signal owned browser group with SIGTERM: ${error.message}`, { cause: error })); }
      if (!isExited() && !await wait(TERM_GRACE_MS)) {
        if (!isExited()) {
          try { await signal('SIGKILL'); }
          catch (error) { errors.push(new Error(`Could not signal owned browser group with SIGKILL: ${error.message}`, { cause: error })); }
        }
        if (!isExited()) await wait(KILL_REAP_MS);
      }
    }

    if (pid && !isExited()) {
      throw new Error(`Browser child exit was not observed; private profile retained at ${profile}.`, {
        cause: errors.length ? new AggregateError(errors, 'Browser cleanup errors') : undefined,
      });
    }
    if (pid && platform !== 'win32') {
      let remains;
      try { remains = await groupExists(); }
      catch (error) {
        throw new Error(`Could not confirm owned browser process group is gone; private profile retained at ${profile}.`, { cause: error });
      }
      if (remains) {
        throw new Error(`Owned browser process group remains after direct-child reaping; private profile retained at ${profile}.`);
      }
    }

    // Chrome may close the CDP socket before replying to Browser.close; that
    // disconnect is expected only when the owned direct child actually exited.
    if (browserCloseError && !(isExited() && browserCloseError.message === 'CDP connection closed.')) {
      errors.push(new Error(`CDP Browser.close failed: ${browserCloseError.message}`, { cause: browserCloseError }));
    }

    try { await removeProfile(profile); }
    catch (error) { errors.push(new Error(`Could not remove private browser profile ${profile}: ${error.message}`, { cause: error })); }
    if (errors.length) throw new AggregateError(errors, 'Browser custody was released, but cleanup reported errors.');
  };

  return {
    setCdp(connection) { activeCdp = connection; },
    close() {
      cleanupPromise ??= cleanup();
      return cleanupPromise;
    },
  };
}

export async function launchBrowser({ width, height, scale, origin }) {
  const executable = process.env.ADMIRALTY_CAPTURE_CHROME || [
    '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    '/usr/bin/google-chrome', '/usr/bin/chromium', '/usr/bin/chromium-browser',
  ].find(existsSync);
  if (!executable) throw new Error('Chrome not found; set ADMIRALTY_CAPTURE_CHROME to its executable.');
  const profile = await mkdtemp(join(tmpdir(), 'admiralty-capture-'));
  let child;
  try {
    child = spawn(executable, [
      '--headless=new', '--remote-debugging-port=0', `--user-data-dir=${profile}`,
      '--no-first-run', '--no-default-browser-check', '--disable-background-networking',
      '--disable-component-update', '--disable-sync', '--disable-extensions',
      '--force-color-profile=srgb', '--hide-scrollbars', '--window-size=1440,900', 'about:blank',
    ], { stdio: 'ignore', detached: process.platform !== 'win32' });
  } catch (error) {
    await rm(profile, { recursive: true, force: true });
    throw error;
  }
  let launchError;
  let exitObserved = false;
  child.once('error', error => { launchError = error; });
  child.once('exit', () => { exitObserved = true; });
  let cdp;
  const cleanup = createCleanupLifecycle({
    child, profile, spawnFailedWithoutPid: () => !child.pid && Boolean(launchError),
    platform: process.platform,
  });
  const close = () => cleanup.close();
  try {
    let port;
    for (let attempt = 0; attempt < 150; attempt++) {
      if (launchError) throw launchError;
      if (exitObserved || typeof child.exitCode === 'number' || typeof child.signalCode === 'string') {
        throw new Error('Capture browser exited during startup.');
      }
      try { port = Number((await readFile(join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0]); } catch {}
      if (port) break;
      await delay(100);
    }
    if (!port) throw new Error('Capture browser startup timed out.');
    const discovery = await fetch(`http://127.0.0.1:${port}/json/list`, { signal: AbortSignal.timeout(2_000) });
    if (!discovery.ok) throw new Error(`Capture browser CDP discovery failed (${discovery.status}).`);
    const targets = await discovery.json();
    const target = targets.find(item => item.type === 'page');
    if (!target?.webSocketDebuggerUrl) throw new Error('Capture browser did not publish a page CDP target.');
    cdp = await connect(target.webSocketDebuggerUrl);
    cleanup.setCdp(cdp);
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
    await cdp.send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: scale, mobile: false });
    await cdp.send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: 'dark' }] });
    return { cdp, close, blocked, requests };
  } catch (error) {
    try { await close(); }
    catch (cleanupError) { throw new AggregateError([error, cleanupError], 'Browser startup failed and cleanup reported an error.'); }
    throw error;
  }
}

export function connectCdpForTest(url, WebSocketConstructor) {
  return connect(url, WebSocketConstructor);
}

async function connect(url, WebSocketConstructor = WebSocket) {
  const socket = new WebSocketConstructor(url), pending = new Map(), handlers = new Map();
  let sequence = 0, connectionError = null, disposed = false;
  const failPending = error => {
    connectionError ??= error;
    for (const [id, task] of pending) {
      clearTimeout(task.timer);
      pending.delete(id);
      task.reject(error);
    }
  };
  const connection = {
    get error() { return connectionError; },
    on(name, handler) { handlers.set(name, [...(handlers.get(name) ?? []), handler]); },
    send(method, params = {}, timeoutMs = COMMAND_TIMEOUT_MS) {
      if (connectionError || disposed) return Promise.reject(connectionError ?? new Error('CDP connection is closed.'));
      return new Promise((resolve, reject) => {
        const id = ++sequence;
        const timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timeout: ${method}`)); }, timeoutMs);
        pending.set(id, { resolve, reject, timer });
        try { socket.send(JSON.stringify({ id, method, params })); }
        catch (error) { clearTimeout(timer); pending.delete(id); reject(error); }
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
    dispose(reason = 'CDP connection disposed.') {
      if (disposed) return Promise.resolve();
      disposed = true;
      failPending(new Error(reason));
      try { socket.close(); }
      catch (error) { return Promise.reject(error); }
      return Promise.resolve();
    },
  };
  socket.addEventListener('message', ({ data }) => {
    let message;
    try { message = JSON.parse(data); }
    catch (error) { failPending(new Error(`Invalid CDP message: ${error.message}`, { cause: error })); return; }
    if (message.id) {
      const task = pending.get(message.id);
      if (!task) return;
      clearTimeout(task.timer); pending.delete(message.id);
      message.error ? task.reject(new Error(message.error.message)) : task.resolve(message.result);
    } else {
      for (const handler of handlers.get(message.method) ?? []) {
        Promise.resolve().then(() => handler(message.params)).catch(error => {
          const failure = new Error(`CDP event handler failed: ${error.message}`, { cause: error });
          failPending(failure);
        });
      }
    }
  });
  socket.addEventListener('error', () => failPending(new Error('CDP WebSocket error.')));
  socket.addEventListener('close', () => failPending(new Error('CDP connection closed.')));
  try {
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('CDP WebSocket open timed out.')), 2_000);
      socket.addEventListener('open', () => { clearTimeout(timer); resolve(); }, { once: true });
      socket.addEventListener('error', () => { clearTimeout(timer); reject(connectionError); }, { once: true });
      socket.addEventListener('close', () => { clearTimeout(timer); reject(connectionError ?? new Error('CDP WebSocket closed before opening.')); }, { once: true });
    });
  } catch (error) {
    try { await connection.dispose(`CDP connection setup failed: ${error.message}`); }
    catch (disposeError) { throw new AggregateError([error, disposeError], 'CDP connection setup and disposal failed.'); }
    throw error;
  }
  return connection;
}
