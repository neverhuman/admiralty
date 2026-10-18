#!/usr/bin/env python3
"""Dashboard v2 extras (read-only): super_captain activity, global resources, hierarchy, token efficiency.
Imported by collector.py: out.update(extras.collect(out, screens, helpers)). Never stuffs screens."""
import os, re, json, time, glob, datetime, subprocess, collections
HOME = '/home/ubuntu'
BASE = os.environ.get('CDASH_RUNTIME', '/home/ubuntu/xbabe2_kicker/captains-dashboard')  # runtime data dir (status.json, history, tokens.db); code lives in admiralty/dashboard
CODE = os.path.dirname(os.path.realpath(__file__))
CAPD = HOME + '/xbabe2_kicker/captains-20261017'
SC_STATUS = CAPD + '/super_captain_status.json'
SC_LOG = CAPD + '/super_captain_LOG.md'
SC_TICKLOG = CAPD + '/super_captain_tick.log'
SC_DEC = CAPD + '/super_captain_DECISIONS.md'
SC_INBOX = CAPD + '/super_captain_INBOX.md'
GRES = BASE + '/global_resources.json'
QHIST = BASE + '/.quota_history.json'
SESS = HOME + '/.codex/sessions'
COORD = {'joil': HOME + '/joil/COORD.md', 'dope': HOME + '/dope/COMMS.md', 'jopedime': HOME + '/JopeDime/COMMS.md', 'jarambit': HOME + '/jarambit/COMMS.md'}
REPOS = {'joil': HOME + '/joil', 'dope': HOME + '/dope', 'jopedime': HOME + '/JopeDime', 'jarambit': HOME + '/jarambit'}
# Published Codex credit rates per 1M tokens (Standard speed): (uncached input, cached input, output).
# Source of truth: captains/rates/codex_rates_*.json (newest file wins); the literal below is only a fallback.
_RATES_FALLBACK = {'gpt-6-astra': (250, 25, 1250), 'gpt-5.6-sol': (100, 10, 500), 'gpt-5.6-terra': (50, 5, 300),
         'gpt-6.1-sol': (50, 2.5, 250), 'gpt-6-sol': (50, 5, 250), 'gpt-5.6-luna': (5, 0.5, 30), 'gpt-6-luna': (2.5, 0.25, 12.5)}
RATES_DIR = os.environ.get('ADMIRALTY_RATES_DIR', os.path.join(os.path.dirname(os.path.dirname(os.path.realpath(__file__))), 'captains', 'rates'))

def load_rate_file(vendor):
    """Return (path, parsed json) for the newest captains/rates/<vendor>_rates_*.json, or (None, None)."""
    try:
        fs = sorted(f for f in os.listdir(RATES_DIR) if f.startswith(vendor + '_rates_') and f.endswith('.json'))
        if not fs: return None, None
        p = os.path.join(RATES_DIR, fs[-1])
        with open(p) as fh: return p, json.load(fh)
    except Exception:
        return None, None

def _load_codex_rates():
    p, d = load_rate_file('codex')
    try:
        r = {m['model']: (float(m['input']), float(m['cached_input']), float(m['output'])) for m in d['models']}
        if 'gpt-6-luna' in r: return r, p
    except Exception:
        pass
    return dict(_RATES_FALLBACK), None

RATES, RATES_SOURCE = _load_codex_rates()
DOMINATED = {'gpt-6-sol', 'gpt-5.6-sol', 'gpt-5.6-terra', 'gpt-5.6-luna'}
ROUTINES = ['Joil drive via captain_joil (30 min)', 'DOPE workplan drive via captain_dope (30 min)',
            'JopeDime self-play workplan drive via captain_jopedime (30 min)', 'Jarambit drive via captain_jarambit (30 min)']
NAMES = {'joil': 'Joil', 'dope': 'DOPE', 'jopedime': 'JopeDime', 'jarambit': 'Jarambit'}

def now(): return datetime.datetime.now(datetime.timezone.utc)
def iso(t=None): return (t or now()).strftime('%Y-%m-%dT%H:%M:%SZ')
def mtime_iso(p):
    try: return datetime.datetime.fromtimestamp(os.path.getmtime(p), datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')
    except Exception: return None
def rj(p):
    try:
        with open(p) as f: return json.load(f)
    except Exception: return None
def parse_ts(s):
    try: return datetime.datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp()
    except Exception: return None

# ---------------- model normalisation / tiers ----------------
def norm_model(kind, model, effort):
    """Return (family, model_id, effort). family in codex|claude|agy|gemini|unknown"""
    m = (model or '').strip(); e = effort
    if kind == 'agy': return 'agy', 'agy ' + (m or 'Gemini (model unknown)'), e
    if kind == 'claude':
        ml = m.lower()
        for a in ('opus', 'sonnet', 'fable', 'haiku'):
            if a in ml: return 'claude', 'claude ' + a, e
        return 'claude', 'claude (model unknown)', e
    if kind == 'codex':
        mm = re.match(r'(?i)(gpt-[\w.\-]+?)(?:\s+(low|medium|high|xhigh|max|ultra))?$', m.replace('GPT', 'gpt').replace('Gpt', 'gpt'))
        if mm:
            mid = mm.group(1).lower(); e = e or mm.group(2)
            return 'codex', mid, (e or '').lower() or None
        return 'codex', 'codex (model unknown)', e
    if kind == 'gemini': return 'gemini', 'gemini ' + m, e
    return 'unknown', m or 'unknown', e

def codex_tier(mid, eff):
    if mid == 'gpt-6-luna': return 'T1' if eff == 'high' else 'T0' if eff in ('medium', 'low', None) else 'T0/T1?'
    if mid == 'gpt-6.1-sol': return 'T3' if eff == 'xhigh' else 'T2' if eff in ('high', 'medium', 'low') else 'T2/T3?'
    if mid == 'gpt-6-astra': return 'T4'
    if mid in DOMINATED: return 'dominated'
    return '?'

# ---------------- codex sessions (real token data) ----------------
TYPROC = {}
SESS_STATE = {}   # path -> {'off', 'events': [(ts, inp, cached, out, reasoning)], 'model', 'effort', 'tier', 'cwd', 'rl'}
def scan_file(p):
    st = SESS_STATE.setdefault(p, {'off': 0, 'events': [], 'model': None, 'effort': None, 'service_tier': None, 'cwd': None, 'rl': None, 'rl_ts': None})
    try:
        size = os.path.getsize(p)
        if size < st['off']: st.update(off=0, events=[])
        if size == st['off']: return st
        with open(p, 'rb') as f:
            f.seek(st['off']); data = f.read(size - st['off'])
        cut = data.rfind(b'\n')
        if cut < 0: return st
        st['off'] += cut + 1
        for line in data[:cut].split(b'\n'):
            if b'"token_count"' in line:
                try: d = json.loads(line)
                except Exception: continue
                pl = d.get('payload') or {}; info = pl.get('info') or {}; tu = info.get('total_token_usage') or {}
                ts = parse_ts(d.get('timestamp', ''))
                if tu and ts:
                    st['events'].append((ts, tu.get('input_tokens', 0), tu.get('cached_input_tokens', 0), tu.get('output_tokens', 0), tu.get('reasoning_output_tokens', 0)))
                if pl.get('rate_limits') and ts: st['rl'] = pl['rate_limits']; st['rl_ts'] = ts
            elif b'"turn_context"' in line or b'"session_meta"' in line:
                try: d = json.loads(line)
                except Exception: continue
                pl = d.get('payload') or {}
                if d.get('type') == 'session_meta': st['cwd'] = pl.get('cwd') or st['cwd']; continue
                st['cwd'] = pl.get('cwd') or st['cwd']
                st['model'] = pl.get('model') or st['model']
                cm = ((pl.get('collaboration_mode') or {}).get('settings') or {})
                st['effort'] = cm.get('reasoning_effort') or pl.get('effort') or pl.get('reasoning_effort') or st['effort']
                for k in ('service_tier', 'speed'):
                    if pl.get(k): st['service_tier'] = pl.get(k)
        if len(st['events']) > 4000: st['events'] = st['events'][-4000:]
    except Exception:
        pass
    return st

def recent_session_files(hours=3):
    out = []; cutoff = time.time() - hours * 3600
    for day in (now(), now() - datetime.timedelta(days=1)):
        for p in glob.glob('%s/%04d/%02d/%02d/rollout-*.jsonl' % (SESS, day.year, day.month, day.day)):
            try:
                if os.path.getmtime(p) >= cutoff: out.append(p)
            except Exception: pass
    return out

def credits(mid, inp, cached, out):
    r = RATES.get(mid)
    if not r: return None
    return (max(0, inp - cached) * r[0] + cached * r[1] + out * r[2]) / 1e6

def at_or_before(events, t):
    best = None
    for e in events:
        if e[0] <= t: best = e
        else: break
    return best

def session_for_pid(pids):
    for p in pids:
        try:
            for fd in os.listdir('/proc/%d/fd' % p):
                try:
                    l = os.readlink('/proc/%d/fd/%s' % (p, fd))
                except Exception: continue
                if '/.codex/sessions/' in l and l.endswith('.jsonl'): return l
        except Exception: continue
    return None

# ---------------- ledgers: LANE lines + MODEL usage ----------------
LANE_RE = re.compile(r'LANE\s+`?(\S+?)`?\s*\|\s*screen\s+`?(\S+?)`?\s*\|\s*model\s+(\S+)\s+(\S+)\s*\(\s*(T\d)\s*,\s*(simple|medium|hard)\s*\)(?:.*?\bhost\s+(\S+))?', re.I)
MODEL_RE = re.compile(r'MODEL\s+(\S+)\s*\|\s*task\s+(\S+)\s*\|\s*class\s+(\w+)\s*\|\s*tier\s+(T\d)\s*\|\s*model\s+(\S+)\s*(\S*)\s*\|\s*screen\s+(\S+)\s*\|\s*outcome\s+(\w+)', re.I)
def tail_text(p, nbytes=600000):
    try:
        with open(p, 'rb') as f:
            f.seek(0, 2); s = f.tell(); f.seek(max(0, s - nbytes)); return f.read().decode('utf-8', 'replace')
    except Exception: return ''
# Canonical ledger schema (playbook §4e/§9): ts, project, task, screen, host, tier, model, effort,
# outcome (started|pass|fail|escalated|deescalated; legacy loop|timeout accepted), escalated_to, reason, tokens.
# Legacy §9 lines (utc, to_tier, note, class, ...) are mapped so old lines still count.
def norm_usage(d, project=None):
    ts = d.get('ts') or d.get('utc')
    esc = d.get('escalated_to') if d.get('escalated_to') is not None else d.get('to_tier')
    return {'project': d.get('project') or project, 'utc': ts, 'ts': ts, 'task': d.get('task'), 'class': str(d.get('class', '') or '').lower(),
            'tier': str(d.get('tier', '') or '').upper(), 'model': d.get('model'), 'effort': d.get('effort'),
            'outcome': str(d.get('outcome', '') or '').lower(), 'screen': d.get('screen'), 'host': d.get('host'),
            'escalated_to': esc, 'to_tier': esc, 'reason': d.get('reason') or d.get('note'), 'tokens': d.get('tokens')}

def ledgers():
    lanes = {}; usage = []
    for k, p in COORD.items():
        t = tail_text(p)
        for m in LANE_RE.finditer(t):
            lanes[m.group(2)] = {'project': k, 'task': m.group(1), 'model': m.group(3), 'effort': m.group(4), 'tier': m.group(5).upper(), 'class': m.group(6).lower(), 'host': m.group(7)}
        for m in MODEL_RE.finditer(t):
            usage.append({'project': k, 'utc': m.group(1), 'task': m.group(2), 'class': m.group(3).lower(), 'tier': m.group(4).upper(), 'model': m.group(5), 'outcome': m.group(8).lower(), 'screen': m.group(7)})
        files = glob.glob(REPOS[k] + '/.agent/model_usage.jsonl') + glob.glob(REPOS[k] + '/.agent/worktrees/*/.agent/model_usage.jsonl') + glob.glob(REPOS[k] + '/.private/v5-workers/*/model_usage.jsonl')
        for f in files[:60]:
            for line in tail_text(f, 200000).splitlines():
                try: d = json.loads(line)
                except Exception: continue
                if isinstance(d, dict):
                    usage.append(norm_usage(d, k))
    for line in tail_text(CENTRAL_LEDGER, 2000000).splitlines():
        try: d = json.loads(line)
        except Exception: continue
        if isinstance(d, dict):
            usage.append(norm_usage(d))
    return lanes, usage


HC_DIR = BASE + '/.hc'
FOOT = {'codex': re.compile(r'(?i)\b(gpt-[0-9.]+-[a-z]+)\s+(low|medium|high|xhigh|max|ultra)\b'),
        'claude': re.compile(r'(?i)\b(opus|sonnet|fable|haiku)(?:\s*[0-9.]+)?'),
        'agy': re.compile(r'(Gemini [0-9.]+ [A-Za-z]+)')}
def hc_model(sid, kind):
    r = FOOT.get(kind)
    if not r: return None
    try:
        t = open(os.path.join(HC_DIR, sid.replace('/', '_') + '.txt'), errors='replace').read()[-3000:]
    except Exception: return None
    ms = r.findall(t)
    if not ms: return None
    m = ms[-1]
    return ' '.join(m).lower() if isinstance(m, tuple) and kind == 'codex' else (m if isinstance(m, str) else m[0])
def _cfg(path, key):
    try:
        for line in open(path):
            mm = re.match(r'\s*"?%s"?\s*[:=]\s*"([^"]+)"' % key, line)
            if mm: return mm.group(1)
    except Exception: pass
    return None
def default_model(ap):
    if ap['kind'] == 'codex':
        m = _cfg(HOME + '/.codex/config.toml', 'model'); e = _cfg(HOME + '/.codex/config.toml', 'model_reasoning_effort')
        return (m + (' ' + e if e else '')) if m else None
    if ap['kind'] == 'claude':
        return _cfg(HOME + '/.claude/settings.json', 'model')
    return None

# ---------------- main ----------------
def refresh_resources():
    g = rj(GRES)
    age = time.time() - os.path.getmtime(GRES) if os.path.exists(GRES) else 1e9
    if age > 120:
        try:
            subprocess.run(['python3', '-B', CODE + '/globalres.py', '--quiet'], timeout=90, capture_output=True)
            g = rj(GRES)
        except Exception: pass
    return g

def worker_state(w):
    if not w.get('alive'): return 'gone'
    if not w.get('agent_running'): return 'finished'
    tail = ' '.join(w.get('tail') or []).lower()
    if re.search(r"usage limit|rate limit reached|quota (exhausted|exceeded)|you've hit your|needs jepson", tail): return 'blocked'
    ch = parse_ts(w.get('screen_changed_utc') or '')
    if ch and time.time() - ch < 300: return 'active'
    return 'idle'

def collect(out, screens, helpers):
    agent_proc = helpers['agent_proc']; descendants = helpers['descendants']; screen_info = helpers['screen_info']; project_of = helpers['project_of']
    res = {}
    # ---- super captain ----
    sc_scr = next(((k, v) for k, v in screens.items() if v['name'] == 'super_captain'), (None, None))
    tick = next(((k, v) for k, v in screens.items() if v['name'] == 'super_captain_tick'), (None, None))
    sc = {'screen': screen_info(sc_scr[0], sc_scr[1], True, None) if sc_scr[0] else {'name': 'super_captain', 'alive': False},
          'ticker': {'screen': tick[0], 'alive': bool(tick[0]), 'log_tail': tail_text(SC_TICKLOG, 4000).strip().splitlines()[-4:]},
          'status': rj(SC_STATUS), 'status_mtime_utc': mtime_iso(SC_STATUS), 'log_mtime_utc': mtime_iso(SC_LOG)}
    lt = [l for l in tail_text(SC_LOG, 20000).splitlines() if l.strip()]
    sc['log_tail'] = lt[-30:]
    checks = re.findall(r'^## CHECK\s+(\d+)\s*\|\s*(\S+)', tail_text(SC_LOG, 400000), re.M)
    sc['checks_logged'] = len(checks); sc['last_check'] = {'no': checks[-1][0], 'utc': checks[-1][1]} if checks else None
    decs = []
    for blk in re.split(r'(?m)^(?=## SC-DEC-)', tail_text(SC_DEC, 200000)):
        m = re.match(r'## (SC-DEC-\d+)\s*\|\s*([^|]+)\|\s*([^|]+)\|\s*(.+)', blk)
        if not m: continue
        f = dict((k.lower(), v.strip()) for k, v in re.findall(r'(?m)^-\s*(Question|Decision|Reversibility|Rationale|Affects):\s*(.+)$', blk))
        decs.append({'id': m.group(1), 'when': m.group(2).strip(), 'project': m.group(3).strip(), 'title': m.group(4).strip()[:200],
                     'question': f.get('question', '')[:300], 'decision': f.get('decision', '')[:400], 'reversibility': f.get('reversibility', '')[:160]})
    sc['decisions'] = decs[-12:]; sc['decisions_total'] = len(decs); sc['decisions_mtime_utc'] = mtime_iso(SC_DEC)
    inbox = tail_text(SC_INBOX, 100000)
    sc['inbox_items'] = len(re.findall(r'(?m)^## \d{4}-', inbox)); sc['inbox_mtime_utc'] = mtime_iso(SC_INBOX)
    res['super_captain'] = sc
    # ---- resources ----
    g = refresh_resources(); res['global_resources'] = g
    TY = BASE + '/token_yield.json'
    try:
        age = time.time() - os.path.getmtime(TY) if os.path.exists(TY) else 1e9
        pr = TYPROC.get('p')
        if age > 600 and (pr is None or pr.poll() is not None):
            TYPROC['p'] = subprocess.Popen(['nice', '-n', '19', 'ionice', '-c3', 'python3', '-B', CODE + '/token_yield.py'], stdout=subprocess.DEVNULL, stderr=open(BASE + '/token_yield.err', 'a'))
    except Exception: pass
    res['token_yield'] = rj(TY)
    # ---- agents (all screens on xbabe2 with an agent process) ----
    lanes, usage = ledgers()
    files = recent_session_files()
    for p in files: scan_file(p)
    for p in list(SESS_STATE):
        if p not in files and not os.path.exists(p): SESS_STATE.pop(p, None)
    winfo = {}
    for key, p in (out.get('projects') or {}).items():
        for w in [p.get('captain') or {}] + (p.get('workers') or []):
            if w.get('id'): winfo[w['id']] = (key, w)
    if sc_scr[0]: winfo[sc_scr[0]] = ('super', sc['screen'])
    agents = []
    for sid, s in screens.items():
        ap = agent_proc(s['pid'])
        if not ap: continue
        key, w = winfo.get(sid, (project_of(s['name']) or 'unassigned', {}))
        model = ap.get('model'); eff = ap.get('effort')
        sess = None; sst = None
        if ap['kind'] == 'codex':
            sess = session_for_pid([ap['pid']] + descendants(ap['pid']))
            if sess:
                sst = scan_file(sess)
                model = sst.get('model') or model; eff = sst.get('effort') or eff
        src_override = None
        if not model:
            hc = hc_model(sid, ap['kind'])
            if hc: model, src_override = hc, 'screen footer'
        if not model and w.get('model') and w['model'] not in ('codex', 'claude', 'agy'):
            model, src_override = w['model'], 'screen footer'
        if not model:
            dflt = default_model(ap)
            if dflt: model, src_override = dflt, 'inferred: no --model flag, CLI default config'
        fam, mid, eff = norm_model(ap['kind'], model, eff)
        if fam == 'claude' and not eff and src_override and src_override.startswith('inferred'):
            eff = _cfg(HOME + '/.claude/settings.json', 'effortLevel')
        a = {'screen': s['name'], 'id': sid, 'project': key, 'host': 'xbabe2', 'family': fam, 'model': mid, 'effort': eff,
             'model_source': 'codex session turn_context' if sst and sst.get('model') else ('process args' if ap.get('model') else (src_override or 'undetected')),
             'tier': codex_tier(mid, eff) if fam == 'codex' else ('planner' if fam == 'agy' else fam),
             'state': worker_state(w) if w else 'unknown', 'task': w.get('task'), 'flags': []}
        if sst:
            ev = sst['events']
            a['session'] = os.path.basename(sess)
            if ev:
                a['tokens_total'] = {'input': ev[-1][1], 'cached': ev[-1][2], 'output': ev[-1][3]}
                a['last_token_utc'] = iso(datetime.datetime.fromtimestamp(ev[-1][0], datetime.timezone.utc))
            if sst.get('service_tier'): a['service_tier'] = sst['service_tier']
        ln = lanes.get(s['name'])
        if ln: a['lane'] = ln; a['class'] = ln['class']; a['task'] = a['task'] or ln['task']; a['host'] = ln.get('host') or 'xbabe2'
        if fam == 'codex':
            if mid in DOMINATED: a['flags'].append('dominated model %s (playbook: do not use)' % mid)
            if eff in ('max', 'ultra'): a['flags'].append('effort %s (playbook: never)' % eff)
            stier = str(a.get('service_tier') or '').lower()
            if stier and stier not in ('default', 'standard', 'auto', 'flex'): a['flags'].append('speed/service tier %s (playbook: Standard only)' % stier)
            if a.get('class') == 'simple' and ('sol' in mid or 'astra' in mid): a['flags'].append('%s on a SIMPLE task' % mid)
        if fam == 'claude' and a.get('class') == 'simple' and 'opus' in mid: a['flags'].append('opus on a SIMPLE task')
        agents.append(a)
    # remote screens from globalres (names only)
    remote = []
    for h in (g or {}).get('hosts', []):
        if h.get('host') == 'xbabe2': continue
        for sname in h.get('screens', []):
            n = sname.split('.', 1)[-1]
            remote.append({'screen': n, 'id': sname, 'host': h['host'], 'project': project_of(n) or 'unassigned'})
    res['remote_screens'] = remote
    # ---- token efficiency ----
    by_model = collections.Counter(a['model'] + ((' ' + a['effort']) if a.get('effort') and a['family'] == 'codex' else '') for a in agents)
    codex_ag = [a for a in agents if a['family'] == 'codex']
    working = [a for a in codex_ag if a['state'] in ('active', 'idle', 'blocked', 'unknown')]
    cheap = [a for a in codex_ag if a['model'] in ('gpt-6-luna',)]
    out_rates = [RATES[a['model']][2] for a in codex_ag if a['model'] in RATES]
    tiers = collections.Counter(a['tier'] for a in codex_ag)
    T = time.time()
    win = {}
    for w_min in (15, 60):
        t0 = T - w_min * 60; agg = collections.defaultdict(lambda: {'input': 0, 'cached': 0, 'output': 0, 'credits': 0.0, 'sessions': 0})
        for p, st in SESS_STATE.items():
            ev = st['events']
            if not ev or ev[-1][0] < t0: continue
            base = at_or_before(ev, t0) or (0, 0, 0, 0, 0)
            last = ev[-1]; mid = st.get('model') or 'unknown'
            di, dc, do = last[1] - base[1], last[2] - base[2], last[3] - base[3]
            if di < 0 or do < 0: continue
            a = agg[mid]; a['input'] += di; a['cached'] += dc; a['output'] += do; a['sessions'] += 1
            c = credits(mid, di, dc, do); a['credits'] += c if c is not None else 0
            if c is None: a['unpriced'] = True
        tot_c = sum(v['credits'] for v in agg.values())
        win[str(w_min)] = {'by_model': {k: dict(v, credits=round(v['credits'], 2)) for k, v in agg.items()},
                           'credits_est': round(tot_c, 2), 'credits_per_hour_est': round(tot_c * 60 / w_min, 1),
                           'output_tokens': sum(v['output'] for v in agg.values()), 'input_tokens': sum(v['input'] for v in agg.values())}
    # quota (real, from the latest rate_limits event of any session)
    latest = max((st for st in SESS_STATE.values() if st.get('rl')), key=lambda st: st['rl_ts'], default=None)
    quota = None
    qh = rj(QHIST) or []
    if latest:
        rl = latest['rl']; pr = rl.get('primary') or {}; cr = rl.get('credits') or {}
        quota = {'used_percent': pr.get('used_percent'), 'window_minutes': pr.get('window_minutes'), 'resets_at_utc': iso(datetime.datetime.fromtimestamp(pr['resets_at'], datetime.timezone.utc)) if pr.get('resets_at') else None,
                 'credits_balance': cr.get('balance'), 'plan_type': rl.get('plan_type'), 'limit_reached': rl.get('rate_limit_reached_type'), 'observed_utc': iso(datetime.datetime.fromtimestamp(latest['rl_ts'], datetime.timezone.utc))}
        if pr.get('used_percent') is not None and (not qh or qh[-1][0] < latest['rl_ts']):
            try: bal = float(cr.get('balance')) if cr.get('balance') is not None else None
            except Exception: bal = None
            qh.append([latest['rl_ts'], pr['used_percent'], bal])
        qh = [x for x in qh if T - x[0] < 12 * 3600]
        try:
            with open(QHIST + '.tmp', 'w') as f: json.dump(qh, f)
            os.replace(QHIST + '.tmp', QHIST)
        except Exception: pass
        base = next((x for x in qh if T - x[0] <= 3 * 3600 and T - x[0] >= 20 * 60), None)
        if base and qh[-1][0] > base[0]:
            rate = (qh[-1][1] - base[1]) / ((qh[-1][0] - base[0]) / 3600)
            quota['burn_pct_per_hour'] = round(rate, 2); quota['burn_window_min'] = round((qh[-1][0] - base[0]) / 60)
            if rate > 0: quota['hours_to_100pct'] = round((100 - qh[-1][1]) / rate, 1)
        quota['history'] = [[iso(datetime.datetime.fromtimestamp(x[0], datetime.timezone.utc)), x[1]] for x in qh[-60:]]
    esc_counts = collections.Counter(u['outcome'] for u in usage)
    tier_usage = collections.Counter(u['tier'] for u in usage)
    class_tier = collections.Counter('%s@%s' % (u['class'], u['tier']) for u in usage)
    res['token_efficiency'] = {
        'agents_total': len(agents), 'by_family': dict(collections.Counter(a['family'] for a in agents)),
        'by_model': dict(by_model.most_common()), 'codex_tiers': dict(tiers),
        'codex_agents': len(codex_ag), 'codex_on_cheapest': len(cheap),
        'pct_codex_on_cheapest': round(100.0 * len(cheap) / len(codex_ag), 1) if codex_ag else None,
        'cost_index_avg_output_rate': round(sum(out_rates) / len(out_rates), 1) if out_rates else None,
        'cost_index_vs_all_luna': round(sum(out_rates) / len(out_rates) / 12.5, 1) if out_rates else None,
        'cost_index_note': 'Average published output credit rate per 1M tokens across live Codex agents (Luna=12.5 ... Astra=1250); 1.0x = every agent on gpt-6-luna. Per-agent rate, not weighted by tokens.',
        'windows': win, 'windows_note': 'Real token counts from ~/.codex/sessions token_count events (all Codex sessions on xbabe2, last 3h of files). Credits are an ESTIMATE using published credit rates; OpenAI does not publish exact Pro-quota per token.',
        'quota': quota, 'quota_note': 'used_percent / credits balance are real values reported by Codex (rate_limits in session events). Burn rate is computed from those samples.',
        'flags': [{'screen': a['screen'], 'project': a['project'], 'text': f} for a in agents for f in a['flags']],
        'ledger': {'model_lines': len(usage), 'outcomes': dict(esc_counts), 'tiers': dict(tier_usage), 'class_at_tier': dict(class_tier),
                   'escalated': esc_counts.get('escalated', 0), 'lane_lines_with_class': len(lanes),
                   'note': 'From MODEL lines in coordination logs and .agent/model_usage.jsonl (playbook §4c)' if usage else 'No MODEL usage lines found yet (playbook §4c) — escalation counts unavailable'},
        'rates_per_1m': {k: {'input': v[0], 'cached': v[1], 'output': v[2]} for k, v in RATES.items()},
        'rates_source': RATES_SOURCE or 'built-in fallback',
        'rate_files': {v: load_rate_file(v)[0] for v in ('codex', 'claude', 'grok', 'gemini')},
        'claude_note': 'Claude/agy usage is not on Codex quota; token counts for them are not tracked here.'}
    res['token_efficiency']['ledger_stats'] = ledger_stats(usage)
    res['agents'] = agents
    # ---- hierarchy ----
    amap = {a['id']: a for a in agents}
    caps = []
    for key, p in (out.get('projects') or {}).items():
        c = p.get('captain') or {}
        ca = amap.get(c.get('id'), {})
        ws = []
        for w in p.get('workers') or []:
            a = amap.get(w.get('id'))
            ws.append({'screen': w.get('name'), 'host': (a or {}).get('host', 'xbabe2'), 'model': (a or {}).get('model') or w.get('model') or 'unknown',
                       'effort': (a or {}).get('effort'), 'tier': (a or {}).get('tier') or ('—' if not w.get('agent_running') else 'unknown'),
                       'task': w.get('task') or (a or {}).get('task'), 'state': worker_state(w), 'flags': (a or {}).get('flags', [])})
        for r in remote:
            if r['project'] == key: ws.append({'screen': r['screen'], 'host': r['host'], 'model': 'unknown (remote; names only)', 'tier': 'unknown', 'task': None, 'state': 'remote'})
        caps.append({'key': key, 'name': NAMES.get(key, key), 'screen': c.get('id'), 'model': ca.get('model') or c.get('model') or 'unknown', 'state': worker_state(c) if c else 'gone',
                     'counts': p.get('counts'), 'done': p.get('done'), 'total': p.get('total'), 'workers': ws})
    una = [a for a in agents if a['project'] == 'unassigned']
    res['hierarchy'] = {'jepson': 'Jepson', 'grokbot': {'name': 'Grok Bot (ENG:watcher)', 'routines': ROUTINES},
                        'super_captain': {'screen': sc['screen'].get('id') or 'super_captain', 'alive': sc['screen'].get('alive'), 'model': (amap.get(sc_scr[0]) or {}).get('model') or sc['screen'].get('model') or 'unknown',
                                          'state': worker_state(sc['screen']) if sc_scr[0] else 'gone', 'ticker_alive': bool(tick[0])},
                        'captains': caps, 'unassigned': [{'screen': a['screen'], 'model': a['model'], 'effort': a.get('effort'), 'state': a['state']} for a in una] +
                        [{'screen': r['screen'] + ' @' + r['host'], 'model': 'unknown', 'state': 'remote'} for r in remote if r['project'] == 'unassigned']}
    try:
        history_update(res, out)
    except Exception as e:
        res['history_error'] = str(e)[:200]
    return res

# ---------------- history (time series for header sparklines) ----------------
HIST_JSONL = BASE + '/history.jsonl'
HIST_STATE = {'last': 0, 'rows': None}
CENTRAL_LEDGER = CAPD + '/model_usage.jsonl'
LUNA_RATES = RATES['gpt-6-luna']

def tier_group(a):
    if a['family'] == 'agy': return 'agy'
    if a['family'] == 'claude': return 'claude'
    if a['family'] == 'codex':
        m = a['model']
        if 'luna' in m: return 'luna'
        if 'astra' in m: return 'astra'
        if 'sol' in m or 'terra' in m: return 'sol'
    return 'other'

def is_luna(u): return 'luna' in str(u.get('model') or '').lower() or str(u.get('tier') or '').upper() in ('T0', 'T1')
def tier_rank(u):
    m = str(u.get('model') or '').lower(); t = str(u.get('tier') or '').upper()
    if 'astra' in m or t == 'T4': return 2
    if 'sol' in m or t in ('T2', 'T3'): return 1
    return 0

def ledger_stats(usage):
    ev = []
    for u in usage:
        ts = parse_ts(str(u.get('utc') or ''))
        if ts: ev.append((ts, u))
    ev.sort(key=lambda x: x[0])
    T = time.time(); hours = [0] * 24; cum = []; c = 0
    by_task = collections.OrderedDict()
    for ts, u in ev:
        key = (u.get('project'), u.get('task'))
        by_task.setdefault(key, []).append(u)
        if is_luna(u) and u.get('outcome') in ('fail', 'loop', 'timeout', 'escalated'):
            c += 1; cum.append([iso(datetime.datetime.fromtimestamp(ts, datetime.timezone.utc)), c])
            h = int((T - ts) // 3600)
            if 0 <= h < 24: hours[23 - h] += 1
    JUDGED = ('pass', 'fail', 'loop', 'timeout', 'escalated')
    judged = {k2: [x for x in v if x.get('outcome') in JUDGED] for k2, v in by_task.items()}
    luna_tasks = [v for v in judged.values() if v and is_luna(v[0])]
    first_pass = sum(1 for v in luna_tasks if v[0].get('outcome') == 'pass')
    funnel = {'luna_tasks': len(luna_tasks),
              'luna_to_sol': sum(1 for v in by_task.values() if any(tier_rank(x) == 0 for x in v) and (any(tier_rank(x) >= 1 for x in v) or any(tier_rank(x) == 0 and x.get('outcome') == 'escalated' for x in v))),
              'sol_tasks': sum(1 for v in by_task.values() if any(tier_rank(x) == 1 for x in v)),
              'sol_to_astra': sum(1 for v in by_task.values() if any(tier_rank(x) == 1 for x in v) and any(tier_rank(x) == 2 for x in v)),
              'astra_tasks': sum(1 for v in by_task.values() if any(tier_rank(x) == 2 for x in v))}
    return {'attempts': len(ev), 'luna_failures_cum': cum[-200:], 'luna_failures_total': c, 'luna_failures_per_hour_24h': hours,
            'luna_first_pass_rate': round(100.0 * first_pass / len(luna_tasks), 1) if luna_tasks else None,
            'luna_first_pass': first_pass, 'funnel': funnel}

def cost_index_now(res):
    te = res['token_efficiency']; w = (te.get('windows') or {}).get('15') or {}
    act = 0.0; luna = 0.0
    for mid, v in (w.get('by_model') or {}).items():
        if mid not in RATES: continue
        act += v['credits']; luna += credits('gpt-6-luna', v['input'], v['cached'], v['output'])
    if luna > 0: return round(act / luna, 2), 'tokens (last 15 min, real token counts; Codex only)'
    if te.get('cost_index_vs_all_luna') is not None: return te['cost_index_vs_all_luna'], 'agent-count weighted (no recent token events)'
    return None, 'no data'

def load_hist():
    if HIST_STATE['rows'] is None:
        rows = []
        try:
            for line in open(HIST_JSONL):
                try: rows.append(json.loads(line))
                except Exception: pass
        except Exception: pass
        HIST_STATE['rows'] = rows; HIST_STATE['last'] = rows[-1]['ts'] if rows else 0
    return HIST_STATE['rows']

def downsample(rows, T):
    keep = []; last_bucket = None
    for r in rows:
        age = T - r['ts']
        if age > 7 * 86400: continue
        if age <= 6 * 3600: keep.append(r); continue
        b = int(r['ts'] // 900)
        if b != last_bucket: keep.append(r); last_bucket = b
    return keep

def history_update(res, out):
    rows = load_hist(); T = time.time()
    if T - HIST_STATE['last'] >= 110:
        te = res['token_efficiency']; ci, basis = cost_index_now(res)
        tg = collections.Counter(tier_group(a) for a in res.get('agents', []))
        scst = ((res.get('super_captain') or {}).get('status') or {}).get('captains') or {}
        lst = te.get('ledger_stats') or {}
        snap = {'ts': round(T), 'cost_index': ci, 'cost_basis': basis, 'tiers': dict(tg),
                'quota_pct': (te.get('quota') or {}).get('used_percent'),
                'credits_h': ((te.get('windows') or {}).get('60') or {}).get('credits_per_hour_est'),
                'luna_fail_total': lst.get('luna_failures_total'), 'luna_fp_rate': lst.get('luna_first_pass_rate'),
                'proj': {}, 'hosts': {}, 'hyg': {}}
        ty = res.get('token_yield') or {}
        if ty.get('total'):
            tt = ty['total']; snap['ty'] = {'tok': tt.get('tokens'), 'out': tt.get('output'), 'cr': tt.get('credits'), 'int': tt.get('integration_tokens'), 'main': tt.get('main_tokens'),
                                           'yi': tt.get('yield_integration_pct'), 'ym': tt.get('yield_main_pct'), 'cov': tt.get('coverage_pct'),
                                           'p': {k: [v.get('tokens'), v.get('integration_tokens'), v.get('main_tokens'), v.get('credits_per_done_todo')] for k, v in (ty.get('projects') or {}).items()}}
        for k, p in (out.get('projects') or {}).items():
            c = p.get('counts') or {}; ci_ = ((p.get('pr') or {}).get('ci') or {})
            snap['proj'][k] = {'done': p.get('done'), 'total': p.get('total'), 'ip': c.get('in_progress'), 'ci': ci_.get('state'), 'pr': (p.get('pr') or {}).get('number')}
            h = (scst.get(k) or {}).get('hygiene') or {}
            if h.get('score') is not None: snap['hyg'][k] = h['score']
        for hh in ((res.get('global_resources') or {}).get('hosts') or []):
            root = next((d for d in hh.get('disks', []) if d['mount'] == '/'), None)
            snap['hosts'][hh['host']] = {'load': round(hh['load'][0] / max(1, hh.get('nproc') or 1) * 100, 1) if hh.get('load') else None, 'disk': root['pct'] if root else None}
        rows.append(snap); HIST_STATE['last'] = T
        try:
            if len(rows) % 30 == 0 or not os.path.exists(HIST_JSONL):
                rows[:] = downsample(rows, T)
                with open(HIST_JSONL + '.tmp', 'w') as f:
                    for r in rows: f.write(json.dumps(r, separators=(',', ':')) + '\n')
                os.replace(HIST_JSONL + '.tmp', HIST_JSONL)
            else:
                with open(HIST_JSONL, 'a') as f: f.write(json.dumps(snap, separators=(',', ':')) + '\n')
        except Exception: pass
    # compact series for the page (<= ~360 points)
    view = downsample(rows, T)
    step = max(1, len(view) // 360)
    res['history'] = {'rows': view[::step] + ([view[-1]] if view and (len(view) - 1) % step else []), 'file': HIST_JSONL,
                      'note': 'one snapshot per ~2 min; last 6h full, older downsampled to 15 min; 7-day retention'}
