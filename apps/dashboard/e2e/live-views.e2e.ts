import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import {
  mkdirSync,
  writeFileSync,
  mkdtempSync,
  chmodSync,
  readFileSync,
  rmSync,
  realpathSync,
} from "node:fs";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import {
  launchBrowserHarness,
  startIsolatedNativeAdmiraltyServer,
  REPO_ROOT,
  type BrowserHarness,
  type IsolatedNativeAdmiraltyServer,
} from "./browser-harness";

function sourceFixture(count = 8) {
  const stamp = new Date().toISOString(),
    oid = (n: number) => n.toString(16).padStart(40, "0");
  return {
    generated_utc: stamp,
    host: "fixture-node",
    order: ["harbor", "observatory"],
    projects: {
      harbor: { name: "Harbor" },
      observatory: { name: "Observatory" },
    },
    hierarchy: {
      owner: "Fixture Owner",
      grokbot: { name: "Grokbot" },
      super_captain: {
        id: "11.admiral",
        name: "admiral",
        host: "fixture-node",
        agent_running: true,
        model: "gpt-6-sol",
      },
      captains: [
        {
          key: "harbor",
          screen: "12.harbor_manager",
          host: "fixture-node",
          agent_running: true,
          model: "gpt-6-sol",
          workers: [],
        },
        {
          key: "observatory",
          screen: "13.observatory_manager",
          host: "fixture-node",
          agent_running: true,
          model: "gpt-6-sol",
          workers: [],
        },
      ],
    },
    agents: Array.from({ length: count }, (_, i) => ({
      id: `${100 + i}.worker_${i}`,
      screen: `worker_${i}`,
      project: i % 2 ? "observatory" : "harbor",
      host: "fixture-node",
      model: i % 2 ? "claude-opus" : "gpt-6-luna",
      state: i === 2 ? "blocked" : "active",
      branch: i === 0 ? "worker/ui" : null,
    })),
    global_resources: {
      hosts: [
        {
          host: "fixture-node",
          reachable: true,
          screen_inventory_available: true,
        },
      ],
    },
    git: {
      harbor: {
        instance_id: "fixture-repo",
        node: "fixture-node",
        observed_utc: stamp,
        shallow: false,
        truncated: false,
        commits: [
          {
            oid: oid(1),
            parents: [],
            subject: "Root commit",
            committed_utc: stamp,
          },
          {
            oid: oid(2),
            parents: [oid(1)],
            subject: "Worker change",
            committed_utc: stamp,
          },
          {
            oid: oid(3),
            parents: [oid(1)],
            subject: "Main change",
            committed_utc: stamp,
          },
          {
            oid: oid(4),
            parents: [oid(3), oid(2)],
            subject: "Merge worker",
            committed_utc: stamp,
          },
        ],
        refs: [
          {
            name: "refs/heads/main",
            oid: oid(4),
            commit_oid: oid(4),
            kind: "branch",
          },
          {
            name: "refs/heads/worker/ui",
            oid: oid(2),
            commit_oid: oid(2),
            kind: "branch",
          },
        ],
        checkouts: [
          {
            id: "fixture-checkout",
            name: "harbor-worker",
            kind: "linked",
            head: oid(2),
            branch: "worker/ui",
            detached: false,
            locked: false,
            prunable: false,
            status: { staged: 1, unstaged: 2, untracked: 0, conflicts: 0 },
          },
        ],
      },
    },
  };
}

describe("built live views against the real Rust server", () => {
  let browser: BrowserHarness, server: IsolatedNativeAdmiraltyServer;
  const evidence = join(REPO_ROOT, "target", "live-views-evidence");
  beforeAll(async () => {
    mkdirSync(evidence, { recursive: true });
    server = await startIsolatedNativeAdmiraltyServer({
      legacySnapshot: sourceFixture(),
    });
    browser = await launchBrowserHarness();
    await browser.setViewportSize(1440, 1000);
  }, 30_000);
  afterAll(async () => {
    await browser?.close();
    await server?.close();
  });
  beforeEach(async () => {
    await browser.resetPage();
  });
  async function connect(route: string, source = server) {
    await browser.goto(`${source.baseUrl}/#/${route}`);
    await browser.waitForSelector(".lv-feed.is-connected, .lv-error button");
    const openedAccess = await browser.evaluate(`(() => {
      if (document.querySelector('.lv-feed.is-connected')) return false;
      const button = document.querySelector('.lv-error button');
      if (!(button instanceof HTMLButtonElement)) throw Error('Native access control unavailable');
      button.click();
      return true;
    })()`);
    if (openedAccess) {
      await browser.fillInputByLabel("Bearer token", source.ownerToken);
      await browser.clickButtonByText("Apply token");
    }
    await browser.waitForSelector(".lv-feed.is-connected");
  }
  it("joins all roles and projects, supports search/selection/cross-links and browser history", async () => {
    await connect("atlas");
    await browser.waitForSelector(".atlas-agent");
    await browser.evaluate(
      "new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))",
    );
    expect(
      await browser.evaluate(
        `(()=>{const el=document.querySelector('.atlas-viewport');const r=el.getBoundingClientRect();const event=new WheelEvent('wheel',{ctrlKey:true,deltaY:20,clientX:r.left+r.width/2,clientY:r.top+r.height/2,bubbles:true,cancelable:true});el.dispatchEvent(event);return event.defaultPrevented;})()`,
      ),
    ).toBe(true);

    expect(
      await browser.evaluate(
        `document.querySelector('.atlas-summary')?.textContent`,
      ),
    ).toContain("2projects");
    expect(
      await browser.evaluate(
        `Array.from(document.querySelectorAll('.atlas-agent:not(.compact-mark) > strong')).filter(name => {
          const card = name.parentElement, box = name.getBoundingClientRect();
          const scale = card.getBoundingClientRect().width / card.offsetWidth;
          return box.height < parseFloat(getComputedStyle(name).fontSize) * scale * 0.95;
        }).map(name => name.textContent)`,
      ),
      "Agent names must retain their full text height at the fitted camera scale",
    ).toEqual([]);
    await browser.captureScreenshot(join(evidence, "fleet-atlas.png"));
    await browser.fillInputByLabel("Search all agents", "worker_0");
    await browser.clickButtonByText("Roster");
    await browser.waitForSelector(".atlas-roster");
    expect(
      await browser.evaluate(
        `document.querySelectorAll('.atlas-roster tbody tr').length`,
      ),
    ).toBe(1);
    await browser.clickButtonByText("worker_0");
    await browser.waitForSelector(".lv-inspector");
    expect(await browser.evaluate(`location.hash`)).toContain("focus=screen");
    await browser.evaluate(`document.querySelector('.lv-inspector a').click()`);
    await browser.waitForText("Git Flow");
    await browser.waitForSelector(".git-canvas");
    expect(await browser.evaluate(`location.hash`)).toContain("project=harbor");
    await browser.evaluate(`history.back()`);
    await browser.waitForSelector(".atlas-roster");
    expect(await browser.evaluate(`location.hash`)).toContain("focus=screen");
  });
  it("renders exact Git ancestry and checkout details, paused inspection and full-window mode", async () => {
    await browser.goto(`${server.baseUrl}/#/git`);
    await browser.waitForText("Session access");
    const sessionButtonText = await browser.evaluate<string>(`(() => {
      const btn = Array.from(document.querySelectorAll('button')).find(b => {
        const t = (b.textContent ?? '').trim();
        return t === 'Session access' || t === 'Connect session access';
      });
      return btn ? (btn.textContent ?? '').trim() : 'Session access';
    })()`);
    await browser.clickButtonByText(sessionButtonText);
    await browser.fillInputByLabel("Bearer token", server.ownerToken);
    await browser.clickButtonByText("Apply token");
    await browser.waitForSelector(".git-canvas");
    await browser.waitForSelector(".lv-feed.is-connected");
    await browser.evaluate(
      "new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))",
    );
    expect(
      await browser.evaluate(
        `document.querySelector('.lv-error')?.textContent??''`,
      ),
    ).toBe("");
    await browser.captureScreenshot(join(evidence, "git-flow.png"));
    await browser.clickButtonByText("Inventory");
    await browser.waitForText("Merge worker");
    expect(
      await browser.evaluate(
        `document.querySelectorAll('.git-inventory tbody tr').length`,
      ),
    ).toBe(4);
    await browser.clickButtonByText("harbor-worker");
    await browser.waitForSelector(".lv-inspector");
    await browser.waitForText("1 staged · 2 unstaged");
    expect(
      await browser.evaluate(
        "document.querySelectorAll('.git-inventory-checkouts section').length",
      ),
    ).toBe(2);
    await browser.clickButtonByAriaLabel("Close inspector");
    await browser.clickButtonByText("Pause");
    await browser.waitForText("Historical inspection paused");
    const changed = sourceFixture();
    changed.git.harbor.checkouts = [];
    changed.git.harbor.commits = [];
    server.writeLegacySnapshot(changed);
    await browser.waitForText("new source cuts", 10_000);
    await browser.clickButtonByText("harbor-worker");
    await browser.waitForSelector(".lv-inspector");
    await browser.waitForText("1 staged · 2 unstaged");
    await browser.clickButtonByAriaLabel("Close inspector");
    await browser.clickButtonByText("Resume live");
    await browser.clickButtonByAriaLabel("Enter fullscreen");
    expect(
      await browser.evaluate(
        `document.querySelector('.live-workspace')?.classList.contains('is-fullscreen')`,
      ),
    ).toBe(true);
    await browser.pressKey("Escape");
  });
  it.each([2, 3])("collapses %i linear interiors beside a real fork and preserves focus and inventory", async (count) => {
    const source = sourceFixture(), repo = source.git.harbor, template = repo.commits[0];
    const oid = (n: number) => ((n * 2654435761) >>> 0).toString(16).padStart(8, "0").padEnd(40, "0");
    repo.commits = Array.from({ length: count + 2 }, (_, i) => ({
      ...template, oid: oid(i + 1), parents: i ? [oid(i)] : [], subject: `Short linear commit ${i + 1}`,
    }));
    repo.commits.push({ ...template, oid: oid(100), parents: [oid(1)], subject: "Actual side branch" });
    repo.refs = [{ name: "refs/heads/main", oid: oid(count + 2), commit_oid: oid(count + 2), kind: "branch" }];
    repo.checkouts = [{ ...repo.checkouts[0], head: oid(1) }];
    server.writeLegacySnapshot(source);
    await browser.setViewportSize(1440, 1000);
    await connect(`git?project=harbor&focus=${oid(1)}`);
    await browser.waitForSelector(".lv-inspector");
    await browser.waitForSelector(`.git-canvas[aria-label*="${count + 3} commits, 1 collapsed ranges"]`);
    await browser.waitForSelector('.git-commit-range[aria-expanded="false"]');
    const label = `${oid(2).slice(0, 7)} → ${oid(count + 1).slice(0, 7)} + ${count} hidden. Expand commits`;
    expect(await browser.evaluate(`document.querySelector('.git-commit-range').getAttribute('aria-label')`)).toBe(label);
    expect(await browser.evaluate(`document.querySelectorAll('.git-commit-range').length`)).toBe(1);
    expect(await browser.evaluate(`document.querySelector('.git-canvas').getAttribute('aria-label')`)).toContain(`${count + 3} commits, 1 collapsed ranges`);
    const hit = await browser.evaluate<{ x: number; y: number; width: number; intended: boolean }>(`(() => {
      const b=document.querySelector('.git-commit-range'), r=b.getBoundingClientRect(), x=r.x+r.width/2, y=r.y+r.height/2;
      return {x,y,width:r.width,intended:b.contains(document.elementFromPoint(x,y))};
    })()`);
    expect(hit.width).toBeLessThan(count * 112 * 0.9);
    expect(hit.intended).toBe(true);
    await browser.movePointer(hit.x, hit.y);
    await browser.clickButtonByAriaLabel(label);
    await browser.waitForSelector('.git-commit-range[aria-expanded="true"]');
    await browser.evaluate(`document.querySelector('.git-commit-range').focus()`);
    await browser.pressKey("Enter");
    await browser.waitForSelector('.git-commit-range[aria-expanded="false"]');
    await browser.clickButtonByText("Expand all");
    await browser.waitForSelector('.git-commit-range[aria-expanded="true"]');
    await browser.clickButtonByText("Collapse linear");
    await browser.waitForSelector('.git-commit-range[aria-expanded="false"]');
    expect(await browser.evaluate(`location.hash`)).toContain(`focus=${oid(1)}`);
    expect(await browser.evaluate(`Boolean(document.querySelector('.lv-inspector'))`)).toBe(true);
    await browser.captureScreenshot(join(evidence, `git-short-${count}.png`));
    await browser.clickButtonByText("Inventory");
    expect(await browser.evaluate(`document.querySelectorAll('.git-inventory tbody tr').length`)).toBe(count + 3);
    await browser.clickButtonByText("Graph");
    await browser.setViewportSize(390, 844);
    for (let i = 0; i < 9; i++) await browser.clickButtonByAriaLabel("Zoom Git out");
    await browser.waitForSelector('.git-commit-range.is-tiny[aria-expanded="false"]');
    expect(await browser.evaluate(`(() => { const b=document.querySelector('.git-commit-range');return b.scrollWidth <= b.clientWidth && b.getBoundingClientRect().width >= 24; })()`)).toBe(true);
    expect(await browser.evaluate(`document.querySelector('.git-commit-range').getAttribute('aria-label')`)).toBe(label);
    expect((await browser.runAxeAudit()).violations).toEqual([]);
    await browser.captureScreenshot(join(evidence, `git-short-${count}-mobile.png`));
    await browser.setViewportSize(1440, 1000);
    server.writeLegacySnapshot(sourceFixture());
  }, 60_000);
  it("compresses long histories into accessible reversible ranges and keeps inventory complete", async () => {
    const source = sourceFixture();
    const repo = source.git.harbor;
    const oid = (n: number) => ((n * 2654435761) >>> 0).toString(16).padStart(8, "0").padEnd(40, "0");
    const template = repo.commits[0];
    repo.refs = [];
    repo.checkouts = [];
    repo.commits = Array.from({ length: 200 }, (_, i) => ({
      ...template, oid: oid(i + 1), parents: i ? [oid(i)] : [], subject: `Linear commit ${i + 1}`,
    }));
    server.writeLegacySnapshot(source);
    await connect("git");
    if (await browser.evaluate(`Boolean(document.querySelector('.git-inventory'))`))
      await browser.clickButtonByText("Graph");
    await browser.waitForSelector('.git-commit-range[aria-expanded="false"]');
    await browser.waitForText("198 hidden");
    expect(await browser.evaluate(`document.querySelector('.git-scroll').scrollWidth`)).toBeLessThan(2000);
    expect(await browser.evaluate(`document.querySelector('.git-canvas').getAttribute('aria-label')`)).toContain("200 commits, 1 collapsed ranges");
    await browser.captureScreenshot(join(evidence, "git-compressed.png"));
    await browser.evaluate(`document.querySelector('.git-commit-range').focus()`);
    await browser.pressKey("Enter");
    await browser.waitForSelector('.git-commit-range[aria-expanded="true"]');
    expect(await browser.evaluate(`document.querySelector('.git-scroll').scrollWidth`)).toBeGreaterThan(15000);
    await browser.captureScreenshot(join(evidence, "git-expanded.png"));
    await browser.pressKey("Enter");
    await browser.waitForSelector('.git-commit-range[aria-expanded="false"]');
    expect(await browser.evaluate(`document.querySelector('.git-scroll').scrollWidth`)).toBeLessThan(2000);
    await browser.clickButtonByText("Inventory");
    expect(await browser.evaluate(`document.querySelectorAll('.git-inventory tbody tr').length`)).toBe(100);
    await browser.clickButtonByText("Show next");
    expect(await browser.evaluate(`document.querySelectorAll('.git-inventory tbody tr').length`)).toBe(200);
    await browser.clickButtonByText(oid(80).slice(0, 10));
    await browser.waitForSelector('.lv-inspector');
    await browser.clickButtonByText("Graph");
    await browser.clickButtonByText("Collapse linear");
    await browser.evaluate("new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))");
    await browser.waitForSelector('.git-commit-range[aria-expanded="false"]');
    await browser.fillInputByLabel("Search Git projects and commits", "Linear commit 80");
    await browser.waitForSelector('.git-commit-range[aria-expanded="true"]');
    await browser.fillInputByLabel("Search Git projects and commits", "");
    await browser.waitForText("198 hidden");
    repo.commits.push({ ...template, oid: oid(201), parents: [oid(200)], subject: "New live tip" });
    server.writeLegacySnapshot(source);
    await browser.waitForText("199 hidden");
    expect(await browser.evaluate(`document.querySelector('.git-canvas').getAttribute('aria-label')`)).toContain("201 commits");
    const report = await browser.runAxeAudit();
    expect(report.violations).toEqual([]);
    await browser.clickButtonByAriaLabel("Switch to light theme");
    expect((await browser.runAxeAudit()).violations).toEqual([]);
    await browser.captureScreenshot(join(evidence, "git-compressed-light.png"));
    await browser.clickButtonByAriaLabel("Switch to dark theme");
    await browser.setViewportSize(640, 844);
    await browser.clickButtonByText("Fit");
    await browser.waitForSelector('.git-commit-range');
    expect(await browser.evaluate(`(() => { const r = document.querySelector('.git-commit-range').getBoundingClientRect(); return r.width; })()`)).toBe(304);
    await browser.evaluate("new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))");
    await browser.captureScreenshot(join(evidence, "git-compressed-narrow.png"));
    await browser.setViewportSize(390, 844);
    await browser.evaluate(`document.querySelector('.git-scroll').scrollLeft = 200`);
    await browser.waitForSelector('.git-commit-range');
    await browser.evaluate("new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))");
    expect(await browser.evaluate(`(() => { const b = document.querySelector('.git-commit-range'); return b.scrollWidth <= b.clientWidth; })()`)).toBe(true);
    await browser.captureScreenshot(join(evidence, "git-compressed-mobile.png"));
    await browser.setViewportSize(1440, 1000);
    server.writeLegacySnapshot(sourceFixture());
  }, 60_000);
  it("keeps expansion independent for projects sharing commit objects", async () => {
    await browser.setViewportSize(1440, 1000);
    const source = sourceFixture();
    const repo = source.git.harbor, template = repo.commits[0];
    const oid = (n: number) => n.toString(16).padStart(40, "0");
    repo.refs = []; repo.checkouts = [];
    repo.commits = Array.from({ length: 20 }, (_, i) => ({
      ...template, oid: oid(i + 1), parents: i ? [oid(i)] : [],
    }));
    Object.assign(source.git, { observatory: structuredClone(repo) });
    server.writeLegacySnapshot(source);
    await connect("git");
    if (await browser.evaluate(`Boolean(document.querySelector('.git-inventory'))`))
      await browser.clickButtonByText("Graph");
    await browser.waitForSelector('.git-band:nth-child(2) .git-commit-range[aria-expanded="false"]');
    await browser.evaluate(`document.querySelector('.git-band:first-child .git-commit-range').click()`);
    await browser.waitForSelector('.git-band:first-child .git-commit-range[aria-expanded="true"]');
    expect(await browser.evaluate(`document.querySelector('.git-band:nth-child(2) .git-commit-range').getAttribute('aria-expanded')`)).toBe("false");
    await browser.clickButtonByText("Inventory");
    await browser.evaluate(`Array.from(document.querySelectorAll('.git-inventory tbody tr')).filter(row => row.textContent.includes('Harbor'))[1].querySelector('button').click()`);
    await browser.waitForSelector('.lv-inspector');
    await browser.clickButtonByText("Graph");
    await browser.waitForSelector('.git-band:first-child .git-canvas[aria-label*="0 collapsed ranges"]');
    expect(await browser.evaluate(`document.querySelector('.git-band:nth-child(2) .git-canvas').getAttribute('aria-label')`)).toContain("1 collapsed ranges");
    server.writeLegacySnapshot(sourceFixture());
  });
  it.each(["root", "tag", "tip", "checkout"] as const)("preserves protected %s selection when collapsing linear ranges", async (kind) => {
    const source = sourceFixture(), repo = source.git.harbor, template = repo.commits[0];
    const oid = (n: number) => ((n * 2654435761) >>> 0).toString(16).padStart(8, "0").padEnd(40, "0");
    repo.commits = Array.from({ length: 40 }, (_, i) => ({ ...template, oid: oid(i + 1), parents: i ? [oid(i)] : [], subject: `Protected history ${i + 1}` }));
    repo.refs = [{ name: "refs/tags/milestone", oid: oid(20), commit_oid: oid(20), kind: "tag" }];
    repo.checkouts = [{ ...repo.checkouts[0], id: "protected-tree", name: "protected-checkout", head: oid(30), branch: "protected/checkout" }];
    server.writeLegacySnapshot(source);
    await connect("git");
    await browser.waitForText("40 commits");
    await browser.waitForText("protected-checkout");
    await browser.clickButtonByText("Inventory");
    if (kind === "checkout") await browser.clickButtonByText("protected-checkout");
    else await browser.clickButtonByText(oid(kind === "root" ? 1 : kind === "tag" ? 20 : 40).slice(0, 10));
    await browser.waitForSelector(".lv-inspector");
    const selected = await browser.evaluate<string>(`location.hash`);
    await browser.clickButtonByText("Graph");
    await browser.clickButtonByText("Expand all");
    await browser.clickButtonByText("Collapse linear");
    await browser.evaluate("new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))");
    expect(await browser.evaluate(`Boolean(document.querySelector('.lv-inspector'))`), "A visible topology anchor must retain its inspector").toBe(true);
    expect(await browser.evaluate(`location.hash`)).toBe(selected);
    expect(await browser.evaluate(`document.querySelectorAll('.git-commit-range[aria-expanded="false"]').length`)).toBeGreaterThan(0);
    server.writeLegacySnapshot(sourceFixture());
  });
  it("keeps manually collapsed matching ranges closed across live tip growth", async () => {
    const source = sourceFixture(), repo = source.git.harbor, template = repo.commits[0];
    const oid = (n: number) => ((n * 2654435761) >>> 0).toString(16).padStart(8, "0").padEnd(40, "0");
    repo.refs = []; repo.checkouts = [];
    repo.commits = Array.from({ length: 20 }, (_, i) => ({ ...template, oid: oid(i + 1), parents: i ? [oid(i)] : [], subject: `Matching history ${i + 1}` }));
    server.writeLegacySnapshot(source);
    await connect("git");
    await browser.clickButtonByText("Follow latest");
    await browser.fillInputByLabel("Search Git projects and commits", "Matching history");
    await browser.waitForSelector('.git-commit-range[aria-expanded="true"]');
    await browser.evaluate(`document.querySelector('.git-commit-range').click()`);
    await browser.waitForSelector('.git-commit-range[aria-expanded="false"]');
    repo.commits.push({ ...template, oid: oid(21), parents: [oid(20)], subject: "Matching history 21" });
    server.writeLegacySnapshot(source);
    await browser.waitForText("21 commits");
    await browser.evaluate("new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))");
    expect(await browser.evaluate(`document.querySelector('.git-commit-range')?.getAttribute('aria-expanded')`), "An unchanged search must not reopen a manually collapsed growing range").toBe("false");
    expect(await browser.evaluate(`document.querySelector('.git-commit-range')?.textContent`)).toContain("19 hidden");
    await browser.clickButtonByText("Inventory");
    await browser.clickButtonByText(oid(10).slice(0, 10));
    await browser.waitForSelector(".lv-inspector");
    await browser.clickButtonByText("Graph");
    await browser.waitForSelector('.git-canvas[aria-label*="0 collapsed ranges"]');
    await browser.clickButtonByText("Collapse linear");
    await browser.waitForSelector('.git-commit-range[aria-expanded="false"]');
    expect(await browser.evaluate(`Boolean(document.querySelector('.lv-inspector'))`)).toBe(false);
    await browser.fillInputByLabel("Search Git projects and commits", "Matching history 1");
    await browser.waitForSelector('.git-commit-range[aria-expanded="true"]');
    server.writeLegacySnapshot(sourceFixture());
  });
  it.each(["direct", "switch-project"] as const)("focuses shared commits in the requested project (%s)", async (mode) => {
    const source = sourceFixture(), repo = source.git.harbor, template = repo.commits[0];
    const oid = (n: number) => ((n * 2654435761) >>> 0).toString(16).padStart(8, "0").padEnd(40, "0");
    repo.refs = []; repo.checkouts = [];
    repo.commits = Array.from({ length: 20 }, (_, i) => ({ ...template, oid: oid(i + 1), parents: i ? [oid(i)] : [], subject: `Shared history ${i + 1}` }));
    Object.assign(source.git, { observatory: { ...structuredClone(repo), instance_id: "second-repository" } });
    const fillers = Array.from({ length: 8 }, (_, i) => `focus-filler-${i}`);
    const tail = Array.from({ length: 8 }, (_, i) => `focus-tail-${i}`);
    for (const id of [...fillers, ...tail]) Object.assign(source.projects, { [id]: { name: id } });
    source.order = ["harbor", ...fillers, "observatory", ...tail];
    server.writeLegacySnapshot(source);
    const focus = oid(10);
    await connect(`git?project=${mode === "direct" ? "observatory" : "harbor"}&focus=${focus}`);
    if (mode === "switch-project") {
      await browser.waitForSelector('.git-band[aria-label="Harbor repository"] .git-canvas[aria-label*="0 collapsed ranges"]');
      await browser.evaluate(`location.hash=${JSON.stringify(`#/git?project=observatory&focus=${focus}`)}`);
    }
    await browser.waitForSelector(".lv-inspector");
    await browser.evaluate("new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))");
    const state = await browser.evaluate<{ top: number; targetTop: number; inspector: string }>(`(() => {
      const scroll=document.querySelector('.git-scroll'), band=document.querySelector('.git-band[aria-label="Observatory repository"]');
      return {top:scroll.scrollTop,targetTop:band.offsetTop,inspector:document.querySelector('.lv-inspector')?.textContent ?? ''};
    })()`);
    expect(state.inspector).toContain("Observatory");
    expect(state.targetTop).toBeGreaterThan(1000);
    expect(state.top, "Camera must reach the explicitly requested project, including identical-OID project switches").toBeGreaterThanOrEqual(state.targetTop - 2);
    server.writeLegacySnapshot(sourceFixture());
  });
  it("preserves filtered-out project expansion and collapse intent during global controls", async () => {
    const source = sourceFixture(), repo = source.git.harbor, template = repo.commits[0];
    const oid = (n: number) => ((n * 2654435761) >>> 0).toString(16).padStart(8, "0").padEnd(40, "0");
    repo.refs = []; repo.checkouts = [];
    repo.commits = Array.from({ length: 20 }, (_, i) => ({ ...template, oid: oid(i + 1), parents: i ? [oid(i)] : [], subject: `Scoped history ${i + 1}` }));
    Object.assign(source.git, { observatory: { ...structuredClone(repo), instance_id: "second-scoped-repository" } });
    server.writeLegacySnapshot(source);
    await connect("git");
    await browser.waitForText("40 commits");
    await browser.waitForSelector('.git-band:nth-child(2) .git-commit-range');
    async function filter(project: string) {
      await browser.evaluate(`(() => {const select=document.querySelector('select[aria-label="Filter Git project"]');select.value=${JSON.stringify(project)};select.dispatchEvent(new Event('change',{bubbles:true}));})()`);
      await browser.evaluate("new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))");
    }
    await browser.evaluate(`document.querySelector('.git-band:first-child .git-commit-range').click()`);
    await browser.waitForSelector('.git-band:first-child .git-commit-range[aria-expanded="true"]');
    await filter("observatory");
    await browser.clickButtonByText("Expand all");
    await browser.clickButtonByText("Collapse linear");
    await filter("");
    expect(await browser.evaluate(`document.querySelector('.git-band:first-child .git-commit-range')?.getAttribute('aria-expanded')`), "Global controls must preserve hidden-project expansion").toBe("true");
    await browser.fillInputByLabel("Search Git projects and commits", "Scoped history");
    await browser.evaluate(`document.querySelector('.git-band:first-child .git-commit-range').click()`);
    await browser.waitForSelector('.git-band:first-child .git-commit-range[aria-expanded="false"]');
    await filter("observatory");
    await browser.clickButtonByText("Collapse linear");
    await browser.clickButtonByText("Expand all");
    await filter("");
    expect(await browser.evaluate(`document.querySelector('.git-band:first-child .git-commit-range')?.getAttribute('aria-expanded')`), "Global controls must preserve hidden-project manual collapse").toBe("false");
    expect(await browser.evaluate(`document.querySelector('.git-band:nth-child(2) .git-commit-range')?.getAttribute('aria-expanded')`)).toBe("true");
    server.writeLegacySnapshot(sourceFixture());
  });
  it("preserves Git offsets and follows new commits after Inventory", async () => {
    const long = sourceFixture(),
      oid = (n: number) => n.toString(16).padStart(40, "0"),
      commit = long.git.harbor.commits[0];
    long.git.harbor.commits = Array.from({ length: 100 }, (_, i) => ({
      ...commit,
      oid: oid(i + 1),
      parents: i ? [oid(i)] : [],
      subject: `Long graph commit ${i + 1}`,
    }));
    for (let i = 0; i < 5; i++) {
      const id = `extra-${i}`;
      Object.assign(long.projects, { [id]: { name: id } });
      long.order.push(id);
    }
    server.writeLegacySnapshot(long);
    await connect("git");
    await browser.waitForText("100 commits");
    if (
      await browser.evaluate(
        `Boolean(document.querySelector('.git-inventory'))`,
      )
    )
      await browser.clickButtonByText("Graph");
    await browser.waitForSelector(".git-scroll");
    await browser.clickButtonByText("Expand all");
    expect(
      await browser.evaluate<number>(
        `document.querySelector('.git-scroll').scrollWidth`,
      ),
    ).toBeGreaterThan(3000);
    await browser.evaluate(`(() => {
      const el = document.querySelector('.git-scroll');
      el.dispatchEvent(new PointerEvent('pointerdown', {bubbles:true}));
      el.scrollLeft = 900; el.scrollTop = 300;
      el.dispatchEvent(new Event('scroll', {bubbles:true}));
    })()`);
    await browser.evaluate(
      "new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))",
    );
    const before = await browser.evaluate<{ left: number; top: number }>(
      `({left:document.querySelector('.git-scroll').scrollLeft,top:document.querySelector('.git-scroll').scrollTop})`,
    );
    expect(before.left).toBeGreaterThan(0);
    expect(before.top).toBeGreaterThan(0);
    await browser.clickButtonByText("Inventory");
    await browser.waitForSelector(".git-inventory");
    await browser.clickButtonByText("Graph");
    await browser.waitForSelector(".git-scroll");
    await browser.evaluate(
      "new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))",
    );
    expect(
      await browser.evaluate(
        `({left:document.querySelector('.git-scroll').scrollLeft,top:document.querySelector('.git-scroll').scrollTop})`,
      ),
    ).toEqual(before);
    await browser.clickButtonByText("Follow latest");
    await browser.clickButtonByText("Inventory");
    long.git.harbor.commits.push(
      ...Array.from({ length: 50 }, (_, i) => ({
        ...commit,
        oid: oid(i + 101),
        parents: [oid(i + 100)],
        subject: `New frontier commit ${i + 101}`,
      })),
    );
    server.writeLegacySnapshot(long);
    await browser.waitForText("150 commits");
    await browser.clickButtonByText("Graph");
    await browser.waitForSelector(".git-scroll");
    await browser.evaluate(
      "new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))",
    );
    expect(
      await browser.evaluate(
        `(()=>{const el=document.querySelector('.git-scroll');return el.scrollWidth-el.clientWidth-el.scrollLeft;})()`,
      ),
      "Follow latest must reach the new frontier after updates arrive in Inventory",
    ).toBeLessThanOrEqual(1);
    server.writeLegacySnapshot(sourceFixture());
  });
  it("updates an existing map through SSE while preserving coordinates and exposes the full 500-agent census", async () => {
    await connect("atlas");
    await browser.waitForSelector(".atlas-agent");
    const before = await browser.evaluate<string>(
      `Array.from(document.querySelectorAll('.atlas-agent > strong')).find(name => name.textContent === 'worker_0')?.parentElement?.getAttribute('style')`,
    );
    expect(before).toBeTypeOf("string");
    const next = sourceFixture(500);
    next.agents[0].state = "waiting for input";
    server.writeLegacySnapshot(next);
    await browser.waitForText("503", 10_000);
    const after = await browser.evaluate<string>(
      `Array.from(document.querySelectorAll('.atlas-agent > strong')).find(name => name.textContent === 'worker_0')?.parentElement?.getAttribute('style')`,
    );
    expect(after).toBe(before);
    await browser.fillInputByLabel("Search all agents", "worker_499");
    await browser.clickButtonByText("Roster");
    await browser.waitForText("worker_499");
    expect(
      await browser.evaluate(
        `document.querySelectorAll('.atlas-roster tbody tr').length`,
      ),
    ).toBe(1);
    await browser.clickButtonByText("worker_499");
    await browser.waitForSelector(".lv-inspector");
    await browser.waitForText("worker_499");
    const response = await fetch(`${server.baseUrl}/v2/views`, {
      headers: { Authorization: `Bearer ${server.ownerToken}` },
    });
    const snapshot = (await response.json()) as { agents: unknown[] };
    expect(snapshot.agents.length).toBe(505);
  });
  it("collects a real temporary repository and streams a new commit into the built UI", async () => {
    const dir = mkdtempSync(join(realpathSync("/tmp"), "adm-views-real-git-"));
    chmodSync(dir, 0o700);
    let realServer: IsolatedNativeAdmiraltyServer | undefined;
    const run = (program: string, args: string[]) => {
      const result = spawnSync(program, args, {
        cwd: dir,
        encoding: "utf8",
        timeout: 30_000,
      });
      expect(result.status, result.stderr).toBe(0);
      return result.stdout.trim();
    };
    try {
      run("/usr/bin/git", ["init", "-b", "main"]);
      run("/usr/bin/git", ["config", "user.email", "fixture@example.invalid"]);
      run("/usr/bin/git", ["config", "user.name", "Fixture"]);
      run("/usr/bin/git", ["commit", "--allow-empty", "-m", "Actual root"]);
      const config = join(dir, "observe.json"),
        output = join(dir, "output");
      writeFileSync(
        config,
        JSON.stringify({
          schema_version: 1,
          node_id: "fixture-node",
          output_dir: output,
          projects: [
            {
              id: "real-git",
              name: "Real Git fixture",
              repository: dir,
              tracker: { kind: "none" },
            },
          ],
        }),
        { mode: 0o600 },
      );
      const observer =
        process.env.ADMIRALTY_E2E_OBSERVER_BIN ??
        join(REPO_ROOT, "target", "debug", "admiralty-observe");
      run(observer, ["--config", config, "--once"]);
      realServer = await startIsolatedNativeAdmiraltyServer({
        legacySnapshot: JSON.parse(
          readFileSync(join(output, "status.json"), "utf8"),
        ),
      });
      await connect("git", realServer);
      await browser.waitForSelector(".git-canvas");
      await browser.clickButtonByText("Inventory");
      await browser.waitForText("Actual root");
      const head = run("/usr/bin/git", ["rev-parse", "HEAD"]);
      run("/usr/bin/git", [
        "commit",
        "--allow-empty",
        "-m",
        "Actual live commit",
      ]);
      run(observer, ["--config", config, "--once"]);
      realServer.writeLegacySnapshot(
        JSON.parse(readFileSync(join(output, "status.json"), "utf8")),
      );
      await browser.waitForText("Actual live commit", 10_000);
      const response = await fetch(`${realServer.baseUrl}/v2/views`, {
        headers: { Authorization: `Bearer ${realServer.ownerToken}` },
      });
      const view = (await response.json()) as {
        projects: Array<{
          git: { commits: Array<{ subject: string; parents: string[] }> };
        }>;
      };
      expect(
        view.projects[0].git.commits.find(
          (c) => c.subject === "Actual live commit",
        )?.parents,
      ).toEqual([head]);
    } finally {
      await realServer?.close();
      rmSync(dir, { recursive: true, force: true });
    }
  }, 60_000);
  it("passes axe in both themes, graph/roster/inventory and narrow screens with reduced motion", async () => {
    server.writeLegacySnapshot(sourceFixture());
    await connect("atlas");
    await browser.waitForText("11");
    await browser.clickButtonByText("Fit fleet");
    await browser.emulateReducedMotion(true);
    const results: unknown[] = [];
    async function audit(label: string) {
      await browser.evaluate(
        "new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))",
      );
      const report = await browser.runAxeAudit();
      results.push({ label, report });
      writeFileSync(
        join(evidence, "axe-results.json"),
        JSON.stringify(results, null, 2),
      );
      expect(
        report.violations,
        `${label}: ${JSON.stringify(report.violations)}`,
      ).toEqual([]);
      expect(report.incomplete, `${label} incomplete`).toEqual([]);
    }
    await audit("atlas dark");
    await browser.clickButtonByText("Roster");
    await audit("roster dark");
    await browser.clickButtonByAriaLabel("Switch to light theme");
    await audit("roster light");
    await browser.clickButtonByText("Map");
    await audit("atlas light");
    await browser.clickNav("Git Flow");
    await browser.waitForSelector(".git-canvas");
    await audit("git light");
    await browser.clickButtonByText("Inventory");
    await audit("git inventory light");
    await browser.clickButtonByAriaLabel("Switch to dark theme");
    await audit("git inventory dark");
    await browser.clickButtonByText("Graph");
    await audit("git dark");
    await browser.setViewportSize(390, 844);
    await browser.clickNav("Fleet Atlas");
    await browser.waitForSelector(".atlas-roster");
    await audit("mobile roster");
    await browser.captureScreenshot(join(evidence, "fleet-mobile.png"));
    writeFileSync(
      join(evidence, "axe-results.json"),
      JSON.stringify(results, null, 2),
    );
  }, 60_000);
});
