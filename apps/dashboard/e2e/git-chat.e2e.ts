import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { launchBrowserHarness, startDashboardTestServer, REPO_ROOT, type BrowserHarness, type DashboardTestServer } from "./browser-harness";

describe("built per-project Git operator chat", () => {
  let browser: BrowserHarness, server: DashboardTestServer;
  const evidence = join(REPO_ROOT, "target", "git-chat-evidence");
  async function select(label: string, value: string) {
    await browser.evaluate(`(() => {const element=document.querySelector('select[aria-label=' + CSS.escape(${JSON.stringify(label)}) + ']');if (!element) throw Error('Select missing');element.value=${JSON.stringify(value)};element.dispatchEvent(new Event('change',{bubbles:true}));})()`);
  }
  async function inputState() {
    return browser.evaluate<{ hover: boolean; noHover: boolean; fine: boolean; coarse: boolean; touchPoints: number; hovered: boolean; focused: boolean; opacity: string }>(`(() => {
      const chat=document.querySelector('.git-chat');
      return {hover:matchMedia('(hover:hover)').matches,noHover:matchMedia('(hover:none)').matches,
        fine:matchMedia('(pointer:fine)').matches,coarse:matchMedia('(pointer:coarse)').matches,
        touchPoints:navigator.maxTouchPoints,hovered:chat.matches(':hover'),focused:chat.matches(':focus-within'),opacity:getComputedStyle(chat).opacity};
    })()`);
  }
  async function expectOpacity(opacity: string) {
    expect(await browser.evaluate(`new Promise(resolve => {
      const until=Date.now()+1500, check=()=>{const current=getComputedStyle(document.querySelector('.git-chat')).opacity;
        current===${JSON.stringify(opacity)} || Date.now()>=until ? resolve(current) : setTimeout(check,16)};check();
    })`)).toBe(opacity);
  }
  beforeAll(async () => {
    mkdirSync(evidence, { recursive: true });
    server = await startDashboardTestServer();
    browser = await launchBrowserHarness({ desktopInput: true });
  }, 30_000);
  beforeEach(async () => {
    // Reset page/fixture and native input state so no test depends on a previous
    // chat being opened, an access token, or a mobile pointer configuration.
    await browser.resetPage();
    await browser.setViewportSize(1440, 1000);
    await browser.emulateTouchInput(false);
    await browser.movePointer(1, 1);
    await browser.goto(`${server.baseUrl}/#/detail`);
    await browser.evaluate(`(() => {
      const original = window.fetch.bind(window), stamp = new Date().toISOString();
      const oid = n => n.toString(16).padStart(40, '0');
      const projects = ['dope', 'other'].map((id, index) => ({ id, name: index ? 'Other' : 'DOPE', git: {
        instance_id: 'repo:' + id, node:'fixture', observed_utc:stamp, shallow:false, truncated:false,
        commits:Array.from({length:80}, (_, i) => ({oid:oid(i + 1), parents:i ? [oid(i)] : [], subject:'Fixture commit ' + i, committed_utc:stamp})),
        refs:[{name:'refs/heads/main', oid:oid(80), commit_oid:oid(80), kind:'branch'}], checkouts:[],
      }}));
      const snapshot = {schema_version:1, revision:'chat-fixture', simulated:false, observed_utc:stamp, projects, agents:[], sources:[]};
      const targets = project => [{id:'captain:' + project, role:'captain', label:project + ' captain'}, {id:'admiral:global',role:'admiral',label:'Fleet admiral'}];
      const reports = Object.fromEntries(projects.map(project => [project.id, {
        schema_version:1, instance_id:project.git.instance_id, observed_utc:stamp, integration_ref:'refs/heads/main', state:'attention', actionable:true,
        observed_commits:80,observed_refs:1,observed_checkouts:0,missing_parent_oids:[],shallow:false,truncated:false,observation_error:false,fingerprint:'fixture',
        issues:[{code:'diverged_branch',severity:'attention',ref_name:'refs/heads/worker',checkout_id:null,detail:'Worker has unique commits and is behind integration.'}],
        branches:[{ref_name:'refs/heads/worker',relation:'diverged',ahead:{value:3,lower_bound:false},behind:{value:8,lower_bound:false}}],
      }]));
      window.__chatRequests = []; window.__chatGetCount = 0; window.__release = false; window.__chatStored = [];
      window.fetch = async (input, init) => {
        const url = new URL(typeof input === 'string' ? input : input.url, location.href);
        if (url.pathname === '/v2/views/events') return new Response(new ReadableStream({start(controller) {
          window.__chatViewController = controller; window.__chatViewSnapshot = snapshot;
          controller.enqueue(new TextEncoder().encode('event: views\\nid: chat-fixture\\ndata: ' + JSON.stringify(snapshot) + '\\n\\n'));
          init?.signal?.addEventListener('abort', () => {try {controller.close()} catch {}});
        }}), {headers:{'Content-Type':'text/event-stream'}});
        if (url.pathname === '/v2/git/health') return Response.json({schema_version:1,reports,error:window.__gitHealthError || null});
        if (url.pathname === '/v2/operator-chat' && init?.method === 'POST') {
          const request = JSON.parse(init.body); window.__chatRequests.push(request);
          let message = window.__chatStored.find(item => item.id === request.id);
          if (!message) {message = {...request, state:'delivered',created_utc:new Date().toISOString(),detail:'Delivered to target inbox; acknowledgment and execution are separate.'}; window.__chatStored.push(message)}
          if (window.__chatMode === 'hold-post') return new Promise(resolve => {window.__chatResolvePost = () => resolve(Response.json(message))});
          if (window.__chatMode === 'deny-post') return Response.json({}, {status:403});
          if (window.__loseFirstResponse && window.__chatRequests.length === 1) throw new TypeError('Synthetic lost response');
          window.__release = true;
          return Response.json(message);
        }
        if (url.pathname === '/v2/operator-chat') {
          window.__chatGetCount++;
          const project = url.searchParams.get('project_id');
          const payload = {schema_version:1,enabled:true,simulated:true,targets:targets(project),messages:window.__release ? window.__chatStored.filter(item => item.project_id === project) : [],responses:window.__release ? [{id:'reply:' + project,project_id:project,target_id:'captain:' + project,text:'Inspecting divergence; no refs changed.',created_utc:new Date().toISOString()}] : [],history_truncated:window.__chatHistoryTruncated === true,error:null};
          if (window.__chatMode === 'deny-get') return Response.json({}, {status:401});
          if (window.__chatMode === 'hold-get') return new Promise(resolve => {window.__chatResolveGet = () => resolve(Response.json(payload))});
          return Response.json(payload);
        }
        return original(input, init);
      };
      location.hash = '#/git';
    })()`);
    await browser.waitForSelector(".git-chat-toggle");
    await browser.waitForSelector(".lv-feed.is-connected");
  }, 30_000);
  afterAll(async () => { await browser?.close(); await server?.close(); });

  it("floats through horizontal pan, focuses fully, and retries the exact routed command", async () => {
    await browser.evaluate(`window.__loseFirstResponse=true;`);
    expect(await browser.evaluate(`window.__chatGetCount`)).toBe(0);
    await browser.clickButtonByText("Expand all");
    expect(await browser.evaluate(`(() => {const scroll=document.querySelector('.git-scroll');return scroll.scrollWidth > scroll.clientWidth})()`)).toBe(true);
    const before = await browser.evaluate<{ left: number; right: number }>(`(() => {const box=document.querySelector('.git-chat').getBoundingClientRect();return {left:box.left,right:box.right}})()`);
    await browser.evaluate(`(() => {const scroll=document.querySelector('.git-scroll');scroll.scrollLeft=900;scroll.dispatchEvent(new Event('scroll',{bubbles:true}));})()`);
    await browser.evaluate(`new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))`);
    expect(await browser.evaluate<number>(`document.querySelector('.git-scroll').scrollLeft`)).toBeGreaterThan(0);
    expect(await browser.evaluate(`(() => {const box=document.querySelector('.git-chat').getBoundingClientRect();return {left:box.left,right:box.right}})()`)).toEqual(before);
    await browser.evaluate(`document.activeElement?.blur()`);
    await browser.movePointer(1, 1);
    await expectOpacity("0.86");
    const idle = await inputState();
    expect(idle).toEqual({hover:true,noHover:false,fine:true,coarse:false,touchPoints:0,hovered:false,focused:false,opacity:"0.86"});
    console.log("CHAT_DESKTOP_IDLE", idle);
    const hoverPoint = await browser.evaluate<{ x: number; y: number }>(`(() => {const box=document.querySelector('.git-chat-toggle').getBoundingClientRect();return {x:box.left+box.width/2,y:box.top+box.height/2}})()`);
    await browser.movePointer(hoverPoint.x, hoverPoint.y);
    await expectOpacity("1");
    const hovered = await inputState();
    expect(hovered).toMatchObject({hovered:true,focused:false,opacity:"1"});
    console.log("CHAT_DESKTOP_HOVER", hovered);
    await browser.movePointer(1, 1);
    await expectOpacity("0.86");
    await browser.clickButtonByText("DOPE · Chat");
    await browser.waitForText("Exact target: captain:dope");
    await browser.evaluate(`document.querySelector('.git-chat textarea').focus()`);
    await expectOpacity("1");
    const focused = await inputState();
    expect(focused).toMatchObject({hovered:false,focused:true,opacity:"1"});
    console.log("CHAT_DESKTOP_FOCUS", focused);
    await select("DOPE chat kind", "command");
    await browser.fillInputByLabel("DOPE chat text", "Inspect these split branches; propose a merge plan.");
    await browser.clickButtonByText("Send command");
    await browser.waitForText("Delivery uncertain");
    await browser.clickButtonByText("Retry same message");
    await browser.waitForText("Delivered to inbox; awaiting reply");
    const requests = await browser.evaluate<unknown[]>(`window.__chatRequests`);
    expect(requests).toHaveLength(2);
    expect(requests[0]).toEqual(requests[1]);
    expect(requests[0]).toMatchObject({ project_id: "dope", target_id: "captain:dope", kind: "command" });
    await browser.waitForText("Inspecting divergence; no refs changed.", 8_000);
    await select("DOPE chat destination", "admiral:global");
    await select("DOPE chat kind", "message");
    await browser.fillInputByLabel("DOPE chat text", "Admiral: please review DOPE divergence.");
    await browser.clickButtonByText("Send message");
    await browser.waitForText("Admiral: please review DOPE divergence.");
    expect(await browser.evaluate(`window.__chatRequests[2]`)).toMatchObject({ project_id: "dope", target_id: "admiral:global", kind: "message" });
    await browser.captureScreenshot(join(evidence, "project-chat.png"));
  });

  it("keeps project drafts, pauses sending and exposes cached health evidence", async () => {
    await browser.clickButtonByText("DOPE · Chat");
    await browser.waitForText("Exact target: captain:dope");
    await select("DOPE chat destination", "admiral:global");
    await browser.fillInputByLabel("DOPE chat text", "Unsubmitted DOPE draft");
    await select("Filter Git project", "other");
    await browser.clickButtonByText("Other · Chat");
    await browser.waitForText("Exact target: captain:other");
    expect(await browser.evaluate(`document.querySelector('.git-chat textarea').value`)).toBe("");
    await browser.fillInputByLabel("Other chat text", "Other project draft");
    await select("Filter Git project", "dope");
    await browser.waitForSelector(".git-chat textarea");
    expect(await browser.evaluate(`document.querySelector('.git-chat textarea').value`)).toBe("Unsubmitted DOPE draft");
    await browser.waitForText("Exact target: admiral:global");
    await browser.clickButtonByText("Pause");
    await browser.waitForText("Resume live observations before sending");
    expect(await browser.evaluate(`document.querySelector('.git-chat button[type=submit]').disabled`)).toBe(true);
    await browser.clickButtonByText("Resume live");
    await browser.waitForSelector(".git-health-attention");
    await browser.evaluate(`document.querySelector('.git-health summary').click()`);
    await browser.waitForText("Worker has unique commits and is behind integration.");
    expect(await browser.evaluate(`document.querySelector('.git-health-detail').textContent`)).toContain("ahead 3 / behind 8");
    await browser.evaluate(`window.__gitHealthError='Captain notification delivery failed; report retained';`);
    await browser.waitForSelector(".git-health-unknown", 8_000);
    await browser.waitForText("Captain notification delivery failed; report retained");
    await browser.evaluate(`window.__gitHealthError=null;`);
    await browser.evaluate(`document.querySelector('.git-health summary').click()`);
    const audit = await browser.runAxeAudit();
    expect(audit.violations, JSON.stringify(audit.violations)).toEqual([]);
    await browser.clickButtonByText("DOPE · Chat");
    const reads = await browser.evaluate<number>(`window.__chatGetCount`);
    await browser.evaluate(`new Promise(resolve=>setTimeout(resolve,3200))`);
    expect(await browser.evaluate(`window.__chatGetCount`)).toBe(reads);
    await browser.captureScreenshot(join(evidence, "git-health.png"));
  });

  it("clears private drafts and local delivery history on access change, and fits narrow screens", async () => {
    await browser.clickButtonByText("DOPE · Chat");
    await browser.fillInputByLabel("DOPE chat text", "Private draft from old access");
    await browser.evaluate(`window.__chatStored=[];window.__release=false;`);
    await browser.clickButtonByAriaLabel("Access settings");
    await browser.fillInputByLabel("Bearer token", "changed-synthetic-token");
    await browser.clickButtonByText("Apply token");
    await browser.waitForSelector(".lv-feed.is-connected");
    expect(await browser.evaluate(`document.querySelector('.git-chat-toggle').getAttribute('aria-expanded')`)).toBe("false");
    await browser.clickButtonByText("DOPE · Chat");
    await browser.waitForText("Exact target: captain:dope");
    expect(await browser.evaluate(`document.querySelector('.git-chat textarea').value`)).toBe("");
    expect(await browser.evaluate(`document.querySelectorAll('.git-chat-timeline article').length`)).toBe(0);
    await browser.setViewportSize(390, 844);
    await browser.emulateTouchInput(true);
    await browser.movePointer(1, 1);
    await browser.evaluate(`document.activeElement?.blur()`);
    await expectOpacity("1");
    const touch = await inputState();
    expect(touch).toEqual({hover:false,noHover:true,fine:false,coarse:true,touchPoints:1,hovered:false,focused:false,opacity:"1"});
    console.log("CHAT_TOUCH_IDLE", touch);
    await browser.evaluate(`new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))`);
    const bounds = await browser.evaluate<{ left: number; right: number }>(`(() => {const box=document.querySelector('.git-chat').getBoundingClientRect();return {left:box.left,right:box.right}})()`);
    expect(bounds.left).toBeGreaterThanOrEqual(0);
    expect(bounds.right).toBeLessThanOrEqual(390);
    const audit = await browser.runAxeAudit();
    expect(audit.violations, JSON.stringify(audit.violations)).toEqual([]);
    await browser.captureScreenshot(join(evidence, "project-chat-mobile.png"));
  });

  it("fences late read/write successes after either concurrent request rejects access", async () => {
    await browser.setViewportSize(1440, 1000);
    async function freshScope(suffix: string) {
      await browser.evaluate(`window.__chatMode='normal';window.__chatStored=[];window.__release=false;window.__chatResolvePost=undefined;window.__chatResolveGet=undefined;window.__chatHistoryTruncated=true;`);
      await browser.clickButtonByAriaLabel("Access settings");
      await browser.fillInputByLabel("Bearer token", `synthetic-access-race-${suffix}`);
      await browser.clickButtonByText("Apply token");
      await browser.waitForSelector(".lv-feed.is-connected");
      await browser.clickButtonByText("DOPE · Chat");
      await browser.waitForText("Exact target: captain:dope");
      await browser.waitForText("older messages or replies were omitted");
    }
    await freshScope("read-denied");
    await browser.evaluate(`window.__chatMode='hold-post';`);
    await browser.fillInputByLabel("DOPE chat text", "Sensitive pending write");
    await browser.clickButtonByText("Send message");
    await browser.waitForText("Sending…");
    await browser.evaluate(`window.__chatMode='deny-get';`);
    await browser.waitForText("Session access required for operator chat", 8_000);
    await browser.evaluate(`window.__chatResolvePost();`);
    await browser.evaluate(`new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))`);
    expect(await browser.evaluate(`document.querySelectorAll('.git-chat-timeline article').length`)).toBe(0);
    expect(await browser.evaluate(`document.querySelector('.git-chat button[type=submit]').disabled`)).toBe(true);

    await freshScope("write-denied");
    await browser.evaluate(`window.__chatMode='hold-get';`);
    // Hold an actual component poll, then let the write reject access first.
    await browser.evaluate(`new Promise((resolve,reject) => {const until=Date.now()+6000;const check=()=>window.__chatResolveGet ? resolve(true) : Date.now()>until ? reject(Error('Poll not pending')) : setTimeout(check,25);check()})`);
    await browser.evaluate(`window.__chatMode='deny-post';`);
    await browser.fillInputByLabel("DOPE chat text", "Sensitive write rejected");
    await browser.clickButtonByText("Send message");
    await browser.waitForText("Session access required for operator chat", 8_000);
    await browser.evaluate(`window.__chatResolveGet();`);
    await browser.evaluate(`new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))`);
    expect(await browser.evaluate(`document.querySelector('.git-chat-destination').textContent`)).not.toContain("Exact target:");
    expect(await browser.evaluate(`document.querySelectorAll('.git-chat-timeline article').length`)).toBe(0);
    expect(await browser.evaluate(`document.querySelector('.git-chat button[type=submit]').disabled`)).toBe(true);
  }, 20_000);

  it("isolates cached live chat from simulated frames and resumes with fresh live destinations", async () => {
    await browser.setViewportSize(1440, 1000);
    await browser.evaluate(`window.__chatMode='normal';window.__chatStored=[];window.__release=false;window.__chatHistoryTruncated=false;`);
    await browser.clickButtonByAriaLabel("Access settings");
    await browser.fillInputByLabel("Bearer token", "synthetic-source-isolation-token");
    await browser.clickButtonByText("Apply token");
    await browser.waitForSelector(".lv-feed.is-connected");
    await browser.clickButtonByText("DOPE · Chat");
    await browser.waitForText("Exact target: captain:dope");
    await browser.fillInputByLabel("DOPE chat text", "Private live draft before simulation");
    expect(await browser.evaluate(`document.querySelector('.git-chat button[type=submit]').disabled`)).toBe(false);
    const before = await browser.evaluate<{ reads: number; writes: number }>(`({reads:window.__chatGetCount,writes:window.__chatRequests.length})`);
    await browser.evaluate(`(() => {
      const next={...window.__chatViewSnapshot,revision:'chat-synthetic-cut',simulated:true};
      window.__chatViewSnapshot=next;
      window.__chatViewController.enqueue(new TextEncoder().encode('event: views\\nid: '+next.revision+'\\ndata: '+JSON.stringify(next)+'\\n\\n'));
    })()`);
    await browser.waitForSelector(".lv-demo");
    expect(await browser.evaluate(`document.querySelector('.git-chat-toggle').getAttribute('aria-expanded')`)).toBe("false");
    await browser.clickButtonByText("DOPE · Chat");
    await browser.waitForText("Operator chat is disabled for simulated observations.");
    expect(await browser.evaluate(`document.querySelector('.git-chat-destination').textContent`)).not.toContain("Exact target:");
    expect(await browser.evaluate(`document.querySelector('.git-chat textarea').value`)).toBe("");
    await browser.fillInputByLabel("DOPE chat text", "Attempt from simulated view");
    expect(await browser.evaluate(`document.querySelector('.git-chat button[type=submit]').disabled`)).toBe(true);
    await browser.evaluate(`document.querySelector('.git-chat form').dispatchEvent(new Event('submit',{bubbles:true,cancelable:true}))`);
    await browser.evaluate(`new Promise(resolve=>setTimeout(resolve,3200))`);
    expect(await browser.evaluate(`({reads:window.__chatGetCount,writes:window.__chatRequests.length})`)).toEqual(before);
    await browser.evaluate(`(() => {
      const next={...window.__chatViewSnapshot,revision:'chat-live-restored',simulated:false};
      window.__chatViewSnapshot=next;
      window.__chatViewController.enqueue(new TextEncoder().encode('event: views\\nid: '+next.revision+'\\ndata: '+JSON.stringify(next)+'\\n\\n'));
    })()`);
    await browser.waitForSelector(".lv-feed.is-connected");
    await browser.evaluate(`new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))`);
    expect(await browser.evaluate(`document.querySelector('.git-chat-toggle').getAttribute('aria-expanded')`)).toBe("false");
    await browser.clickButtonByText("DOPE · Chat");
    await browser.waitForText("Exact target: captain:dope");
    expect(await browser.evaluate(`document.querySelector('.git-chat textarea').value`)).toBe("");
    expect(await browser.evaluate<number>(`window.__chatGetCount`)).toBeGreaterThan(before.reads);
    await browser.fillInputByLabel("DOPE chat text", "Restored live message");
    await browser.clickButtonByText("Send message");
    await browser.waitForText("Delivered to inbox; awaiting reply");
    expect(await browser.evaluate<number>(`window.__chatRequests.length`)).toBe(before.writes + 1);
  }, 20_000);
});
