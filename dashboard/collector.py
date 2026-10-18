#!/usr/bin/env python3
"""Read-only live collector for the 4 captain dashboard.
Writes status.json every INTERVAL seconds. Never types into screens; only hardcopy.
Tolerates missing/changing files (fields become "unknown")."""
import json, os, re, subprocess, time, glob, hashlib, traceback, datetime, collections

HOME = '/home/ubuntu'
BASE = os.environ.get('CDASH_RUNTIME', '/home/ubuntu/xbabe2_kicker/captains-dashboard')  # runtime data dir (status.json, history, tokens.db); code lives in admiralty/dashboard
OUT = os.path.join(BASE, 'status.json')
HC = os.path.join(BASE, '.hc')
GHC = os.path.join(BASE, '.gh_cache.json')
HIST = os.path.join(BASE, '.history.json')
GITC = os.path.join(BASE, '.git_cache.json')
INTERVAL = int(os.environ.get('CDASH_INTERVAL', '45'))
GH = HOME + '/.local/bin/gh'
GH_TTL = 120
WORKER_HC_EVERY = 3
os.makedirs(HC, mode=0o700, exist_ok=True)

CAPTAINS = {'joil': '2708475.captain_joil', 'dope': '2716151.captain_dope',
            'jopedime': '2724936.captain_jopedime', 'jarambit': '2740538.captain_jarambit'}
NAMES = {'joil': 'Joil', 'dope': 'DOPE', 'jopedime': 'JopeDime', 'jarambit': 'Jarambit'}

def now_iso():
    return datetime.datetime.now(datetime.timezone.utc).isoformat(timespec='seconds')

def mtime_iso(p):
    try:
        return datetime.datetime.fromtimestamp(os.path.getmtime(p), datetime.timezone.utc).isoformat(timespec='seconds')
    except Exception:
        return None

def rj(p):
    try:
        with open(p) as f:
            return json.load(f)
    except Exception:
        return None

def wj(p, d):
    tmp = p + '.tmp'
    with open(tmp, 'w') as f:
        json.dump(d, f, indent=1, default=str)
    os.replace(tmp, p)

def run(args, timeout=20, cwd=None):
    try:
        r = subprocess.run(args, capture_output=True, text=True, timeout=timeout, cwd=cwd)
        return r.returncode, r.stdout, r.stderr
    except Exception as e:
        return -1, '', str(e)

# ---------------- text hygiene ----------------
ANSI = re.compile(r'\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b[\]P^_].*?(\x07|\x1b\\)|\x1b[@-Z\\-_]')
SECRET_PATTERNS = [
    re.compile(r'gh[pousr]_[A-Za-z0-9]{20,}'), re.compile(r'github_pat_[A-Za-z0-9_]{20,}'),
    re.compile(r'sk-[A-Za-z0-9_-]{16,}'), re.compile(r'xox[abprs]-[A-Za-z0-9-]{10,}'),
    re.compile(r'AKIA[0-9A-Z]{16}'), re.compile(r'AIza[0-9A-Za-z_-]{30,}'),
    re.compile(r'(?i)(bearer|authorization:)\s+\S+'),
    re.compile(r'(?i)\b(password|passwd|secret|token|api[_-]?key)\s*[=:]\s*\S+'),
    re.compile(r'-----BEGIN [A-Z ]*PRIVATE KEY-----'),
]

def clean(s, private_guard=False):
    s = ANSI.sub('', s).replace('\x00', '').replace('\r', '')
    s = ''.join(ch for ch in s if (ch == '\n' or ch == '\t' or ord(ch) >= 32) and ch != '\ufffd')
    out = []
    for line in s.split('\n'):
        for pat in SECRET_PATTERNS:
            line = pat.sub('[redacted]', line)
        if private_guard and ('.private' in line or 'CONTROL' in line):
            line = '[line withheld: references private Joil CONTROL]'
        out.append(line.rstrip())
    return '\n'.join(out)

def tail_lines(p, n=5, maxlen=420, private_guard=False):
    try:
        with open(p, 'rb') as f:
            f.seek(0, 2); size = f.tell(); f.seek(max(0, size - 96000))
            data = f.read().decode('utf-8', 'replace')
    except Exception:
        return None
    lines = [l for l in clean(data, private_guard).split('\n') if l.strip()]
    return [l[:maxlen] + ('…' if len(l) > maxlen else '') for l in lines[-n:]]

def needs_jepson(p, n=3):
    try:
        with open(p, 'rb') as f:
            f.seek(0, 2); size = f.tell(); f.seek(max(0, size - 200000))
            data = f.read().decode('utf-8', 'replace')
    except Exception:
        return []
    lines = clean(data).split('\n')[-80:]
    hits = []
    for l in lines:
        m = re.search(r'NEEDS JEPSON\**`?\s*:\**`?\s*(.+)', l)
        if not m: continue
        if m.start() > 0 and l[m.start() - 1] == '`': continue  # quoted example, not an actual ask
        if re.search(r'\b[Nn]o `?NEEDS JEPSON', l): continue
        txt = m.group(1).strip(' `*')
        if re.match(r'(?i)(none|n/?a|nothing|—|-)\b', txt): continue
        hits.append(txt[:260])
    out = []
    for h in hits:
        if h not in out: out.append(h)
    return out[-n:]

# ---------------- screens ----------------
def screen_list():
    rc, out, _ = run(['screen', '-ls'], timeout=10)
    res = {}
    for m in re.finditer(r'^\s+(\d+)\.(\S+)\s+\(([^)]*)\)\s+\((\w+)\)', out, re.M):
        res[m.group(1) + '.' + m.group(2)] = {'pid': int(m.group(1)), 'name': m.group(2), 'started': m.group(3), 'state': m.group(4)}
    return res

def descendants(pid, depth=5):
    out, frontier = [], [pid]
    for _ in range(depth):
        nxt = []
        for p in frontier:
            try:
                with open(f'/proc/{p}/task/{p}/children') as f:
                    kids = [int(x) for x in f.read().split()]
            except Exception:
                kids = []
            nxt += kids
        out += nxt; frontier = nxt
        if not frontier: break
    return out

def cmdline(pid):
    try:
        with open(f'/proc/{pid}/cmdline', 'rb') as f:
            return [a.decode('utf-8', 'replace') for a in f.read().split(b'\0') if a]
    except Exception:
        return []

def agent_proc(spid):
    for p in descendants(spid):
        a = cmdline(p)
        if not a: continue
        j = ' '.join(a[:3])
        kind = None
        if re.search(r'(^|/)agy(\s|$)', j): kind = 'agy'
        elif re.search(r'(^|/)codex(\s|$)', j) and 'resume-stage' not in j: kind = 'codex'
        elif re.search(r'(^|/)claude(\s|$)', j): kind = 'claude'
        elif re.search(r'(^|/)gemini(\s|$)', j): kind = 'gemini'
        if kind:
            model = None; eff = None
            for i, x in enumerate(a):
                if x in ('-m', '--model') and i + 1 < len(a): model = a[i + 1]
                if x == '-c' and i + 1 < len(a) and a[i + 1].startswith('model_reasoning_effort='):
                    eff = a[i + 1].split('=', 1)[1]
            prompt = a[-1] if len(a) > 1 and len(a[-1]) > 30 else ''
            return {'kind': kind, 'pid': p, 'model': model, 'effort': eff, 'prompt': prompt}
    # supervisor-style (python resume-stage scripts)
    for p in descendants(spid):
        a = cmdline(p)
        if a and any('resume-stage' in x or 'supervisor' in x for x in a):
            return {'kind': 'supervised', 'pid': p, 'model': None, 'effort': None, 'prompt': ''}
    return None

def hardcopy(sid):
    path = os.path.join(HC, sid.replace('/', '_') + '.txt')
    run(['screen', '-S', sid, '-X', 'hardcopy', path], timeout=8)
    time.sleep(0.15)
    try:
        with open(path, 'rb') as f:
            return clean(f.read().decode('utf-8', 'replace'))
    except Exception:
        return None

BOX = re.compile(r'^[\s\-─━═│┃╭╮╰╯┌┐└┘├┤┬┴┼▔▁▏▕█▌▐░▒▓>›»]*$')
NOISE = re.compile(r'(esc to cancel|ctrl\+o to expand|Tip: |shift\+tab|\[ SCREEN:|^\s*\? for shortcuts|context left|Running command\.\.\.$)', re.I)
MODEL_RE = re.compile(r'(Gemini [0-9.]+ [A-Za-z]+(?: [·\-] ?\w+)?|gpt-[\w.\-]+(?: (?:xhigh|high|medium|low))?|claude-[\w.\-]+|Opus [0-9.]+|Sonnet [0-9.]+|Fable [0-9.]+)')

def meaningful(text, n=8, private_guard=False):
    if not text: return []
    lines = []
    for l in text.split('\n'):
        if private_guard and ('.private' in l or 'CONTROL' in l):
            l = '[line withheld: references private Joil CONTROL]'
        if not l.strip() or BOX.match(l) or NOISE.search(l): continue
        lines.append(l.strip()[:240])
    dedup = []
    for l in lines:
        if not dedup or dedup[-1] != l: dedup.append(l)
    return dedup[-n:]

STATE = {'hc_hash': {}, 'hc_changed': {}, 'worker_info': {}, 'cycle': 0}

def screen_info(sid, scr, do_hc, project, task_hint=None, private_guard=False):
    info = {'id': sid, 'name': scr['name'] if scr else sid.split('.', 1)[-1], 'alive': bool(scr)}
    if not scr:
        return info
    ap = agent_proc(scr['pid'])
    info['agent'] = ap['kind'] if ap else None
    info['agent_running'] = bool(ap)
    model = None
    if ap and ap.get('model'):
        model = ap['model'] + (' ' + ap['effort'] if ap.get('effort') else '')
    text = None
    if do_hc:
        text = hardcopy(sid)
        if text is not None:
            h = hashlib.sha1(text.encode()).hexdigest()
            if STATE['hc_hash'].get(sid) != h:
                STATE['hc_hash'][sid] = h; STATE['hc_changed'][sid] = now_iso()
            if not model:
                ms = MODEL_RE.findall(text[-1500:]) or MODEL_RE.findall(text)
                if ms: model = ms[-1]
            prev = STATE['worker_info'].setdefault(sid, {})
            prev['tail'] = meaningful(text, 8, private_guard)
            if model: prev['model'] = model
            tid = None
            ids = TASK_RE.get(project)
            if ids:
                found = ids.findall(text)
                if found: tid = found[-1] if isinstance(found[-1], str) else found[-1][0]
            prev['mentioned_task'] = tid
    prev = STATE['worker_info'].get(sid, {})
    info['model'] = model or prev.get('model') or (ap['kind'] if ap else None)
    info['screen_changed_utc'] = STATE['hc_changed'].get(sid)
    if task_hint:
        info['task'] = task_hint; info['task_source'] = 'ledger'
    else:
        pt = None
        if ap and ap.get('prompt') and TASK_RE.get(project):
            m = TASK_RE[project].findall(ap['prompt'])
            if m: pt = m[0] if isinstance(m[0], str) else m[0][0]
        if pt:
            info['task'] = pt; info['task_source'] = 'launch prompt'
        elif prev.get('mentioned_task'):
            info['task'] = prev['mentioned_task']; info['task_source'] = 'last ID mentioned on screen'
    info['tail'] = prev.get('tail', [])
    return info

TASK_RE = {
    'joil': re.compile(r'\b(V5-\d\d(?:-\d\d)?)\b'),
    'dope': re.compile(r'\b([PICDSUEMFRQX]\d\d)\b'),
    'jopedime': re.compile(r'\b(I\d\d)\b'),
    'jarambit': re.compile(r'\b((?:WP|AUTO|G16|RSV2|CERT|DOC|REN|RDM|PAPER|CI)-\d\d)\b'),
}

def project_of(name):
    n = name.lower()
    if n.startswith('captain_'): return n.split('_', 1)[1]
    if n.startswith('joil'): return 'joil'
    if n.startswith('dope'): return 'dope'
    if n.startswith('jope'): return 'jopedime'
    if 'jarambit' in n or n.startswith('jr_'): return 'jarambit'
    return None

# ---------------- github ----------------
def gh_json(args):
    rc, out, err = run([GH] + args, timeout=40)
    if rc != 0:
        return None, (err or out).strip()[:200]
    try:
        return json.loads(out), None
    except Exception as e:
        return None, str(e)

def summarize_checks(roll):
    c = collections.Counter()
    failing = []
    for x in roll or []:
        st = (x.get('status') or x.get('state') or '').upper()
        concl = (x.get('conclusion') or '').upper()
        if x.get('__typename') == 'StatusContext':
            s = (x.get('state') or '').upper()
            k = 'success' if s == 'SUCCESS' else 'pending' if s in ('PENDING', 'EXPECTED') else 'failure'
        elif st != 'COMPLETED':
            k = 'pending'
        elif concl in ('SUCCESS', 'NEUTRAL', 'SKIPPED'):
            k = 'success'
        else:
            k = 'failure'
        c[k] += 1
        if k == 'failure': failing.append(x.get('name') or x.get('context'))
    state = 'none' if not c else 'failure' if c['failure'] else 'pending' if c['pending'] else 'success'
    return {'state': state, 'counts': dict(c), 'failing': failing[:8]}

PR_FIELDS = 'number,title,state,isDraft,headRefName,headRefOid,reviewDecision,statusCheckRollup,reviews,mergedAt,updatedAt,url'

def pr_lookup(repo, number=None, prefer_branches=()):
    cache = rj(GHC) or {}
    key = f'{repo}#{number or "auto"}'
    ent = cache.get(key)
    if ent and time.time() - ent.get('fetched', 0) < GH_TTL:
        return ent['data']
    data = {'repo': repo}
    num = number
    if not num:
        lst, err = gh_json(['pr', 'list', '--repo', repo, '--state', 'all', '--limit', '8', '--json', 'number,headRefName,state,mergedAt,title'])
        if lst is None:
            data['error'] = err
        else:
            openp = [p for p in lst if p['state'] == 'OPEN']
            pick = [p for p in openp if p['headRefName'] in prefer_branches] or openp
            if pick:
                num = pick[0]['number']
            else:
                data['note'] = 'no open PR'
                merged = [p for p in lst if p['state'] == 'MERGED']
                if merged:
                    num = merged[0]['number']
    if num:
        pr, err = gh_json(['pr', 'view', str(num), '--repo', repo, '--json', PR_FIELDS])
        if pr is None:
            data['error'] = err
        else:
            reviews = pr.get('reviews') or []
            head = pr.get('headRefOid')
            exact = [r for r in reviews if (r.get('commit') or {}).get('oid') == head]
            last = reviews[-1] if reviews else None
            data.update({
                'number': pr['number'], 'title': pr.get('title'), 'url': pr.get('url'), 'state': pr.get('state'),
                'draft': pr.get('isDraft'), 'head': head, 'branch': pr.get('headRefName'),
                'merged_at': pr.get('mergedAt'), 'updated_at': pr.get('updatedAt'),
                'review_decision': pr.get('reviewDecision') or None,
                'last_review': {'author': (last.get('author') or {}).get('login'), 'state': last.get('state'),
                                'commit': ((last.get('commit') or {}).get('oid') or '')[:12], 'at': last.get('submittedAt')} if last else None,
                'exact_head_approved': any(r.get('state') == 'APPROVED' for r in exact),
                'ci': summarize_checks(pr.get('statusCheckRollup')),
            })
    cache[key] = {'fetched': time.time(), 'data': data}
    data['fetched_utc'] = now_iso()
    wj(GHC, cache)
    return data

# ---------------- change tracking ----------------
HISTORY = rj(HIST) or {}

def track(project, statuses, src):
    """statuses: {task: status}. Records collector-observed transitions."""
    h = HISTORY.setdefault(project, {'last': {}, 'changes': []})
    last = h['last']
    t = now_iso()
    if last:
        for k, v in statuses.items():
            if k in last and last[k] != v:
                h['changes'].append({'task': k, 'old': last[k], 'new': v, 'at': t, 'source': 'observed by dashboard (' + src + ')'})
            elif k not in last:
                h['changes'].append({'task': k, 'old': None, 'new': v, 'at': t, 'source': 'new task observed (' + src + ')'})
    h['last'] = dict(statuses)
    h['changes'] = h['changes'][-60:]
    return list(h['changes'])

def merge_recent(*lists, n=8):
    allc = [c for l in lists for c in (l or []) if c.get('at')]
    seen, out = set(), []
    for c in sorted(allc, key=lambda c: c['at'], reverse=True):
        k = (c['task'], c.get('new'), c['at'][:16])
        if k in seen: continue
        if c.get('old') == c.get('new'):  # note-only update: keep just the newest per task
            nk = ('note', c['task'])
            if nk in seen: continue
            seen.add(nk)
        seen.add(k); out.append(c)
    return out[:n]

def bucket(s):
    s = (s or '').lower()
    if s in ('done', 'verified', 'accepted', 'complete', 'completed'): return 'done'
    if s in ('in_progress', 'review', 'in_review', 'dispatched', 'active', 'assigned'): return 'in_progress'
    if s.startswith('blocked') or s in ('audit_held', 'failed', 'fail'): return 'blocked'
    return 'open'

# ---------------- Joil ----------------
JOIL = HOME + '/joil'
JCTL = JOIL + '/.private/v5-workers'
JWT = JOIL + '/.private/worktrees/v5-engineering-plan'
JPLAN = 'docs/v5-plan'

def joil_git_changes():
    cache = rj(GITC) or {}
    rc, out, _ = run(['git', '-C', JWT, 'log', '-n', '10', '--format=%H %cI', '--', JPLAN + '/progress'], timeout=15)
    changes = []
    for line in out.split('\n'):
        if not line.strip(): continue
        sha, at = line.split()
        if sha not in cache:
            rc2, names, _ = run(['git', '-C', JWT, 'diff', '--name-only', sha + '^', sha, '--', JPLAN + '/progress'], timeout=15)
            ch = []
            for nm in names.split():
                def load(ref):
                    r, o, _ = run(['git', '-C', JWT, 'show', f'{ref}:{nm}'], timeout=15)
                    try: return json.loads(o)['tasks'] if r == 0 else {}
                    except Exception: return {}
                a, b = load(sha + '^'), load(sha)
                for k, v in b.items():
                    ov = (a.get(k) or {}).get('status')
                    if ov != v.get('status'):
                        ch.append({'task': k, 'old': ov, 'new': v.get('status')})
            cache[sha] = ch
        for c in cache[sha]:
            changes.append(dict(c, at=datetime.datetime.fromisoformat(at).astimezone(datetime.timezone.utc).isoformat(timespec='seconds'),
                                source='git commit ' + sha[:8]))
    wj(GITC, cache)
    return changes

def joil_validator():
    p = JCTL + '/gorkbot/validator/LATEST.md'
    try:
        txt = open(p).read()
    except Exception:
        return {'status': 'unknown'}
    sec = txt.split('## Previous')[0]
    m = re.search(r'audit (\d+)', sec)
    obs = re.search(r'Observed:\s*(\S+Z)', sec)
    canon = re.search(r'(\d+) done / (\d+) review / (\d+) todo', sec)
    held = sorted(set(x for x in re.findall(r'(V5-\d\d-\d\d)[^\n]{0,60}AUDIT-HELD', sec)))
    return {'audit': m.group(1) if m else None, 'observed_utc': obs.group(1).replace('Z', '+00:00') if obs else None,
            'verified_mentions': len(re.findall(r'\bVERIFIED\b', sec)), 'weak_mentions': len(re.findall(r'\bWEAK\b', sec)),
            # count real FAIL verdicts only: not "no FAIL" and not expected red-control logs ("FAILED with exit 101")
            'fail_mentions': len(re.findall(r'(?<!no )\bFAIL\b(?! *[,/]? *(?:no |0\b))', re.sub(r'FAILED with exit \d+', '', sec))),
            'canonical': {'done': int(canon.group(1)), 'review': int(canon.group(2)), 'todo': int(canon.group(3))} if canon else None,
            'audit_held': held, 'file_mtime_utc': mtime_iso(p)}

def joil_main_statuses():
    rc, out, _ = run(['git', '-C', JOIL, 'ls-tree', '--name-only', 'origin/main', JPLAN + '/progress/'], timeout=15)
    res = {}
    if rc != 0: return res
    for f in out.split():
        rc2, js, _ = run(['git', '-C', JOIL, 'show', 'origin/main:' + f], timeout=15)
        try: res.update({k: v.get('status') for k, v in json.loads(js)['tasks'].items()})
        except Exception: pass
    return res

def collect_joil(screens, do_hc):
    r = {'name': 'Joil', 'ledger_source': 'integration worktree docs/v5-plan/progress/*.json (via v5_plan.py format)'}
    try:
        tasks = rj(os.path.join(JWT, JPLAN, 'TASKS.json'))
        statuses, packets = {}, []
        for p in tasks['packets']:
            st = rj(os.path.join(JWT, JPLAN, 'progress', p['id'] + '.json')) or {'tasks': {}}
            ps = {t['id']: (st['tasks'].get(t['id']) or {}).get('status', 'unknown') for t in p['tasks']}
            statuses.update(ps)
            packets.append({'id': p['id'], 'done': sum(v == 'done' for v in ps.values()), 'total': len(ps)})
        val = joil_validator()
        r['validator'] = val
        held = set(val.get('audit_held') or [])
        eff = {k: ('blocked' if k in held and v != 'done' else v) for k, v in statuses.items()}
        c = collections.Counter(bucket(v) for v in eff.values())
        r['total'] = len(statuses); r['done'] = c['done']
        r['counts'] = {'done': c['done'], 'in_progress': c['in_progress'], 'blocked': c['blocked'], 'open': c['open']}
        r['raw_status_counts'] = dict(collections.Counter(statuses.values()))
        r['packets_accepted'] = sum(1 for p in packets if p['total'] and p['done'] == p['total'])
        r['packets_total'] = len(packets)
        r['_statuses'] = eff
        rc, h, _ = run(['git', '-C', JWT, 'log', '-1', '--format=%h %cI'], timeout=10)
        r['ledger_head'] = h.strip()
        r['recent'] = merge_recent(joil_git_changes(), track('joil', statuses, 'progress json'))
        r['blocked_ids'] = sorted(held)
    except Exception as e:
        r['error'] = 'ledger unreadable: ' + str(e)[:160]
    lease = rj(JCTL + '/lease.json')
    r['lease'] = {'expires_utc': lease.get('expires_utc'), 'renewals': len(lease.get('renewals') or [])} if lease else 'unknown'
    r['pr'] = pr_lookup('neverhuman/joil', 6)
    r['coord_log'] = {'path': JOIL + '/COORD.md', 'tail': tail_lines(JOIL + '/COORD.md', 5, private_guard=True),
                      'mtime_utc': mtime_iso(JOIL + '/COORD.md')}
    r['needs_jepson'] = needs_jepson(JOIL + '/COORD.md')
    hints = {}
    for role in ('codex', 'claude'):
        s = rj(f'{JCTL}/status/{role}-supervisor.json') or {}
        if s.get('screen'): hints[s['screen']] = f"stage {s.get('stage')} ({s.get('state')})"
    r['_hints'] = hints
    return r

# ---------------- DOPE ----------------
DOPE_D = HOME + '/dope/target/unified-spec-workplan-20261017'
DOPE_P = DOPE_D + '/packet-v2-9e11fd166f5f'

def collect_dope(screens, do_hc):
    r = {'name': 'DOPE', 'ledger_source': 'packet-v2 tasks.json + progress.json + events.jsonl'}
    try:
        t = rj(DOPE_P + '/tasks.json'); pr_ = rj(DOPE_P + '/progress.json') or {}
        statuses = {x['id']: x.get('status', 'unknown') for x in t['tasks']}
        c = collections.Counter(bucket(v) for v in statuses.values())
        r['total'] = len(statuses); r['done'] = c['done']
        r['counts'] = {k: c[k] for k in ('done', 'in_progress', 'blocked', 'open')}
        r['raw_status_counts'] = dict(collections.Counter(statuses.values()))
        kinds = collections.Counter((x.get('kind'), bucket(x.get('status'))) for x in t['tasks'])
        r['by_kind'] = {'planning_done': kinds[('planning', 'done')], 'planning_total': sum(v for (k, _), v in kinds.items() if k == 'planning'),
                        'impl_done': sum(v for (k, b), v in kinds.items() if k != 'planning' and b == 'done'),
                        'impl_total': sum(v for (k, _), v in kinds.items() if k != 'planning')}
        r['_statuses'] = statuses
        r['ledger_revision'] = pr_.get('ledger_revision') or t.get('revision')
        r['ledger_updated_utc'] = pr_.get('updated_at') or t.get('updated_at')
        ev = []
        try:
            for line in open(DOPE_P + '/events.jsonl'):
                try:
                    e = json.loads(line)
                except Exception:
                    continue
                ev.append({'task': e.get('task'), 'old': e.get('old_status'), 'new': e.get('status'), 'at': e.get('at'),
                           'source': 'events.jsonl ' + str(e.get('actor') or ''), 'note': clean(str(e.get('note') or ''))[:200]})
        except Exception:
            pass
        r['recent'] = merge_recent(ev, track('dope', statuses, 'tasks.json'))
        r['blockers'] = [clean(str(b))[:260] for b in (pr_.get('blockers') or [])]
        r['leases'] = [{'owner': l.get('owner'), 'tasks': l.get('tasks'), 'lease': l.get('lease_id')} for l in (pr_.get('active_leases') or [])]
        r['current_focus'] = clean(str(pr_.get('current_focus') or ''))[:400]
        r['blocked_ids'] = [k for k, v in statuses.items() if bucket(v) == 'blocked']
    except Exception as e:
        r['error'] = 'ledger unreadable: ' + str(e)[:160]
    r['pr'] = pr_lookup('neverhuman/dope', 269)
    r['coord_log'] = {'path': HOME + '/dope/COMMS.md', 'tail': tail_lines(HOME + '/dope/COMMS.md', 5), 'mtime_utc': mtime_iso(HOME + '/dope/COMMS.md')}
    r['needs_jepson'] = needs_jepson(HOME + '/dope/COMMS.md')
    hints = {}
    for l in r.get('leases', []):
        if l.get('owner'): hints[l['owner']] = ', '.join(l.get('tasks') or [])
    r['_hints'] = hints
    return r

# ---------------- JopeDime ----------------
JE = HOME + '/jope-evidence/spec-integration-20261017'
JP = JE + '/packet-v4-921643bc3ae0'

def jope_receipts(statuses):
    repo = HOME + '/JopeDime'
    rc, main, _ = run(['git', '-C', repo, 'rev-parse', '--verify', '-q', 'origin/main^{commit}'], timeout=10)
    main = main.strip() if rc == 0 else ''
    ok, relabeled = [], []
    if not main: return ok, relabeled
    for f in sorted(glob.glob(JE + '/verify/I[0-9][0-9].json')):
        tid = os.path.basename(f)[:-5]
        r = rj(f) or {}
        sha, csha = str(r.get('sha') or ''), str(r.get('checkout_sha') or '')
        if tid not in statuses or str(r.get('status', '')).upper() != 'PASS' or not re.fullmatch(r'[0-9a-f]{40}', sha) or (csha and csha != sha):
            continue
        if not (sha == main or run(['git', '-C', repo, 'merge-base', '--is-ancestor', sha, main], timeout=15)[0] == 0):
            continue
        rc, ct, _ = run(['git', '-C', repo, 'log', '-1', '--format=%ct', sha], timeout=10)
        try:
            va = datetime.datetime.fromisoformat(str(r.get('verified_at_utc') or r.get('verified_at') or '').replace('Z', '+00:00')).timestamp()
            if rc == 0 and va < int(ct.strip()):
                relabeled.append(tid); continue
        except ValueError:
            pass
        ok.append(tid)
    return ok, relabeled

def collect_jopedime(screens, do_hc):
    r = {'name': 'JopeDime', 'ledger_source': 'packet-v4 tasks.json (66 ids, immutable, all PROPOSED) overlaid with LIVE_STATUS.json verified/active + captain-state ASSIGNMENT_*.md'}
    try:
        t = rj(JP + '/tasks.json'); live = rj(JE + '/LIVE_STATUS.json') or {}
        statuses = {x['id']: 'open' for x in t['tasks']}
        titles = {x['id']: x.get('title') for x in t['tasks']}
        verified = [v.get('task_id') for v in live.get('verified_tasks') or []]
        # also count per-task verify receipts (verify/Ixx.json) that are a genuine PASS on a commit contained in main
        # and were verified after that commit existed (receipts relabeled to a newer sha are NOT counted)
        rec_ok, rec_relabeled = jope_receipts(statuses)
        verified = sorted(set(verified) | set(rec_ok))
        r['receipts'] = {'genuine_on_main': len(rec_ok), 'relabeled_not_rerun': sorted(rec_relabeled)}
        for v in verified:
            if v in statuses: statuses[v] = 'done'
        at = live.get('active_task') or {}
        active = at.get('task_id')
        if active in statuses and statuses[active] != 'done': statuses[active] = 'in_progress'
        assigned = []
        alive_names = {v['name'] for v in (screens or {}).values()}
        queued = []
        for f in glob.glob(JE + '/captain-state/ASSIGNMENT_*.md'):
            tid = os.path.basename(f)[11:-3]
            if tid in statuses and statuses[tid] == 'open':
                try:
                    m = re.search(r'\*\*Screen\*\*:\s*`?(?:\d+\.)?([\w\-]+)', open(f, errors='replace').read(4000))
                except Exception:
                    m = None
                if m and m.group(1) not in alive_names:
                    queued.append(tid); continue   # assignment written but its screen is not running yet
                statuses[tid] = 'assigned'; assigned.append(tid)
        r['queued_assignments_no_live_screen'] = sorted(queued)
        c = collections.Counter(bucket(v) for v in statuses.values())
        r['total'] = len(statuses); r['done'] = c['done']
        r['counts'] = {k: c[k] for k in ('done', 'in_progress', 'blocked', 'open')}
        r['raw_status_counts'] = dict(collections.Counter(statuses.values()))
        r['live_status'] = {'updated_utc': live.get('updated_at'), 'programme_progress': live.get('programme_progress'),
                            'active_task': {'task': active, 'owner': at.get('owner'), 'branch': at.get('branch'),
                                            'head': (at.get('head_sha') or '')[:12], 'lease_expires_utc': at.get('lease_expires_at')} if active else None}
        r['assigned_from_captain_state'] = sorted(assigned)
        r['_statuses'] = statuses
        pp = live.get('programme_progress') or {}
        rm = live.get('receiving_main') or {}
        main_green = str(rm.get('main_guard_conclusion') or '').startswith('success') and str(rm.get('full_main_cpu_conclusion') or '').startswith('success')
        r['_stage_sets'] = {'done': set(verified), 'on_main': (set(pp.get('verified_task_ids') or []) | set(rec_relabeled)) - set(verified),
                            'lane_complete': set(pp.get('lane_complete_task_ids') or []), 'lane_complete_branch': pp.get('lane_complete_branch'),
                            'in_progress': set(pp.get('in_progress_code_task_ids') or []),
                            'main': {'sha': (rm.get('sha') or '')[:12], 'ci_green': main_green}}
        seed = []
        if verified:
            seed = [{'task': v, 'old': None, 'new': 'done (verified)', 'at': (live.get('updated_at') or '').replace('Z', '+00:00'),
                     'source': 'LIVE_STATUS.json updated_at'} for v in verified]
        for tid in assigned:
            seed.append({'task': tid, 'old': 'open', 'new': 'assigned', 'at': mtime_iso(JE + f'/captain-state/ASSIGNMENT_{tid}.md'), 'source': 'ASSIGNMENT file mtime'})
        if active:
            seed.append({'task': active, 'old': None, 'new': 'in_progress', 'at': (live.get('updated_at') or '').replace('Z', '+00:00'), 'source': 'LIVE_STATUS active_task'})
        r['recent'] = merge_recent(seed, track('jopedime', statuses, 'LIVE_STATUS/assignments'))
        r['blocked_ids'] = []
        r['_hints'] = {at.get('owner'): active} if active and at.get('owner') else {}
        r['titles'] = {k: titles[k] for k in list(verified) + ([active] if active else []) + assigned if k in titles}
    except Exception as e:
        r['error'] = 'ledger unreadable: ' + str(e)[:160]; r['_hints'] = {}
    r['pr'] = pr_lookup('neverhuman/JopeDime', None, ('feat/selfplay-batch4-integration-20261018', 'feat/selfplay-spec-integration-20261018'))
    r['coord_log'] = {'path': HOME + '/JopeDime/COMMS.md', 'tail': tail_lines(HOME + '/JopeDime/COMMS.md', 5), 'mtime_utc': mtime_iso(HOME + '/JopeDime/COMMS.md')}
    r['needs_jepson'] = needs_jepson(HOME + '/JopeDime/COMMS.md')
    return r

# ---------------- Jarambit ----------------
JA = HOME + '/jarambit'

def find_workplan():
    cands = [JA + '/docs/WORKPLAN.md']
    rc, out, _ = run(['git', '-C', JA, 'worktree', 'list', '--porcelain'], timeout=10)
    for l in out.split('\n'):
        if l.startswith('worktree '):
            cands.append(l[9:].strip() + '/docs/WORKPLAN.md')
    ex = [c for c in cands if os.path.isfile(c)]
    if not ex: return None
    return max(ex, key=os.path.getmtime)

def parse_md_tasks(path):
    tasks = {}
    header = None
    for line in open(path, encoding='utf-8', errors='replace'):
        if not line.startswith('|'):
            header = None; continue
        cells = [c.strip() for c in line.strip().strip('|').split('|')]
        if header is None:
            header = [c.lower() for c in cells]; continue
        if all(set(c) <= set('-: ') for c in cells): continue
        if 'task id' not in header or 'status' not in header: continue
        try:
            tid = cells[header.index('task id')].strip('` *')
            si = header.index('status')
            if len(cells) != len(header):  # unescaped '|' inside a cell (e.g. LaTeX |x|): count status from the right
                si = len(cells) - (len(header) - si)
            st = cells[si]
        except IndexError:
            continue
        m = re.search(r'`?([A-Z_]+)`?', st)
        if not re.match(r'^[A-Z0-9]+(-[A-Z0-9]+)+$', tid): continue
        title = cells[1].strip('` ') if len(cells) > 1 else ''
        tasks[tid] = {'status': (m.group(1) if m else st)[:30], 'status_text': clean(st)[:120], 'title': title[:90]}
    return tasks

def collect_jarambit(screens, do_hc):
    r = {'name': 'Jarambit'}
    wp = find_workplan()
    if not wp:
        r['error'] = 'docs/WORKPLAN.md not found on main or any worktree'; r['total'] = None
    else:
        on_main = wp == JA + '/docs/WORKPLAN.md'
        r['ledger_source'] = wp + ('' if on_main else ' (NOT yet on main; captain draft in worktree)')
        r['ledger_mtime_utc'] = mtime_iso(wp)
        try:
            tasks = parse_md_tasks(wp)
            statuses = {k: v['status'] for k, v in tasks.items()}
            c = collections.Counter(bucket(v) for v in statuses.values())
            r['total'] = len(statuses); r['done'] = c['done']
            r['counts'] = {k: c[k] for k in ('done', 'in_progress', 'blocked', 'open')}
            r['raw_status_counts'] = dict(collections.Counter(statuses.values()))
            r['blocked_ids'] = [k for k, v in statuses.items() if bucket(v) == 'blocked']
            r['_statuses'] = statuses
            r['blocked_detail'] = {k: tasks[k]['status_text'] for k in r['blocked_ids']}
            r['recent'] = merge_recent(track('jarambit', statuses, 'WORKPLAN.md table'))
            r['titles'] = {k: v['title'] for k, v in tasks.items() if bucket(v['status']) != 'open'}
            txt = open(wp, encoding='utf-8', errors='replace').read()
            m = re.search(r'Currently Done[^\n]*?`(\d+) / (\d+)`', txt)
            r['declared_counter'] = {'done': int(m.group(1)), 'total': int(m.group(2)), 'note': "WORKPLAN.md section 4 self-reported counter (rollups CERT/DOC counted once each)"} if m else None
        except Exception as e:
            r['error'] = 'WORKPLAN parse failed: ' + str(e)[:160]
    # waves
    waves = []
    for d in sorted(glob.glob(JA + '/out/w[0-9]*'), key=lambda p: int(re.sub(r'\D', '', os.path.basename(p)) or 0)):
        lb = rj(d + '/leaderboard.json')
        waves.append((os.path.basename(d), lb, d))
    certified = sum(int((lb or {}).get('qualified_count') or 0) for _, lb, _ in waves)
    champs = [w for w, lb, _ in waves if lb and lb.get('champion') and 'qualified_count' not in lb]
    done_waves = [(w, lb, d) for w, lb, d in waves if lb]
    latest = None
    if done_waves:
        w, lb, d = done_waves[-1]
        designs = lb.get('designs') or []
        best = max(designs, key=lambda x: x.get('w_cad') if isinstance(x.get('w_cad'), (int, float)) else -1e9) if designs else None
        latest = {'wave': w, 'assessed': lb.get('assessed_count'), 'qualified': lb.get('qualified_count'), 'unverified': lb.get('unverified'),
                  'champion': lb.get('champion'), 'mtime_utc': mtime_iso(d + '/leaderboard.json'),
                  'best': {'label': best.get('label'), 'w_cad': best.get('w_cad'), 'battery_pass_count': best.get('battery_pass_count'),
                           'all_16_gates': best.get('all_16_gates'), 'rs_v2_all_hard': best.get('rs_v2_all_hard')} if best else None}
    running = [w for w, lb, d in waves if not lb and os.path.getmtime(d) > time.time() - 6 * 3600]
    r['waves'] = {'latest_completed': latest, 'running_or_pending': running[-3:], 'certified_designs_total': certified,
                  'legacy_champion_waves': champs, 'legacy_note': 'older leaderboards (no qualified_count field) list a champion; per WORKPLAN T02 pre-#84 records do not count as certified', 'scope': 'xbabe2 ~/jarambit/out/w*/leaderboard.json (xbabe3 waves not included)',
                  'recent_completed': [{'wave': w, 'assessed': lb.get('assessed_count'), 'qualified': lb.get('qualified_count')} for w, lb, _ in done_waves[-4:]]}
    r['pr'] = pr_lookup('neverhuman/jarambit', None, ('plan/workplan-w313-w314',))
    r['coord_log'] = {'path': JA + '/COMMS.md', 'tail': tail_lines(JA + '/COMMS.md', 5), 'mtime_utc': mtime_iso(JA + '/COMMS.md')}
    r['needs_jepson'] = needs_jepson(JA + '/COMMS.md')
    r['_hints'] = {}
    return r


# ---------------- active lanes overlay (Addendum 2) ----------------
LANE_RE = re.compile(r'LANE\s+`?([A-Z0-9]+(?:-[A-Z0-9]+)*)`?\s*\|(.*)$')
LANE_STATE = re.compile(r'state\s+`?(in_progress|lane_complete|integrated|done|abandoned|stopped)`?', re.I)
def lane_lines(path):
    """Latest LANE line per task id from a coordination log (format in captains-20261017/COMMON_ADD2.md)."""
    out = {}
    try:
        with open(path, 'rb') as f:
            f.seek(0, 2); size = f.tell(); f.seek(max(0, size - 600000))
            data = f.read().decode('utf-8', 'replace')
    except Exception:
        return out
    for l in clean(data).split('\n'):
        m = LANE_RE.search(l)
        if not m: continue
        st = LANE_STATE.search(m.group(2))
        if not st: continue
        scr = re.search(r'screen\s+`?([\w.\-]+)', m.group(2))
        br = re.search(r'branch\s+`?([\w./\-]+)', m.group(2))
        out[m.group(1)] = {'state': st.group(1).lower(), 'screen': scr.group(1) if scr else None, 'branch': br.group(1).strip('`') if br else None}
    return out

def joil_claims():
    """Active Joil work that the progress ledger cannot express (it only has todo/review/done):
    per-worker next-assignment.json packets and lane supervisor stages."""
    claims = {}
    for f in glob.glob(JCTL + '/*/next-assignment.json'):
        d = rj(f) or {}
        who = os.path.basename(os.path.dirname(f))
        pk = d.get('packet')
        ids = [t.get('id') for t in (d.get('tasks') or []) if isinstance(t, dict) and t.get('id')]
        for tid in ids or ([pk] if pk else []):
            claims[tid] = who + ' next-assignment'
    for f in glob.glob(JCTL + '/status/*-supervisor.json'):
        d = rj(f) or {}
        if d.get('state') not in ('running', 'starting', 'launched'): continue
        for tid in set(re.findall(r'V5-\d\d(?:-\d\d)?', str(d.get('stage') or '').upper())):
            claims[tid] = (d.get('role') or os.path.basename(f)) + ' running stage'
    return claims

def overlay_active(key, p):
    statuses = p.pop('_statuses', None)
    if not statuses:
        p.pop('_stage_sets', None); return
    log = (p.get('coord_log') or {}).get('path')
    lanes = lane_lines(log) if log else {}
    claims = joil_claims() if key == 'joil' else {}
    alive = {w.get('name') for w in []}
    try:
        _, sl, _ = run(['screen', '-ls'], timeout=10)
        alive = set(re.findall(r'^\s+\d+\.(\S+)\s', sl, re.M))
    except Exception:
        pass
    src = {}
    def mark(tid, why):
        targets = [tid] if tid in statuses else [k for k in statuses if k.startswith(tid + '-')]
        for t in targets:
            if bucket(statuses[t]) in ('open',):
                src[t] = why
    for tid, why in claims.items(): mark(tid, why)
    for tid, ln in lanes.items():
        scr = (ln.get('screen') or '').split('.', 1)[-1]
        if scr and alive and scr not in alive and ln['state'] == 'in_progress':
            continue  # LANE line names a screen that is not running
        if ln['state'] in ('in_progress', 'lane_complete', 'integrated'):
            mark(tid, 'LANE ' + ln['state'] + (' @' + ln['screen'] if ln.get('screen') else ''))
    eff = {k: ('in_progress' if k in src else v) for k, v in statuses.items()}
    c = collections.Counter(bucket(v) for v in eff.values())
    ledger_ip = sum(1 for v in statuses.values() if bucket(v) == 'in_progress')
    p['counts'] = {k: c[k] for k in ('done', 'in_progress', 'blocked', 'open')}
    p['in_progress_breakdown'] = {'ledger_state': ledger_ip, 'active_claims_and_lanes': len(src),
        'lane_lines_seen': len(lanes), 'lane_complete': sum(1 for l in lanes.values() if l['state'] == 'lane_complete'),
        'queued_no_live_screen': len(p.get('queued_assignments_no_live_screen') or []),
        'label': 'in progress = ledger in-progress/review/assigned states + tasks with an active claim/assignment or a LANE line (in_progress|lane_complete|integrated) in the coordination log; JopeDime ASSIGNMENT files and LANE lines count only while their named screen is running (others are shown as queued)'}
    p['active_claims'] = dict(sorted(src.items())[:80])
    try:
        compute_stages(key, p, statuses, lanes, src)
    except Exception as e:
        p['stages_error'] = str(e)[:200]

# ---------------- delivery stages (per task) ----------------
STAGES = ['done', 'on_main', 'in_pr', 'implemented', 'in_progress', 'blocked', 'not_started']
STAGE_LABEL = {'done': 'done (acceptance-verified on main)', 'on_main': 'on main, CI green, awaiting acceptance', 'in_pr': 'in integration PR',
               'implemented': 'implemented in lane', 'in_progress': 'in progress', 'blocked': 'blocked', 'not_started': 'not started'}
RANK = {'not_started': 0, 'blocked': 1, 'in_progress': 2, 'implemented': 3, 'in_pr': 4, 'on_main': 5, 'done': 6}
REPO_DIR = {'joil': HOME + '/joil', 'dope': HOME + '/dope', 'jopedime': HOME + '/JopeDime', 'jarambit': HOME + '/jarambit'}
ANC_CACHE = {}

def _rev(repo, ref):
    rc, out, _ = run(['git', '-C', repo, 'rev-parse', '--verify', '-q', ref + '^{commit}'], timeout=10)
    return out.strip() if rc == 0 and out.strip() else None

def _is_anc(repo, a, b):
    k = (repo, a, b)
    if k not in ANC_CACHE:
        rc, _, _ = run(['git', '-C', repo, 'merge-base', '--is-ancestor', a, b], timeout=15)
        ANC_CACHE[k] = (rc == 0)
        if len(ANC_CACHE) > 5000: ANC_CACHE.clear()
    return ANC_CACHE[k]

def compute_stages(key, p, statuses, lanes, active_src):
    """Per-task delivery stage. Only promotes (never demotes) a task as more evidence is found."""
    ss = p.pop('_stage_sets', None) or {}
    stage, why = {}, {}
    held = {}   # ledger-blocked tasks stay 'blocked' (acceptance/audit hold) even when their code is further along
    def promote(t, s, w):
        if t not in statuses: return
        if stage.get(t) == 'blocked' and t not in active_src:
            if RANK[s] > RANK.get(held.get(t, 'not_started'), 0): held[t] = s
            return
        if RANK[s] > RANK.get(stage.get(t, 'not_started'), 0): stage[t] = s; why[t] = w
    for t, v in statuses.items():
        b = 'in_progress' if t in active_src else bucket(v)
        stage[t] = {'done': 'done', 'in_progress': 'in_progress', 'blocked': 'blocked'}.get(b, 'not_started'); why[t] = 'ledger ' + str(v)
    pr = p.get('pr') or {}
    prn = pr.get('number') if pr.get('state') == 'OPEN' else None
    accepted_in_pr = 0
    if key == 'joil':
        # PR #6 merged 2026-10-18 09:55Z (2ad0795): origin/main now carries docs/v5-plan, so tasks done in main's ledger ARE
        # acceptance-verified on main. Done/review only on the integration branch stay 'in PR' (merge is Jepson-only).
        main_done = joil_main_statuses()
        for t, v in statuses.items():
            if main_done.get(t) == 'done':
                stage[t] = 'done'; why[t] = 'ledger done on origin/main'
            elif v in ('done', 'review'):
                stage[t] = 'in_pr'; why[t] = 'ledger %s on integration branch, not yet on main (main merge is Jepson-only)' % v
                if v == 'done': accepted_in_pr += 1
    if key == 'jopedime':
        for t in ss.get('in_progress', ()): promote(t, 'in_progress', 'LIVE_STATUS in_progress_code')
        for t in ss.get('lane_complete', ()): promote(t, 'implemented', 'LIVE_STATUS lane_complete')
        if prn and ss.get('lane_complete_branch') and ss.get('lane_complete_branch') == pr.get('branch'):
            for t in ss.get('lane_complete', ()): promote(t, 'in_pr', 'lane_complete on PR #%s branch %s' % (prn, pr.get('branch')))
        for t in ss.get('on_main', ()): promote(t, 'on_main', 'LIVE_STATUS verified_task_ids or relabeled receipt (main %s, CI %s; no genuine verify-on-main receipt yet)' % (ss.get('main', {}).get('sha'), 'green' if ss.get('main', {}).get('ci_green') else 'not green'))
    repo = REPO_DIR.get(key)
    main_sha = _rev(repo, 'origin/main') if repo else None
    pr_sha = pr.get('head') if prn else None
    if pr_sha and repo and not _rev(repo, pr_sha): pr_sha = None
    for t, ln in (lanes or {}).items():
        targets = [t] if t in statuses else [k for k in statuses if k.startswith(t + '-')]
        for tt in targets:
            if ln['state'] == 'lane_complete': promote(tt, 'implemented', 'LANE lane_complete')
            if ln['state'] == 'integrated': promote(tt, 'in_pr', 'LANE integrated')
            if ln['state'] == 'done': promote(tt, 'implemented', 'LANE done')
            br = ln.get('branch')
            if br in (pr.get('branch'), 'main', 'origin/main'): br = None   # the shared integration/main branch itself says nothing about this lane
            if br and repo and stage.get(tt) != 'done':
                bs = _rev(repo, 'origin/' + br) or _rev(repo, br)
                if bs and main_sha and bs != main_sha and _is_anc(repo, bs, main_sha):
                    promote(tt, 'on_main', 'lane branch %s merged into origin/main' % br)
                elif bs and pr_sha and _is_anc(repo, bs, pr_sha):
                    promote(tt, 'in_pr', 'lane branch %s contained in PR #%s head' % (br, prn))
    cnt = collections.Counter(stage.values())
    p['stages'] = {s: cnt.get(s, 0) for s in STAGES}
    p['stage_labels'] = STAGE_LABEL
    p['stage_meta'] = {'pr_number': pr.get('number'), 'pr_open': bool(prn), 'main_sha': (main_sha or '')[:12] or None,
                       'main_ci_green': (ss.get('main') or {}).get('ci_green'), 'accepted_in_pr': accepted_in_pr,
                       'blocked_code_stage': dict(collections.Counter(held.values())),
                       'note': {'joil': 'done = task done in the origin/main ledger (PR #6 merged 2026-10-18 09:55Z); done/review only on the integration branch = in PR. Blocked = ledger blocked or validator audit_held, even where code is integrated.',
                                'jopedime': 'done = LIVE_STATUS verified_tasks + verify/Ixx.json PASS receipts on a commit in main, verified after that commit existed (relabeled receipts stay on main); on main = verified_task_ids claimed on green main without verify receipts; in PR = lane_complete ids on the open integration PR branch.',
                                'dope': 'done = packet-v2 tasks.json done; LANE lines + lane-branch ancestry (origin/main, PR head) refine the rest.',
                                'jarambit': 'done = WORKPLAN.md DONE; LANE lines + lane-branch ancestry refine the rest.'}.get(key)}
    p['stage_by_task'] = {t: {'stage': stage[t], 'why': why.get(t)} for t in sorted(stage)}

COLLECT = {'joil': collect_joil, 'dope': collect_dope, 'jopedime': collect_jopedime, 'jarambit': collect_jarambit}

def flags_for(key, p):
    f = []
    cap = p.get('captain') or {}
    if not cap.get('alive'): f.append('Captain screen is NOT running')
    elif not cap.get('agent_running'): f.append('Captain screen exists but agy process not found')
    if p.get('error'): f.append(p['error'])
    pr = p.get('pr') or {}
    if pr.get('error'): f.append('GitHub lookup failed: ' + pr['error'])
    ci = pr.get('ci') or {}
    if pr.get('state') == 'OPEN' and ci.get('state') == 'failure':
        f.append('PR #%s CI failing: %s' % (pr.get('number'), ', '.join(x for x in ci.get('failing') or [] if x)))
    for b in p.get('blocked_ids') or []:
        f.append('Blocked: ' + b + ((' — ' + p.get('blocked_detail', {}).get(b, '')) if p.get('blocked_detail') else ''))
    if key == 'joil':
        v = p.get('validator') or {}
        if v.get('fail_mentions'): f.append('Validator latest audit mentions FAIL x%d' % v['fail_mentions'])
        if v.get('weak_mentions'): f.append('Validator latest audit mentions WEAK x%d' % v['weak_mentions'])
        l = p.get('lease') or {}
        try:
            exp = datetime.datetime.fromisoformat(l['expires_utc'])
            hrs = (exp - datetime.datetime.now(datetime.timezone.utc)).total_seconds() / 3600
            if hrs < 2: f.append('Joil lease expires in %.1f h' % hrs)
        except Exception:
            f.append('Joil lease expiry unknown')
    if key == 'dope':
        for b in (p.get('blockers') or [])[:4]:
            if 'block' in b.lower(): f.append('Ledger blocker: ' + b[:200])
    for n in p.get('needs_jepson') or []:
        f.append('NEEDS JEPSON (coordination log): ' + n[:220])
    return f

def cycle():
    STATE['cycle'] += 1
    do_worker_hc = STATE['cycle'] % WORKER_HC_EVERY == 1
    screens = screen_list()
    out = {'generated_utc': now_iso(), 'interval_s': INTERVAL, 'host': 'xbabe2', 'projects': {}, 'order': list(CAPTAINS)}
    for key, fn in COLLECT.items():
        try:
            p = fn(screens, do_worker_hc)
        except Exception as e:
            p = {'name': NAMES[key], 'error': 'collector error: ' + str(e)[:200]}
            traceback.print_exc()
        try:
            overlay_active(key, p)
        except Exception as e:
            p.pop('_statuses', None); p['overlay_error'] = str(e)[:200]
        hints = p.pop('_hints', {}) or {}
        cid = CAPTAINS[key]
        cap_scr = screens.get(cid)
        if not cap_scr:  # fall back by name
            cap_scr = next((v for k, v in screens.items() if v['name'] == cid.split('.', 1)[1]), None)
        p['captain'] = screen_info(cid, cap_scr, True, key, private_guard=(key == 'joil'))
        p['captain']['tail'] = (STATE['worker_info'].get(cid) or {}).get('tail', [])
        workers = []
        for sid, s in sorted(screens.items(), key=lambda kv: kv[1]['name']):
            if sid == cid or project_of(s['name']) != key: continue
            hint = hints.get(sid) or hints.get(s['name'])
            workers.append(screen_info(sid, s, do_worker_hc, key, hint, private_guard=(key == 'joil')))
        for w in workers:
            w['tail'] = w.get('tail', [])[-3:]
        p['workers'] = workers
        p['flags'] = flags_for(key, p)
        out['projects'][key] = p
    try:
        import extras  # dashboard v2: super_captain, resources, hierarchy, token efficiency (read-only)
        out.update(extras.collect(out, screens, {"agent_proc": agent_proc, "descendants": descendants, "screen_info": screen_info, "project_of": project_of}))
    except Exception as e:
        out["extras_error"] = str(e)[:300]; traceback.print_exc()
    wj(HIST, HISTORY)
    wj(OUT, out)

def main():
    while True:
        t0 = time.time()
        try:
            cycle()
            print(time.strftime('%H:%M:%S'), 'status.json written in %.1fs' % (time.time() - t0), flush=True)
        except Exception:
            traceback.print_exc()
        time.sleep(max(5, INTERVAL - (time.time() - t0)))

if __name__ == '__main__':
    if os.environ.get('CDASH_ONCE'):
        cycle()
    else:
        main()
