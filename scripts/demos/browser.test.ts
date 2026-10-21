import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { test } from 'node:test';
import { launchBrowser } from './browser.ts';
import type { CaptureBrowser } from './browser.ts';

test('capture browser allows live reads and prevents writes and cross-origin requests', async () => {
  const received: string[] = [];
  const server = createServer((request, response) => {
    received.push(`${request.method} ${request.url}`);
    response.end(request.url === '/' ? '<!doctype html><title>Capture guard</title>' : 'read');
  });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const port = () => {
    const address = server.address();
    assert.ok(address && typeof address === 'object', 'test server must listen on TCP');
    return address.port;
  };
  const origin = `http://127.0.0.1:${port()}`;
  let browser: CaptureBrowser | undefined;
  try {
    browser = await launchBrowser({ width: 1440, height: 900, scale: 2, origin });
    await browser.cdp.send('Page.navigate', { url: origin });
    await browser.cdp.wait('document.title === "Capture guard"');
    const result = await browser.cdp.evaluate(`(async () => {
      const read = await fetch('/live-read').then(r => r.text());
      const write = await fetch('/must-not-arrive', {method:'POST',body:'test'}).then(()=>false,()=>true);
      const external = await fetch('http://localhost:${port()}/external').then(()=>false,()=>true);
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
