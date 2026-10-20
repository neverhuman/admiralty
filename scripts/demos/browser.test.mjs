import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createServer } from 'node:http';
import { mkdtemp, rm, stat } from 'node:fs/promises';
import { EventEmitter, once } from 'node:events';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { connectCdpForTest, createCleanupLifecycle, launchBrowser } from './browser.mjs';

const bounded = promise => new Promise((resolve, reject) => {
  const timer = setTimeout(() => reject(new Error('test liveness deadline exceeded')), 8_000);
  Promise.resolve(promise).then(
    value => { clearTimeout(timer); resolve(value); },
    error => { clearTimeout(timer); reject(error); },
  );
});

class FakeChild extends EventEmitter {
  constructor(pid = 12345) { super(); this.pid = pid; this.exitCode = null; this.signalCode = null; }
  kill() { return true; }
}

class FakeWebSocket extends EventTarget {
  constructor(url) {
    super();
    this.url = url;
    this.closed = false;
    FakeWebSocket.latest = this;
    setImmediate(() => this.dispatchEvent(new Event('open')));
  }
  send() {}
  close() {
    this.closed = true;
    this.dispatchEvent(new Event('close'));
  }
}

test('capture browser allows live reads and prevents writes and cross-origin requests', async () => {
  const received = [];
  const server = createServer((request, response) => {
    received.push(`${request.method} ${request.url}`);
    response.end(request.url === '/' ? '<!doctype html><title>Capture guard</title>' : 'read');
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const origin = `http://127.0.0.1:${server.address().port}`;
  let browser;
  try {
    browser = await launchBrowser({ width: 1440, height: 900, scale: 2, origin });
    await browser.cdp.send('Page.navigate', { url: origin });
    await browser.cdp.wait('document.title === "Capture guard"');
    const result = await browser.cdp.evaluate(`(async () => {
      const read = await fetch('/live-read').then(r => r.text());
      const write = await fetch('/must-not-arrive', {method:'POST',body:'test'}).then(()=>false,()=>true);
      const external = await fetch('http://localhost:${server.address().port}/external').then(()=>false,()=>true);
      return { read, write, external };
    })()`);
    assert.deepEqual(result, { read: 'read', write: true, external: true });
    assert(received.includes('GET /live-read'));
    assert(!received.some(request => request.includes('must-not-arrive') || request.includes('external')));
    assert.equal(browser.blocked.length, 2);
  } finally {
    await browser?.close();
    await new Promise(resolve => server.close(resolve));
  }
});

test('close is memoized across concurrent calls and accepts an observed graceful child exit', async () => {
  const profile = '/private/capture-profile';
  const child = new FakeChild();
  let removals = 0, closes = 0, disposals = 0;
  const lifecycle = createCleanupLifecycle({
    child, profile, platform: 'win32',
    cdp: {
      async send(method) {
        assert.equal(method, 'Browser.close');
        closes += 1;
        child.exitCode = 0;
        child.emit('exit', 0, null);
      },
      async dispose() { disposals += 1; },
    },
    adapter: { removeProfile: async value => { assert.equal(value, profile); removals += 1; } },
  });
  const first = lifecycle.close(), second = lifecycle.close();
  assert.equal(first, second);
  await bounded(Promise.all([first, second]));
  assert.equal(closes, 1);
  assert.equal(disposals, 1);
  assert.equal(removals, 1);
});

test('CDP disposal closes its socket and rejects pending commands without retaining timers', async () => {
  const cdp = await bounded(connectCdpForTest('ws://127.0.0.1/devtools', FakeWebSocket));
  const socket = FakeWebSocket.latest;
  const pending = cdp.send('Runtime.evaluate', {}, 20_000);
  const rejected = assert.rejects(pending, /test disposal/);
  await bounded(cdp.dispose('test disposal'));
  await rejected;
  assert.equal(socket.closed, true);
});

test('TERM-ignoring finite owned process is killed and reaped before profile removal', async () => {
  const profile = await mkdtemp(join(tmpdir(), 'capture-browser-test-'));
  const child = spawn(process.execPath, ['-e', "process.on('SIGTERM',()=>{}); setInterval(()=>{},1000); setTimeout(()=>process.exit(0),6000)"], {
    detached: true, stdio: 'ignore',
  });
  let exitObserved = false;
  child.once('exit', () => { exitObserved = true; });
  const lifecycle = createCleanupLifecycle({ child, profile, platform: process.platform });
  try {
    await bounded(lifecycle.close());
    assert.equal(exitObserved, true);
    assert.equal(child.signalCode, 'SIGKILL');
    await assert.rejects(stat(profile), { code: 'ENOENT' });
  } finally {
    if (!exitObserved && Number.isInteger(child.pid)) {
      const exited = once(child, 'exit');
      try { process.kill(-child.pid, 'SIGKILL'); } catch (error) { if (error.code !== 'ESRCH') throw error; }
      await bounded(exited);
    }
    if (exitObserved) await rm(profile, { recursive: true, force: true });
  }
});

test('unconfirmed direct-child and group custody retains its private profile', async () => {
  const profile = await mkdtemp(join(tmpdir(), 'capture-browser-unconfirmed-'));
  const child = new FakeChild();
  const signals = [];
  let removed = false;
  const lifecycle = createCleanupLifecycle({
    child, profile, platform: 'linux',
    adapter: {
      waitForExit: async () => false,
      signalOwned: async signal => { signals.push(signal); },
      groupExists: async () => true,
      removeProfile: async () => { removed = true; },
    },
  });
  try {
    await assert.rejects(bounded(lifecycle.close()), /exit was not observed; private profile retained/);
    assert.deepEqual(signals, ['SIGTERM', 'SIGKILL']);
    assert.equal(removed, false);
    assert.equal((await stat(profile)).isDirectory(), true);
  } finally {
    await rm(profile, { recursive: true, force: true });
  }
});

test('spawn failure without an assigned PID disposes partial CDP state and removes the profile', async () => {
  const profile = '/private/failed-start-profile';
  const child = new FakeChild(null);
  let disposed = 0, removed = 0;
  const lifecycle = createCleanupLifecycle({
    child, profile, spawnFailedWithoutPid: true, platform: 'linux',
    cdp: { async send() { throw new Error('unexpected'); }, async dispose() { disposed += 1; } },
    adapter: { removeProfile: async () => { removed += 1; } },
  });
  await bounded(lifecycle.close());
  assert.equal(disposed, 1);
  assert.equal(removed, 1);
});
