import { afterEach, expect, it } from 'vitest';
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { launchBrowserHarness, startIsolatedNativeAdmiraltyServer, REPO_ROOT,
  type BrowserHarness, type IsolatedNativeAdmiraltyServer } from './browser-harness';

// Fleet health journey against the real Rust service and production bundle:
// findings from a producer snapshot, then /metrics and the log digest after
// session access, the command palette, theme persistence and a phone viewport.
// Every state must pass the pinned axe audit with zero violations and zero
// incomplete results.

let server: IsolatedNativeAdmiraltyServer | undefined, browser: BrowserHarness | undefined;
let serverStartup: Promise<IsolatedNativeAdmiraltyServer> | undefined;
let browserStartup: Promise<BrowserHarness> | undefined;
let closePromise: Promise<void> | undefined;

function closeOwnedFixture(): Promise<void> {
  return closePromise ??= (async () => {
    const startedBrowser = await browserStartup?.catch(() => undefined);
    try { await (browser ?? startedBrowser)?.close(); } finally {
      const startedServer = await serverStartup?.catch(() => undefined);
      await (server ?? startedServer)?.close();
    }
  })();
}
afterEach(closeOwnedFixture, 90_000);

function count(value: number, lowerBound = false) { return { value, lower_bound: lowerBound }; }

it('ranks findings, reads service metrics and digests, and stays accessible in every state', async () => {
  const observed = new Date(Date.now() - 20_000).toISOString().replace(/\.\d+Z$/, 'Z');
  const source = {
    generated_utc: observed, interval_s: 15, host: 'fixture-node', simulated: false, order: ['harbor'],
    projects: { harbor: { name: 'Harbor', total: 3 } },
    git_health: { harbor: {
      state: 'attention', observed_utc: observed, integration_ref: 'refs/heads/main', divergence_threshold: 20,
      issues: [{ code: 'diverged_branch' }],
      metrics: {
        unmerged_branches: 2, merged_branches: 1, diverged_branches: 1, remote_unmerged_refs: 0,
        max_unmerged_behind: count(140), max_unmerged_ahead: count(3), oldest_unmerged_tip_age_s: 7200,
        dirty_checkouts: 1, conflicted_checkouts: 0, detached_checkouts: 0, cleanup_candidate_checkouts: ['lane-done'],
        integration_tip_age_s: 600, integration_commits_24h: count(4), integration_commits_7d: count(31, true),
        integration_merges_7d: count(0), integration_reverts_7d: count(1), integration_fixups_7d: count(0),
      },
    } },
    global_resources: { hosts: [{
      host: 'build', reachable: true, nproc: 8, load: [3, 2, 1],
      mem: { avail_pct: 40, avail_gb: 12, total_gb: 31, swap_used_gb: 0, swap_total_gb: 8 },
      disks: [{ mount: '/', pct: 88, size: 100e9, avail: 12e9, growth_gb_per_h: 4, hours_to_full: 3, level: 'red' }],
      pressure: { cpu: { some_avg60: 2 }, memory: { full_avg60: 0 }, io: { full_avg60: 1 } }, d_state_procs: 0,
      top_rss: [{ pid: 42, user: 'builder', rss_gb: 8, pcpu: 190, comm: 'rustc' }],
      warnings: [{ level: 'red', text: 'build / 88.0% used, 12 GB free, ~3.0 h to full at 4.0 GB/h' }],
    }] },
  };
  mkdirSync(join(REPO_ROOT, 'target'), { recursive: true });
  const evidence = mkdtempSync(join(REPO_ROOT, 'target', 'fleet-health-evidence-'));
  const axes: { mode: string; violations: unknown[]; incomplete: unknown[] }[] = [];
  const audit = async (mode: string) => {
    const axe = await browser!.runAxeAudit();
    axes.push({ mode, violations: axe.violations, incomplete: axe.incomplete });
    writeFileSync(join(evidence, 'axe.json'), JSON.stringify(axes, null, 2));
    expect(axe.violations, mode).toEqual([]);
    expect(axe.incomplete, mode).toEqual([]);
  };
  let baseUrl = '';
  try {
    serverStartup = startIsolatedNativeAdmiraltyServer({ legacySnapshot: source, legacyPublicRead: true });
    server = await serverStartup;
    baseUrl = server.baseUrl;
    if (process.env.ADMIRALTY_E2E_ADMIRALTYD_BIN) expect(server.binaryPath).toBe(process.env.ADMIRALTY_E2E_ADMIRALTYD_BIN);
    expect((await fetch(`${baseUrl}/readyz`)).status).toBe(200);
    expect((await fetch(`${baseUrl}/metrics`)).status).toBe(401);
    browserStartup = launchBrowserHarness();
    browser = await browserStartup;
    await browser.setViewportSize(1440, 900);

    // 1. Findings from the producer snapshot, without session access.
    await browser.goto(`${baseUrl}/#/health`);
    await browser.waitForText('Needs attention');
    await browser.waitForText('An unmerged lane trails integration by 140 commits.');
    const findings = await browser.evaluate<string[]>(`[...document.querySelectorAll('.health-finding')].map(e => e.textContent.replace(/\\s+/g, ' ').trim())`);
    expect(findings[0]).toMatch(/^Critical/);
    expect(findings.some(text => text.includes('build / 88.0% used'))).toBe(true);
    expect(await browser.evaluate<string | null>(`document.querySelector('.health-git a')?.getAttribute('href') ?? null`)).toBe('#/git?project=harbor');
    expect(await browser.evaluate<boolean>(`!!document.querySelector('.health-meter-projection')`)).toBe(true);
    await browser.waitForText('Request metrics need session access.');
    await browser.captureScreenshot(join(evidence, 'health-dark-public.png'));
    await audit('dark:public');

    // 2. Session access unlocks per-route metrics and the agent digest.
    await browser.clickButtonByText('Add access token');
    await browser.fillInputByLabel('Bearer token', server.ownerToken);
    await browser.clickButtonByText('Apply token');
    await browser.waitForSelector('.health-routes');
    await browser.waitForSelector('.health-digest-text');
    expect(await browser.evaluate<string>(`document.querySelector('.health-digest-text').textContent`)).toMatch(/^ADMIRALTY-LOG\/1 /);
    expect(await browser.evaluate<string>(`document.querySelector('.health-routes').textContent`)).toContain('/metrics');
    await browser.captureScreenshot(join(evidence, 'health-dark-owner.png'));
    await audit('dark:owner');

    // 3. Command palette: modal, labelled, keyboard driven.
    await browser.evaluate(`window.dispatchEvent(new KeyboardEvent('keydown', { key: 'k', ctrlKey: true }))`);
    await browser.waitForSelector('.palette[role="dialog"]');
    await audit('dark:palette');
    await browser.fillInputByLabel('Command', 'git flow');
    await browser.pressKey('Enter');
    await browser.waitForSelector('.git-canvas, .lv-feed', 15_000);
    expect(await browser.evaluate<string>('location.hash')).toMatch(/^#\/git/);
    expect(await browser.evaluate<boolean>(`!document.querySelector('.palette')`)).toBe(true);

    // 4. Light theme persists across a reload.
    await browser.evaluate(`location.hash = '#/health'`);
    await browser.waitForText('Needs attention');
    await browser.clickButtonByAriaLabel('Switch to light theme');
    // A query string forces a new document; the same URL with only a hash is a
    // same-document navigation and would not prove persistence.
    const before = await browser.evaluate<number>('performance.timeOrigin');
    await browser.goto(`${baseUrl}/?reload=theme#/health`);
    expect(await browser.evaluate<number>('performance.timeOrigin')).toBeGreaterThan(before);
    await browser.waitForSelector('.health-git tbody tr', 15_000);
    expect(await browser.evaluate<boolean>(`document.querySelector('.app').classList.contains('theme-light')`)).toBe(true);
    await browser.captureScreenshot(join(evidence, 'health-light.png'));
    await audit('light:public');

    // 5. Phone width: stacked Git cards, no sideways page scroll.
    await browser.setViewportSize(390, 844);
    await browser.waitForSelector('.health-git tbody tr', 15_000);
    expect(await browser.evaluate<boolean>(`document.documentElement.scrollWidth <= document.documentElement.clientWidth`)).toBe(true);
    expect(await browser.evaluate<string>(`getComputedStyle(document.querySelector('.health-git tr')).display`)).toBe('grid');
    await browser.captureScreenshot(join(evidence, 'health-light-390.png'));
    await audit('light:390');

    writeFileSync(join(evidence, 'proof.json'), JSON.stringify({
      recorded_utc: new Date().toISOString(), modes: axes.map(entry => entry.mode), findings,
      scope: 'Real isolated Rust service and production React bundle; synthetic producer observations',
    }, null, 2));
    console.log(`Fleet health proof: ${evidence}`);
  } finally {
    await closeOwnedFixture();
  }
}, 90_000);
