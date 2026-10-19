#!/usr/bin/env python3
"""Token yield pipeline (read-only on repos; no fetch). Writes tokens.db + token_yield.json.
Sources: Codex rollouts ~/.codex/sessions/**/*.jsonl (token_count totals, turn_context model/effort/cwd);
Claude transcripts ~/.claude/projects/**/*.jsonl (message.usage, dedup by message.id). xbabe2 only for now.
Attribution: session cwd -> git toplevel -> project; commits in that repo authored in the session window
(or with an `Agent-Session: <id>` trailer = exact). Landing: commit is an ancestor of, or has a patch-id present on,
origin/main (local ref, not fetched) / the integration PR branch."""
import os, re, json, glob, time, sqlite3, subprocess, datetime, collections
HOME = os.environ.get('ADMIRALTY_TY_HOME', '/home/ubuntu')
BASE = os.environ.get('CDASH_RUNTIME', HOME + '/xbabe2_kicker/captains-dashboard')  # runtime data dir (status.json, history, tokens.db); code lives in admiralty/dashboard
DB = BASE + '/tokens.db'; OUT = BASE + '/token_yield.json'; STATUS = BASE + '/status.json'
SINCE_H = float(os.environ.get('TY_SINCE_H', '48'))
RATES = {'gpt-6-astra': (250, 25, 1250), 'gpt-5.6-sol': (100, 10, 500), 'gpt-5.6-terra': (50, 5, 300), 'gpt-6.1-sol': (50, 2.5, 250),
         'gpt-6-sol': (50, 5, 250), 'gpt-5.6-luna': (5, 0.5, 30), 'gpt-6-luna': (2.5, 0.25, 12.5)}
PROJ = [('/home/ubuntu/joil', 'joil'), ('/home/ubuntu/dope', 'dope'), ('/home/ubuntu/JopeDime', 'jopedime'), ('/home/ubuntu/jope-evidence', 'jopedime'),
        ('/home/ubuntu/jope', 'jopedime'), ('/home/ubuntu/jarambit', 'jarambit')]
REPO = {'joil': HOME + '/joil', 'dope': HOME + '/dope', 'jopedime': HOME + '/JopeDime', 'jarambit': HOME + '/jarambit'}
def iso(ts): return datetime.datetime.fromtimestamp(ts, datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')
def pts(s):
    try: return datetime.datetime.fromisoformat(s.replace('Z', '+00:00')).timestamp()
    except Exception: return None
def git(args, cwd, timeout=30):
    try:
        p = subprocess.run(['git', '-C', cwd] + args, capture_output=True, text=True, timeout=timeout)
        return p.returncode, p.stdout
    except Exception: return 1, ''
def tier(model, eff):
    m = (model or '').lower()
    if 'luna' in m: return 'luna'
    if 'astra' in m: return 'astra'
    if 'sol' in m or 'terra' in m: return 'sol'
    if 'opus' in m or 'sonnet' in m or 'claude' in m or 'fable' in m: return 'claude'
    return 'unknown'
def credits(model, inp, cached, out):
    r = RATES.get((model or '').lower())
    if not r: return None
    return (max(0, inp - cached) * r[0] + cached * r[1] + out * r[2]) / 1e6

db = sqlite3.connect(DB)
db.executescript('''CREATE TABLE IF NOT EXISTS files(path TEXT PRIMARY KEY, off INTEGER, size INTEGER);
CREATE TABLE IF NOT EXISTS sessions(id TEXT PRIMARY KEY, tool TEXT, model TEXT, effort TEXT, cwd TEXT, host TEXT, start REAL, end REAL,
  input INTEGER, cached INTEGER, output INTEGER, reasoning INTEGER, credits REAL, path TEXT);
CREATE TABLE IF NOT EXISTS claude_msgs(msg_id TEXT PRIMARY KEY, session TEXT, ts REAL, model TEXT, cwd TEXT, input INTEGER, cached INTEGER, output INTEGER);''')

def scan_codex(path):
    row = db.execute('SELECT off FROM files WHERE path=?', (path,)).fetchone(); off = row[0] if row else 0
    size = os.path.getsize(path)
    if size <= off: return
    sid = os.path.basename(path)[:-6]
    s = db.execute('SELECT model,effort,cwd,start,end,input,cached,output,reasoning FROM sessions WHERE id=?', (sid,)).fetchone()
    model, eff, cwd, start, end, inp, cac, out, rea = s if s else (None, None, None, None, None, 0, 0, 0, 0)
    with open(path, 'rb') as f:
        f.seek(off); data = f.read(size - off)
    cut = data.rfind(b'\n')
    if cut < 0: return
    for line in data[:cut].split(b'\n'):
        if b'"token_count"' not in line and b'"turn_context"' not in line and b'"session_meta"' not in line: continue
        try: d = json.loads(line)
        except Exception: continue
        ts = pts(d.get('timestamp', '')); pl = d.get('payload') or {}
        if ts: start = min(start or ts, ts); end = max(end or ts, ts)
        if d.get('type') == 'session_meta': cwd = pl.get('cwd') or cwd
        elif d.get('type') == 'turn_context':
            cwd = pl.get('cwd') or cwd; model = pl.get('model') or model
            eff = ((pl.get('collaboration_mode') or {}).get('settings') or {}).get('reasoning_effort') or eff
        else:
            tu = ((pl.get('info') or {}).get('total_token_usage')) or {}
            if tu: inp, cac, out, rea = tu.get('input_tokens', 0), tu.get('cached_input_tokens', 0), tu.get('output_tokens', 0), tu.get('reasoning_output_tokens', 0)
    db.execute('INSERT OR REPLACE INTO sessions VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)', (sid, 'codex', model, eff, cwd, 'xbabe2', start, end, inp, cac, out, rea, credits(model, inp, cac, out), path))
    db.execute('INSERT OR REPLACE INTO files VALUES(?,?,?)', (path, off + cut + 1, size))

def scan_claude(path):
    row = db.execute('SELECT off FROM files WHERE path=?', (path,)).fetchone(); off = row[0] if row else 0
    size = os.path.getsize(path)
    if size <= off: return
    with open(path, 'rb') as f:
        f.seek(off); data = f.read(size - off)
    cut = data.rfind(b'\n')
    if cut < 0: return
    for line in data[:cut].split(b'\n'):
        if b'"usage"' not in line: continue
        try: d = json.loads(line)
        except Exception: continue
        m = d.get('message') or {}; u = m.get('usage') or {}
        if not u or not m.get('id'): continue
        cached = u.get('cache_read_input_tokens', 0) + u.get('cache_creation_input_tokens', 0)
        db.execute('INSERT OR REPLACE INTO claude_msgs VALUES(?,?,?,?,?,?,?,?)', (m['id'], d.get('sessionId'), pts(d.get('timestamp', '')), m.get('model'), d.get('cwd'),
                   u.get('input_tokens', 0) + cached, cached, u.get('output_tokens', 0)))
    db.execute('INSERT OR REPLACE INTO files VALUES(?,?,?)', (path, off + cut + 1, size))

T = time.time(); cutoff = T - SINCE_H * 3600
for p in glob.glob(HOME + '/.codex/sessions/*/*/*/*.jsonl'):
    try:
        if os.path.getmtime(p) >= cutoff: scan_codex(p)
    except Exception: pass
for p in glob.glob(HOME + '/.claude/projects/*/*.jsonl'):
    try:
        if os.path.getmtime(p) >= cutoff: scan_claude(p)
    except Exception: pass
# fold claude messages into sessions
for sid, model, cwd, st, en, i, c, o in db.execute('SELECT session, max(model), max(cwd), min(ts), max(ts), sum(input), sum(cached), sum(output) FROM claude_msgs GROUP BY session').fetchall():
    db.execute('INSERT OR REPLACE INTO sessions VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)', ('claude-' + str(sid), 'claude', model, None, cwd, 'xbabe2', st, en, i, c, o, 0, None, None))
db.commit()

# ---- attribution ----
def project_of(cwd):
    for pre, k in PROJ:
        if cwd and (cwd == pre or cwd.startswith(pre + '/') or cwd.startswith(pre + '_') or cwd.startswith(pre + '-')): return k
    return None
TOP = {}
def toplevel(cwd):
    if cwd in TOP: return TOP[cwd]
    rc, o = git(['rev-parse', '--show-toplevel'], cwd, 10) if cwd and os.path.isdir(cwd) else (1, '')
    TOP[cwd] = o.strip() if rc == 0 else None; return TOP[cwd]
try: st = json.load(open(STATUS))
except Exception: st = {}
integ = {k: ((p.get('pr') or {}).get('branch')) for k, p in (st.get('projects') or {}).items()}
PATCH = {}
def patch_ids(repo, ref):
    key = (repo, ref)
    if key in PATCH: return PATCH[key]
    ids = set(); anc = set()
    rc, o = git(['log', '--format=%H', '--since=%d hours ago' % int(SINCE_H + 48), ref], repo, 60)
    if rc == 0: anc = set(o.split())
    try:
        p1 = subprocess.Popen(['git', '-C', repo, 'log', '-p', '--no-color', '--since=%d hours ago' % int(SINCE_H + 48), ref], stdout=subprocess.PIPE)
        p2 = subprocess.run(['git', '-C', repo, 'patch-id', '--stable'], stdin=p1.stdout, capture_output=True, text=True, timeout=120)
        ids = {l.split()[0] for l in p2.stdout.splitlines() if l.strip()}
    except Exception: pass
    PATCH[key] = (ids, anc); return PATCH[key]
def commit_pid(repo, sha):
    try:
        p1 = subprocess.Popen(['git', '-C', repo, 'show', '--no-color', sha], stdout=subprocess.PIPE)
        p2 = subprocess.run(['git', '-C', repo, 'patch-id', '--stable'], stdin=p1.stdout, capture_output=True, text=True, timeout=30)
        return p2.stdout.split()[0] if p2.stdout.strip() else None
    except Exception: return None
def resolve(repo, names):
    for n in names:
        if not n: continue
        rc, o = git(['rev-parse', '--verify', '-q', n], repo, 10)
        if rc == 0: return n
    return None

sessions = db.execute('SELECT id,tool,model,effort,cwd,start,end,input,cached,output,credits FROM sessions WHERE end >= ?', (cutoff,)).fetchall()
agg = collections.defaultdict(lambda: collections.defaultdict(float)); by_tier = collections.defaultdict(lambda: collections.defaultdict(float))
landed_lines = collections.defaultdict(int); notes = []
trailer_hits = 0
for sid, tool, model, eff, cwd, s0, s1, inp, cac, out, cr in sessions:
    k = project_of(cwd) or 'other'; tot = (inp or 0) + (out or 0); t = tier(model, eff)
    A = agg[k]; A['sessions'] += 1; A['input'] += inp or 0; A['cached'] += cac or 0; A['output'] += out or 0; A['tokens'] += tot; A['credits'] += cr or 0
    B = by_tier[t]; B['tokens'] += tot; B['credits'] += cr or 0; B['sessions'] += 1
    if k == 'other' or not cwd: continue
    top = toplevel(cwd)
    if not top: A['unattributable_tokens'] += tot; continue
    A['attributed_tokens'] += tot
    short = sid.split('-', 5)[-1] if tool == 'codex' else sid
    rc, o = git(['log', '--all', '--no-merges', '--format=%H%x09%at%x09%(trailers:key=Agent-Session,valueonly,separator=%x2C)',
                 '--since=@%d' % int((s0 or T) - 60), '--until=@%d' % int((s1 or T) + 300)], top, 30)
    commits = []
    for line in o.splitlines():
        parts = line.split('\t')
        if len(parts) < 2: continue
        tr = parts[2].strip() if len(parts) > 2 else ''
        if tr:
            if short in tr or sid in tr: commits.append(parts[0]); trailer_hits += 1
            continue
        commits.append(parts[0])
    # only commits made in THIS worktree's branch window: restrict to commits reachable from the worktree HEAD
    rc, head_list = git(['rev-list', '--since=@%d' % int((s0 or T) - 60), 'HEAD'], cwd if os.path.isdir(cwd) else top, 30)
    reach = set(head_list.split())
    commits = [c for c in commits if c in reach]
    if not commits: A['no_commit_tokens'] += tot; B['no_commit_tokens'] += tot; continue
    A['committed_tokens'] += tot; B['committed_tokens'] += tot; A['commits'] += len(commits)
    main = resolve(top, ['origin/main', 'main']); ib = integ.get(k); ibr = resolve(top, ['origin/' + ib, ib]) if ib else None
    mids = patch_ids(top, main) if main else (set(), set()); iids = patch_ids(top, ibr) if ibr else (set(), set())
    on_main = on_int = 0
    for c in commits:
        pid = None
        if c in mids[1]: on_main += 1
        else:
            pid = commit_pid(top, c)
            if pid and pid in mids[0]: on_main += 1
        if c in iids[1] or c in mids[1]: on_int += 1
        else:
            pid = pid or commit_pid(top, c)
            if pid and (pid in iids[0] or pid in mids[0]): on_int += 1
    if on_main:
        A['main_tokens'] += tot; B['main_tokens'] += tot; B['main_credits'] += cr or 0; A['main_credits'] += cr or 0; A['main_commits'] += on_main
    if on_int:
        A['integration_tokens'] += tot; B['integration_tokens'] += tot; B['integration_credits'] += cr or 0; A['integration_credits'] += cr or 0; A['integration_commits'] += on_int
    if not on_main and not on_int: A['unlanded_tokens'] += tot
done = {k: (p.get('done') or 0) for k, p in (st.get('projects') or {}).items()}
def fin(A, k=None):
    d = {x: (round(v) if x != 'credits' and 'credits' not in x else round(v, 1)) for x, v in A.items()}
    tot = A.get('tokens', 0)
    d['coverage_pct'] = round(100.0 * A.get('attributed_tokens', 0) / tot, 1) if tot else None
    d['yield_main_pct'] = round(100.0 * A.get('main_tokens', 0) / tot, 1) if tot else None
    d['yield_integration_pct'] = round(100.0 * A.get('integration_tokens', 0) / tot, 1) if tot else None
    d['wasted_pct_est'] = round(100.0 * A.get('no_commit_tokens', 0) / tot, 1) if tot else None
    cr = A.get('credits', 0)
    d['yield_main_credits_pct'] = round(100.0 * A.get('main_credits', 0) / cr, 1) if cr else None
    d['yield_integration_credits_pct'] = round(100.0 * A.get('integration_credits', 0) / cr, 1) if cr else None
    if k and done.get(k): d['credits_per_done_todo'] = round(A.get('credits', 0) / done[k], 1)
    return d
projects = {k: fin(A, k) for k, A in agg.items()}
total = collections.defaultdict(float)
for A in agg.values():
    for x, v in A.items(): total[x] += v
res = {'generated_utc': iso(T), 'window_hours': SINCE_H, 'projects': projects, 'total': fin(total), 'by_tier': {t: fin(B) for t, B in by_tier.items()},
       'integration_branches': integ, 'trailer_matched_commits': trailer_hits,
       'method': 'Tokens are real (Codex token_count totals; Claude message.usage, deduped). Session->project by cwd. A session "landed" if a commit reachable from its worktree HEAD, authored in its time window (or carrying an Agent-Session trailer), is an ancestor of or patch-id-equal to a commit on origin/main (local ref, not fetched) or the integration PR branch. Window attribution is an ESTIMATE (a session gets full credit for its tokens if any of its commits landed). Credits use published Codex rates; Claude credits not computed. xbabe2 sessions only; agy/Gemini usage not logged (unknown).',
       'hosts_covered': ['xbabe2'], 'not_covered': ['xbabe1', 'xbabe3 Codex/Claude sessions', 'agy/Gemini (no usage logs)']}
json.dump(res, open(OUT + '.tmp', 'w'), indent=1); os.replace(OUT + '.tmp', OUT)
print(json.dumps(res['total'])); print(json.dumps({k: (v.get('tokens'), v.get('yield_integration_pct'), v.get('yield_main_pct'), v.get('coverage_pct')) for k, v in projects.items()}))
