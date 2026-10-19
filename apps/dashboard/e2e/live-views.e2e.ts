import { afterAll, beforeAll, describe, expect, it } from "vitest";
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
  async function connect(route: string) {
    await browser.goto(`${server.baseUrl}/#/${route}`);
    await browser.waitForText("Session access");
    await browser.clickButtonByText("Session access");
    await browser.fillInputByLabel("Bearer token", server.ownerToken);
    await browser.clickButtonByText("Apply token");
    await browser.waitForText("Connected");
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
    await browser.clickButtonByText("Session access");
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
  it("updates an existing map through SSE while preserving coordinates and exposes the full 500-agent census", async () => {
    await connect("atlas");
    await browser.waitForSelector(".atlas-agent");
    const before = await browser.evaluate<string>(
      `document.querySelector('[aria-label^="WORKER worker_0"]')?.getAttribute('style')`,
    );
    expect(before).toBeTypeOf("string");
    const next = sourceFixture(500);
    next.agents[0].state = "waiting for input";
    server.writeLegacySnapshot(next);
    await browser.waitForText("503", 10_000);
    const after = await browser.evaluate<string>(
      `document.querySelector('[aria-label^="WORKER worker_0"]')?.getAttribute('style')`,
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
      await browser.goto(`${realServer.baseUrl}/#/git`);
      await browser.waitForText("Session access");
      await browser.clickButtonByText("Session access");
      await browser.fillInputByLabel("Bearer token", realServer.ownerToken);
      await browser.clickButtonByText("Apply token");
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
