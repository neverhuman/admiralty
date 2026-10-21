import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { launchBrowserHarness, startIsolatedNativeAdmiraltyServer, type BrowserHarness, type IsolatedNativeAdmiraltyServer } from './browser-harness';

/** Synthetic producer snapshot with the fields the observer now publishes. */
function producerSnapshot(generated: string) {
  return {
    generated_utc: generated, interval_s: 15, host: 'fixture-node', order: ['dope', 'joil', 'quiet'],
    projects: {
      dope: { name: 'DOPE', total: 65, done: 61, counts: { in_progress: 0, blocked: 4 } },
      joil: { name: 'Joil', total: 160, done: 160, counts: { in_progress: 0, blocked: 0 } },
      quiet: { name: 'Quiet', total: 3, done: 1, counts: { in_progress: 2, blocked: 0 } },
    },
    collector: { implementation: 'rust', duration_ms: 1834, model_turns: 0,
      stages_ms: { fleet: 420, projects: 300, git: 900, git_health: 3, usage: 150, history: 20 } },
    git_health: {
      dope: { state: 'attention', actionable: true, fingerprint: 'sha256:3f2a91c4ffffffff', integration_ref: 'refs/heads/main',
        issues: [{ code: 'merge_conflicts' }, { code: 'diverged_branch' }] },
      joil: { state: 'unknown', actionable: true, fingerprint: 'sha256:aaaabbbb', issues: [{ code: 'shallow_history' }] },
    },
    git_metrics: {
      dope: { complete: true, integration_ref: 'refs/heads/main',
        branches: { compared: 14, merged: 9, ahead_only: 3, diverged: 2, unknown: 0, unmerged_tips_older_30d: 1, unintegrated_commits: { value: 7, lower_bound: false } },
        checkouts: { total: 6, dirty: 2, state_unknown: 1, with_conflicts: 1, detached: 0 },
        integration: { tip_age_seconds: 10800 },
        activity: [{ window_hours: 24, integration_first_parent: { value: 4, lower_bound: false } },
          { window_hours: 168, integration_first_parent: { value: 19, lower_bound: true } }] },
    },
    global_resources: { warnings: [{ level: 'amber', kind: 'disk', text: 'fixture-node disk / 91.2% used, ~11.4 h to full at 2.10 GB/h' }],
      hosts: [{ host: 'fixture-node', reachable: true, nproc: 100, load: [55.1, 1, 1], load_level: 'ok',
        mem: { avail_pct: 61, level: 'ok', swap_total_gb: 8, swap_used_gb: 2 },
        disks: [{ mount: '/', pct: 91.2, level: 'amber', growth_gb_per_h: 2.1, hours_to_full: 11.4 }], gpus: [{ util: 43 }] },
      { host: 'fixture-remote', reachable: false }] },
  };
}

describe('Fleet health against the real admiraltyd', () => {
  let browser: BrowserHarness;
  let server: IsolatedNativeAdmiraltyServer;

  beforeAll(async () => {
    const generated = new Date().toISOString().replace(/\.\d+Z$/, 'Z');
    server = await startIsolatedNativeAdmiraltyServer({ legacySnapshot: producerSnapshot(generated), legacyPublicRead: true });
    browser = await launchBrowserHarness();
  }, 90_000);

  afterAll(async () => {
    await browser?.close();
    await server?.close();
  });

  it('renders hygiene, hosts, cycle lanes and the real agent brief, and passes axe in both themes and at phone width', async () => {
    await browser.goto(`${server.baseUrl}/#/health`);
    await browser.waitForText('Repository hygiene');
    await browser.waitForText('Needs attention');
    await browser.waitForText('7 commits not in integration');
    await browser.waitForText('fixture-remote');
    // The brief comes from the daemon's /v1/brief, not from the browser.
    await browser.waitForText('# fleet gen=');
    const brief = await browser.evaluate<string>(`document.querySelector('.health-brief-text')?.textContent ?? ''`);
    expect(brief).toContain('! git dope attention diverged_branch,merge_conflicts fp=3f2a91c4');
    expect(brief).toContain('! host fixture-remote unreachable');
    expect(brief).toContain('proj dope done=61/65 ip=0 blk=4');
    expect(await browser.evaluate<string>(`document.body.innerText`)).not.toContain('NaN');

    for (const theme of ['theme-dark', 'theme-light']) {
      await browser.evaluate(`document.querySelector('.app').classList.remove('theme-dark', 'theme-light'); document.querySelector('.app').classList.add('${theme}')`);
      const audit = await browser.runAxeAudit();
      expect(audit.violations).toEqual([]);
      expect(audit.incomplete).toEqual([]);
      await browser.captureScreenshot(`../../target/cutover-evidence/fleet-health-desktop-${theme}.png`);
    }

    // Keyboard palette: Ctrl+K, filter, Enter navigates.
    await browser.evaluate(`window.dispatchEvent(new KeyboardEvent('keydown', { key: 'k', ctrlKey: true, bubbles: true }))`);
    await browser.waitForText('Arrow keys to move, Enter to open, Escape to close.');
    const paletteAudit = await browser.runAxeAudit();
    expect(paletteAudit.violations).toEqual([]);
    await browser.captureScreenshot('../../target/cutover-evidence/fleet-health-palette.png');
    await browser.fillInputByLabel('Go to', 'git flow');
    await browser.pressKey('Enter');
    for (let attempt = 0; attempt < 50 && (await browser.evaluate<string>(`location.hash`)) !== '#/git'; attempt += 1) {
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    expect(await browser.evaluate<string>(`location.hash`)).toBe('#/git');

    await browser.goto(`${server.baseUrl}/#/health`);
    await browser.waitForText('Repository hygiene');
    await browser.setViewportSize(390, 844);
    const mobile = await browser.runAxeAudit();
    expect(mobile.violations).toEqual([]);
    expect(mobile.incomplete).toEqual([]);
    expect(await browser.evaluate<boolean>(`document.documentElement.scrollWidth <= window.innerWidth`)).toBe(true);
    await browser.captureScreenshot('../../target/cutover-evidence/fleet-health-mobile.png');
  }, 90_000);
});
