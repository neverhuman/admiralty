#!/usr/bin/env python3
"""super_captain independent audit (read-only, deterministic).

Recomputes project progress from ledgers/receipts instead of trusting captain claims,
and checks the captain/lane screens for problems an LLM summary tends to miss.

Writes  <OUT>/latest.json, <OUT>/latest.md, appends <OUT>/audit.jsonl, keeps <OUT>/state.json.
Never writes to project repos, never types into screens, never kills anything.
Usage:  python3 sc_audit.py [--out DIR] [--quiet]
"""
import argparse, collections, datetime, glob, hashlib, json, os, re, subprocess, sys, time

HOME = os.path.expanduser('~')
KICK = HOME + '/xbabe2_kicker/captains-20261017'
OUT_DEFAULT = KICK + '/sc_audit'
GH = HOME + '/.local/bin/gh'
CAPTAINS = {'joil': 'captain_joil', 'dope': 'captain_dope', 'jopedime': 'captain_jopedime', 'jarambit': 'captain_jarambit'}
LOGS = {'joil': HOME + '/joil/COORD.md', 'dope': HOME + '/dope/COMMS.md',
        'jopedime': HOME + '/JopeDime/COMMS.md', 'jarambit': HOME + '/jarambit/COMMS.md'}
REPOS = {'joil': HOME + '/joil', 'dope': HOME + '/dope', 'jopedime': HOME + '/JopeDime', 'jarambit': HOME + '/jarambit'}
GHREPO = {'joil': 'neverhuman/joil', 'dope': 'neverhuman/dope', 'jopedime': 'neverhuman/JopeDime', 'jarambit': 'neverhuman/jarambit'}
# xbabe2 CPU slices (CPU_SLICES.md); overflow claims are parsed from the file at run time
SLICES = {'jopedime': '22-31,92-101', 'dope': '78-89', 'jarambit': '6-15', 'joil': '36-39,90-91,110-111,32-35,0-5,16-21,40-43'}
AGENT_RANGE = '0-50,64-114'
OVERFLOW = '44-50,64-67,102-109,112-114'
RESERVED_NON_JOIL = '0-5,16-21,32-43,68-77,90-91,110-111'
JE = HOME + '/jope-evidence/spec-integration-20261017'
DOPE_P = HOME + '/dope/target/unified-spec-workplan-20261017/packet-v2-9e11fd166f5f'
JOIL_WT = HOME + '/joil/.private/worktrees/v5-engineering-plan'

findings = []
def F(sev, scope, code, text, evidence=None):
    findings.append({'severity': sev, 'scope': scope, 'code': code, 'text': text[:600], 'evidence': evidence})

def run(args, timeout=20, cwd=None):
    try:
        p = subprocess.run(args, capture_output=True, text=True, timeout=timeout, cwd=cwd)
        return p.returncode, p.stdout, p.stderr
    except Exception as e:
        return 99, '', str(e)

def rj(p):
    try:
        with open(p) as f: return json.load(f)
    except Exception:
        return None

def tail_text(p, nbytes=400000):
    try:
        with open(p, 'rb') as f:
            f.seek(0, 2); n = f.tell(); f.seek(max(0, n - nbytes)); return f.read().decode('utf-8', 'replace')
    except Exception:
        return ''

def cpuset(s):
    out = set()
    for part in re.split(r'[,\s]+', s or ''):
        if not part: continue
        if '-' in part:
            a, b = part.split('-', 1)
            if a.isdigit() and b.isdigit(): out |= set(range(int(a), int(b) + 1))
        elif part.isdigit(): out.add(int(part))
    return out

def fmt_cpus(s):
    s = sorted(s); r = []; i = 0
    while i < len(s):
        j = i
        while j + 1 < len(s) and s[j + 1] == s[j] + 1: j += 1
        r.append(str(s[i]) if i == j else f'{s[i]}-{s[j]}'); i = j + 1
    return ','.join(r)

# ---------------- screens ----------------
def screens():
    rc, out, _ = run(['screen', '-ls'], timeout=10)
    d = {}
    for m in re.finditer(r'^\s*(\d+)\.(\S+)\s', out, re.M):
        d[m.group(2)] = int(m.group(1))
    return d

def hardcopy(name, path):
    try: os.remove(path)
    except OSError: pass
    run(['screen', '-S', name, '-X', 'hardcopy', path], timeout=10)
    time.sleep(0.3)
    try: return open(path, errors='replace').read()
    except Exception: return ''

def descendants(pid):
    rc, out, _ = run(['ps', '-e', '-o', 'pid=,ppid='], timeout=10)
    kids = collections.defaultdict(list)
    for l in out.split('\n'):
        p = l.split()
        if len(p) == 2: kids[int(p[1])].append(int(p[0]))
    res, stack = [], [pid]
    while stack:
        x = stack.pop()
        for k in kids.get(x, []): res.append(k); stack.append(k)
    return res

SURVEY = re.compile(r'(feedback survey|how (would|likely|satisfied)|rate (your|this|us)|take a (quick )?survey|\bNPS\b|on a scale of|1 = |\[1\] .*\[2\]|thumbs (up|down)\?)', re.I)
QUOTA = re.compile(r'(quota (exceeded|exhausted)|rate.?limit(ed)?|RESOURCE_EXHAUSTED|\b429\b|out of credits|usage limit)', re.I)
APPROVAL = re.compile(r'(Allow (this|once)|Do you want to (proceed|run|allow)|\(y/n\)|\[Y/n\]|Approve\?|Press enter to (confirm|continue))', re.I)

def screen_state(text):
    tail = '\n'.join(text.rstrip('\n').split('\n')[-25:])
    st = {'busy': 'esc to cancel' in tail, 'queued_msgs': bool(re.search(r'queued messages', tail, re.I)),
          'survey': bool(SURVEY.search(tail)), 'quota': bool(QUOTA.search(tail)), 'approval_prompt': bool(APPROVAL.search(tail))}
    last_gt = [l for l in tail.split('\n') if l.startswith('>')]
    st['idle_prompt'] = (not st['busy']) and ('? for shortcuts' in tail) and bool(last_gt) and last_gt[-1].strip() == '>'
    st['typed_input'] = bool(last_gt) and last_gt[-1].strip() not in ('>', '')
    st['hash'] = hashlib.sha256(tail.encode()).hexdigest()[:16]
    return st

_rs = {}
def remote_screens(host):
    if host not in _rs:
        rc, out, _ = run(['ssh', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=8', 'ubuntu@' + host, 'screen -ls'], timeout=20)
        _rs[host] = None if rc not in (0, 1) or 'Socket' not in out and 'No Sockets' not in out else {m.group(2): int(m.group(1)) for m in re.finditer(r'^\s*(\d+)\.(\S+)\s', out, re.M)}
    return _rs[host]

def remote_hash(host, name):
    if host == 'xbabe0': return None  # never
    cmd = f"f=/tmp/sc_audit_hc_{name}.txt; rm -f $f; screen -S {name} -X hardcopy $f; sleep 0.3; tail -n 25 $f | sha256sum | cut -c1-16"
    rc, out, _ = run(['ssh', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=8', 'ubuntu@' + host, cmd], timeout=20)
    return out.strip() or None

# ---------------- LANE lines ----------------
KV = re.compile(r'^(screen|wt|worktree|state|status|cpus|task|host|model|tier|effort|class)\s*[= ]\s*(.+)$', re.I)
def lanes_from_log(project):
    txt = tail_text(LOGS[project], 600000)
    lanes = {}
    for raw in txt.split('\n'):
        if 'LANE' not in raw: continue
        l = raw.replace('`', '').strip().lstrip('-* ').strip()
        if not re.match(r'^LANE\b', l): continue
        segs = [s.strip() for s in l.split('|')]
        d = {}
        m0 = re.match(r'^LANE\s+([A-Z0-9][A-Z0-9-]+)$', segs[0])
        if m0: d['task'] = m0.group(1)
        for s in segs[1:]:
            if re.match(r'^\d+\.[\w.-]+$', s): d['screen'] = s; continue
            for sub in re.split(r'\s+(?=(?:model|tier|effort)=)', s):
                m = KV.match(sub.strip())
                if m:
                    k = m.group(1).lower(); k = {'worktree': 'wt', 'status': 'state'}.get(k, k)
                    d.setdefault(k, m.group(2).strip().split()[0] if k in ('screen', 'wt', 'state', 'cpus', 'host') else m.group(2).strip())
        if not d.get('screen'): continue
        name = re.sub(r'^\d+\.', '', d['screen'])
        d['name'] = name; d['line'] = l[:300]
        lanes[name] = d
    return lanes

# ---------------- git helpers ----------------
def worktrees(repo):
    rc, out, _ = run(['git', '-C', repo, 'worktree', 'list', '--porcelain'], timeout=15)
    wts, cur = {}, None
    for l in out.split('\n'):
        if l.startswith('worktree '): cur = l[9:].strip(); wts[cur] = {'locked': False}
        elif cur and l.startswith('locked'): wts[cur]['locked'] = True
        elif cur and l.startswith('branch '): wts[cur]['branch'] = l[7:].strip()
        elif cur and l.startswith('HEAD '): wts[cur]['head'] = l[5:].strip()
    return wts

def is_anc(repo, a, b):
    rc, _, _ = run(['git', '-C', repo, 'merge-base', '--is-ancestor', a, b], timeout=15)
    return rc == 0

def rev(repo, ref):
    rc, out, _ = run(['git', '-C', repo, 'rev-parse', '--verify', '-q', ref + '^{commit}'], timeout=10)
    return out.strip() if rc == 0 else None

# ---------------- per-project ledgers ----------------
EXEC = re.compile(r'\b(cargo|nextest|pytest|just|npm|pnpm|make|ctest|uv run|go test|gh run|gh api|bash\s+\S+\.sh|\./\S+)\b|\bpython3?\s+(-m\s+\w+|[\w/.-]+\.py)\b')
MEASURE = re.compile(r'\b(benchmark\w*|experiment run\w*|real (run|measurement|workload)|measured (reduction|saving|improvement|speedup)|cost (reduction|saving|improvement)|wall.?clock|on (the )?GPU)\b', re.I)
EXEC_CRIT = re.compile(r'\b(review|CI|exact.head|provenance|receiving|merge|run|runs|tested|tests?|measured|executes?|hash|sha256|receipt)\b', re.I)

def audit_jopedime(sc_claims):
    res = {'source': JE + '/verify/*.json'}
    vdir = JE + '/verify'
    repo = REPOS['jopedime']
    main = rev(repo, 'origin/main') or rev(repo, 'main')
    res['main_sha'] = main
    files = sorted(glob.glob(vdir + '/I[0-9][0-9].json'))
    tasks = rj(JE + '/packet-v4-921643bc3ae0/tasks.json') or {}
    total = len(tasks.get('tasks') or []) or 66
    res['total'] = total; res['receipts'] = len(files)
    present = {os.path.basename(f)[:-5] for f in files}
    expected = {t['id'] for t in tasks.get('tasks') or []}
    missing = sorted(expected - present) if expected else []
    if missing: F('amber', 'jopedime', 'RECEIPT_MISSING', f'{len(missing)} verify receipts missing: {" ".join(missing[:20])}', vdir)
    ok_pass, on_main_pass, bad = [], [], collections.defaultdict(list)
    for f in files:
        tid = os.path.basename(f)[:-5]
        r = rj(f)
        if r is None: bad['unreadable'].append(tid); continue
        st = str(r.get('status', '')).upper()
        sha, csha = str(r.get('sha') or ''), str(r.get('checkout_sha') or '')
        if not re.fullmatch(r'[0-9a-f]{40}', sha) or (csha and not re.fullmatch(r'[0-9a-f]{40}', csha)):
            bad['truncated_or_bad_sha'].append(tid)
        if csha and sha and csha != sha: bad['sha_mismatch'].append(tid)
        for k, v in r.items():
            if 'sha256' in k.lower() and isinstance(v, str) and not re.fullmatch(r'(sha256:)?[0-9a-f]{64}', v): bad['truncated_sha256'].append(tid)
        if st != 'PASS': continue
        cases = r.get('case_to_test_map') or r.get('cases') or []
        cmds = r.get('commands') or []
        if any(c.get('exit_code') not in (0, None) for c in cmds + cases) or any(c.get('passed') is False for c in cases):
            bad['pass_with_nonzero_exit'].append(tid); continue
        texts = [' '.join(str(c.get(k, '')) for k in ('command', 'test_or_code')) for c in cases] or [str(c.get('command', '')) for c in cmds]
        doc = [t for t in texts if 'documentary' in t.lower() or not EXEC.search(t)]
        crit = str(r.get('completion_criterion', ''))
        if 'documentary' in str(r.get('completion_criterion_proof', '')).lower() or (texts and len(doc) == len(texts)):
            # doc-only evidence: fatal when the criterion asks for something executed (review, CI, run, provenance...)
            bad['pass_on_doc_keyword_match' if EXEC_CRIT.search(crit) else 'pass_doc_only_doc_criterion'].append(tid)
            if EXEC_CRIT.search(crit): continue
        if MEASURE.search(crit) and not any('measure' in k.lower() for k in r) and 'measurement' not in json.dumps(r).lower():
            bad['measurement_criterion_unit_tests_only'].append(tid)
        if len(cases) == 0: bad['pass_without_cases'].append(tid); continue
        ok_pass.append(tid)
        if main and re.fullmatch(r'[0-9a-f]{40}', sha) and is_anc(repo, sha, main): on_main_pass.append(tid)
    labels = {'truncated_or_bad_sha': ('red', 'PASS/receipt carries a truncated or malformed git sha'),
              'sha_mismatch': ('amber', 'sha != checkout_sha'),
              'truncated_sha256': ('red', 'sha256 field is truncated'),
              'pass_with_nonzero_exit': ('red', 'status PASS but a command/case has nonzero exit or passed=false'),
              'pass_on_doc_keyword_match': ('red', 'status PASS but evidence is a keyword/doc match while the criterion needs executed evidence (review/CI/run/provenance)'),
              'pass_doc_only_doc_criterion': ('info', 'PASS on doc text only (criterion is itself a document deliverable; spot-check)'),
              'measurement_criterion_unit_tests_only': ('amber', 'criterion needs a real measurement/run; receipt shows tests only'),
              'pass_without_cases': ('red', 'status PASS with no acceptance cases mapped'),
              'unreadable': ('amber', 'receipt JSON unreadable')}
    for k, ids in bad.items():
        sev, txt = labels[k]
        F(sev, 'jopedime', 'RECEIPT_' + k.upper(), f'{txt}: {" ".join(sorted(set(ids)))}', vdir)
    res['genuine_pass'] = len(ok_pass); res['genuine_pass_on_main'] = len(on_main_pass)
    res['suspect'] = {k: sorted(set(v)) for k, v in bad.items()}
    # captain's own counter
    m = None
    for m in re.finditer(r'STATUS \| implemented=(\d+)/\d+ integrated_open_pr=(\d+) on_main_ci_green=(\d+) acceptance_verified=(\d+) done=(\d+)', tail_text(LOGS['jopedime'])):
        pass
    if m:
        res['captain_claim'] = {'implemented': int(m.group(1)), 'acceptance_verified': int(m.group(4)), 'done': int(m.group(5))}
        if int(m.group(5)) > len(on_main_pass):
            F('red', 'jopedime', 'COUNT_OVERCLAIM', f'captain STATUS says done={m.group(5)} but only {len(on_main_pass)} receipts are a genuine PASS on a sha contained in main {str(main)[:9]}', LOGS['jopedime'])
        if int(m.group(4)) > len(ok_pass):
            F('amber', 'jopedime', 'VERIFIED_OVERCLAIM', f'captain STATUS says acceptance_verified={m.group(4)} but audit finds {len(ok_pass)} genuine PASS receipts', vdir)
    res['done_recomputed'] = len(on_main_pass)
    return res

def audit_dope():
    res = {'source': DOPE_P + '/tasks.json'}
    t = rj(DOPE_P + '/tasks.json'); p = rj(DOPE_P + '/progress.json') or {}
    if not t: F('amber', 'dope', 'LEDGER_UNREADABLE', 'tasks.json unreadable', DOPE_P); return res
    st = collections.Counter(x.get('status') for x in t['tasks'])
    done = [x for x in t['tasks'] if x.get('status') == 'done']
    res.update(total=len(t['tasks']), done_recomputed=len(done), status_counts=dict(st))
    pc = p.get('counts') or {}
    if pc and pc.get('done') != len(done):
        F('red', 'dope', 'PROGRESS_VS_LEDGER', f'progress.json counts.done={pc.get("done")} but tasks.json has {len(done)} done', DOPE_P)
    for x in done:
        if not x.get('evidence'): F('red', 'dope', 'DONE_NO_EVIDENCE', f'{x["id"]} is done with empty evidence', DOPE_P + '/tasks.json')
    # stale reviewed heads: shas cited in progress.json that are not the current PR head
    head = p.get('pr_269_head_sha')
    res['pr_head_in_progress_json'] = head
    for l in p.get('active_leases') or []:
        hc = l.get('head_commit')
        if head and hc and hc != head and l.get('pr'):
            F('amber', 'dope', 'LEASE_STALE_HEAD', f'lease {l.get("lease_id")} still pins head {hc[:9]} while PR #269 head is {head[:9]}; any acceptance tied to {hc[:9]} must be re-checked on the current head', DOPE_P + '/progress.json')
    claim = max_claim(LOGS['dope'], len(t['tasks']), CAPTAINS['dope'])
    res['captain_claim_max_recent'] = claim
    if claim is not None and claim > len(done):
        F('red', 'dope', 'COUNT_OVERCLAIM', f'captain screen/log recently claimed {claim}/{len(t["tasks"])} done; ledger tasks.json has {len(done)}', LOGS['dope'])
    return res

def max_claim(logp, total, scr):
    """Highest done-count the captain itself is claiming NOW: its screen tail + its own latest STATUS lines."""
    hc = '/tmp/sc_audit_hc_' + scr + '.txt'
    scr_txt = '\n'.join(open(hc, errors='replace').read().split('\n')[-150:]) if os.path.exists(hc) else ''
    own = [l for l in tail_text(logp, 12000).split('\n') if scr in l and 'STATUS' in l.upper()][-2:]
    txt = scr_txt + '\n' + '\n'.join(own)
    txt = '\n'.join(l for l in txt.split('\n') if not re.search(r'gorkbot|grok ?bot|ENG.watcher|super_captain|\bNOT\b|claimed|\bclaim\b|\brev\d+|ledger', l, re.I))
    vals = [int(m.group(1)) for m in re.finditer(r'\b(\d{1,3})\s*/\s*%d\b[^\n]{0,25}\b(done|complete|accepted|verified)' % total, txt, re.I)]
    vals += [int(m.group(2)) for m in re.finditer(r'\b(done|complete|accepted)\b[^\n]{0,6}?[:= ]\s*(\d{1,3})\s*/\s*%d\b' % total, txt, re.I)]
    return max(vals) if vals else None

def audit_jarambit():
    res = {}
    cands = [REPOS['jarambit'] + '/docs/WORKPLAN.md'] + [w + '/docs/WORKPLAN.md' for w in worktrees(REPOS['jarambit'])]
    ex = [c for c in cands if os.path.isfile(c)]
    if not ex: F('amber', 'jarambit', 'LEDGER_MISSING', 'docs/WORKPLAN.md not found'); return res
    wp = max(ex, key=os.path.getmtime); res['source'] = wp
    header, rows = None, {}
    for line in open(wp, errors='replace'):
        if not line.startswith('|'): header = None; continue
        cells = [c.strip() for c in line.strip().strip('|').split('|')]
        if header is None: header = [c.lower() for c in cells]; continue
        if all(set(c) <= set('-: ') for c in cells) or 'task id' not in header or 'status' not in header: continue
        tid = cells[header.index('task id')].strip('` *') if len(cells) > header.index('task id') else ''
        if not re.match(r'^[A-Z0-9]+(-[A-Z0-9]+)+$', tid): continue
        si = header.index('status')
        if len(cells) != len(header): si = len(cells) - (len(header) - si)
        rows[tid] = cells[si] if 0 <= si < len(cells) else ''
    def b(s):
        u = s.upper()
        if 'BLOCKED' in u: return 'blocked'
        if re.search(r'\bDONE\b|\bVERIFIED\b|\bACCEPTED\b', u): return 'done'
        return 'other'
    for k, s in rows.items():
        u = s.upper()
        if 'BLOCKED' in u and re.search(r'\bDONE\b', u):
            F('red', 'jarambit', 'AMBIGUOUS_STATUS', f'{k} status cell contains both BLOCKED and DONE ("{s[:80]}"); naive parsers count it done', wp)
    c = collections.Counter(b(s) for s in rows.values())
    res.update(total=len(rows), done_recomputed=c['done'], blocked=c['blocked'])
    txt = open(wp, errors='replace').read()
    m = re.search(r'Currently Done[^\n]*?`(\d+) / (\d+)`', txt)
    if m:
        res['declared_counter'] = f'{m.group(1)}/{m.group(2)}'
        if int(m.group(1)) > c['done'] or int(m.group(2)) != len(rows):
            F('amber', 'jarambit', 'COUNTER_MISMATCH', f'WORKPLAN counter says {m.group(1)}/{m.group(2)}, table parses {c["done"]}/{len(rows)} (BLOCKED rows not counted)', wp)
    return res

def audit_joil():
    res = {}
    try:
        tasks = rj(JOIL_WT + '/docs/v5-plan/TASKS.json')
        sts = {}
        for pk in tasks['packets']:
            st = rj(JOIL_WT + '/docs/v5-plan/progress/' + pk['id'] + '.json') or {'tasks': {}}
            for t in pk['tasks']:
                sts[t['id']] = ((st['tasks'].get(t['id']) or {}).get('status'), (st['tasks'].get(t['id']) or {}).get('evidence'))
        done = [k for k, (s, e) in sts.items() if s == 'done']
        pk_done = sum(1 for pk in tasks['packets'] if pk['tasks'] and all(sts[t['id']][0] == 'done' for t in pk['tasks']))
        res.update(total=len(sts), done_recomputed=len(done), packets_accepted=pk_done, packets_total=len(tasks['packets']),
                   source=JOIL_WT + '/docs/v5-plan/progress/*.json')
        for k in done:
            if not sts[k][1]: F('amber', 'joil', 'DONE_NO_EVIDENCE', f'{k} done with empty evidence', JOIL_WT + '/docs/v5-plan/progress')
        val = open(HOME + '/joil/.private/v5-workers/gorkbot/validator/LATEST.md', errors='replace').read().split('## Previous')[0]
        held = sorted(set(re.findall(r'(V5-\d\d-\d\d)[^\n]{0,60}AUDIT-HELD', val)))
        bad = [k for k in held if k in done]
        if bad: F('red', 'joil', 'DONE_BUT_AUDIT_HELD', f'validator AUDIT-HELD but ledger done: {" ".join(bad)}', 'joil validator LATEST.md')
        res['validator_audit_held'] = held
    except Exception as e:
        F('amber', 'joil', 'LEDGER_UNREADABLE', str(e)[:200])
    return res

def open_prs(project):
    rc, out, _ = run([GH, 'pr', 'list', '-R', GHREPO[project], '--state', 'open', '--json',
                      'number,headRefName,headRefOid,isDraft,reviewDecision,statusCheckRollup,updatedAt', '-L', '10'], timeout=30)
    try: prs = json.loads(out)
    except Exception: return []
    res = []
    for p in prs:
        roll = p.get('statusCheckRollup') or []
        concl = collections.Counter((c.get('conclusion') or c.get('state') or c.get('status') or 'UNKNOWN').upper() for c in roll)
        failing = sum(v for k, v in concl.items() if k in ('FAILURE', 'ERROR', 'CANCELLED', 'TIMED_OUT', 'ACTION_REQUIRED'))
        pending = sum(v for k, v in concl.items() if k in ('PENDING', 'QUEUED', 'IN_PROGRESS', 'EXPECTED', 'WAITING'))
        ci = 'red' if failing else ('pending' if pending or not roll else 'green')
        res.append({'number': p['number'], 'branch': p['headRefName'], 'head': p['headRefOid'][:9], 'draft': p['isDraft'],
                    'review': p.get('reviewDecision'), 'ci': ci, 'checks': dict(concl)})
        if ci == 'red': F('amber', project, 'PR_CI_RED', f'PR #{p["number"]} @ {p["headRefOid"][:9]} has failing checks {dict(concl)}', f'gh pr checks {p["number"]} -R {GHREPO[project]}')
    return res

def main_ci(project):
    rc, out, _ = run([GH, 'run', 'list', '-R', GHREPO[project], '--branch', 'main', '-L', '6', '--json', 'headSha,conclusion,status,workflowName,createdAt'], timeout=30)
    try: runs = json.loads(out)
    except Exception: return None
    if not runs: return None
    head = runs[0]['headSha']
    same = [r for r in runs if r['headSha'] == head]
    bad = [r['workflowName'] for r in same if r.get('conclusion') in ('failure', 'cancelled', 'timed_out')]
    if bad: F('amber', project, 'MAIN_CI_RED', f'main @ {head[:9]} failing: {", ".join(bad)}')
    return {'head': head[:9], 'runs': [(r['workflowName'], r.get('conclusion') or r.get('status')) for r in same]}

# ---------------- main ----------------
def main():
    ap = argparse.ArgumentParser(); ap.add_argument('--out', default=OUT_DEFAULT); ap.add_argument('--quiet', action='store_true')
    ap.add_argument('--no-gh', action='store_true')
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    now = datetime.datetime.now(datetime.timezone.utc)
    state = rj(a.out + '/state.json') or {'hashes': {}}
    prev = state.get('hashes', {})
    scr = screens()
    newh = {}
    report = {'utc': now.isoformat(timespec='seconds'), 'projects': {}, 'screens': {}, 'lanes': {}}

    # 1. captain + super_captain screens
    for proj, name in list(CAPTAINS.items()) + [('super_captain', 'super_captain')]:
        if name not in scr:
            F('red', proj, 'SCREEN_MISSING', f'{name} screen is not running'); continue
        txt = hardcopy(name, f'/tmp/sc_audit_hc_{name}.txt')
        s = screen_state(txt); newh[name] = {'hash': s['hash'], 'since': prev.get(name, {}).get('since') if prev.get(name, {}).get('hash') == s['hash'] else now.isoformat(timespec='seconds')}
        report['screens'][name] = {k: v for k, v in s.items() if k != 'hash'}
        if s['survey']: F('red', proj, 'SURVEY_PROMPT', f'{name} shows a survey/feedback prompt that blocks the turn; answer/dismiss it (Esc) before nudging', f'/tmp/sc_audit_hc_{name}.txt')
        if s['approval_prompt']: F('red', proj, 'APPROVAL_PROMPT', f'{name} is waiting on an approval/confirm prompt', f'/tmp/sc_audit_hc_{name}.txt')
        if s['quota']: F('amber', proj, 'QUOTA_TEXT', f'{name} tail mentions quota/rate limit', f'/tmp/sc_audit_hc_{name}.txt')
        if s['queued_msgs']:
            F('amber', proj, 'QUEUED_MESSAGES', f'{name} has user-queued messages not yet delivered (busy turn); pasted nudges wait until the turn ends', f'/tmp/sc_audit_hc_{name}.txt')
        since = newh[name]['since']
        age = (now - datetime.datetime.fromisoformat(since)).total_seconds() / 60 if since else 0
        report['screens'][name]['unchanged_min'] = round(age)
        if age >= 25 and s['busy']: F('amber', proj, 'SCREEN_FROZEN', f'{name} busy but screen unchanged for {round(age)} min (hung tool call?)')
        if age >= 25 and s['idle_prompt'] and proj != 'super_captain': F('amber', proj, 'CAPTAIN_IDLE', f'{name} idle at prompt for {round(age)} min')
        if s['typed_input']: report['screens'][name]['note'] = 'text typed at prompt (maybe Jepson) - do not overwrite'

    # 2. lanes: alive, stalled, shared/locked worktrees, cpu pinning
    overflow_claims = collections.defaultdict(set)
    for l in tail_text(KICK + '/CPU_SLICES.md', 200000).split('\n'):
        m = re.match(r'^(\d+),(\d+)\s+(\w+)\s+(\S+)\s+\S+Z\s*$', l.strip())
        if m: overflow_claims[m.group(4)] |= {int(m.group(1)), int(m.group(2))}
    allowed_range = cpuset(AGENT_RANGE)
    for proj in CAPTAINS:
        lanes = lanes_from_log(proj)
        wts = worktrees(REPOS[proj]); locked = {w for w, v in wts.items() if v.get('locked')}
        running = {n: d for n, d in lanes.items() if str(d.get('state', '')).lower() in ('running', 'in_progress', 'active')}
        by_wt = collections.defaultdict(list)
        out = {}; dead = []
        wt_roots = sorted(wts, key=len, reverse=True)
        for n, d in running.items():
            host = d.get('host', 'xbabe2')
            info = {'task': d.get('task'), 'wt': d.get('wt'), 'host': host, 'alive': None}
            if host == 'xbabe2':
                info['alive'] = n in scr
                if not info['alive']:
                    dead.append(f'{n}({d.get("task")})')
                else:
                    txt = hardcopy(n, f'/tmp/sc_audit_hc_{n}.txt'); s = screen_state(txt)
                    p = prev.get(n, {})
                    newh[n] = {'hash': s['hash'], 'since': p.get('since') if p.get('hash') == s['hash'] else now.isoformat(timespec='seconds')}
                    age = (now - datetime.datetime.fromisoformat(newh[n]['since'])).total_seconds() / 60
                    info['unchanged_min'] = round(age)
                    if age >= 30: F('amber', proj, 'LANE_STALLED', f'{n} ({d.get("task")}) screen unchanged {round(age)} min while LANE says running', f'/tmp/sc_audit_hc_{n}.txt')
                    if s['survey'] or s['approval_prompt']: F('red', proj, 'LANE_PROMPT_BLOCKED', f'{n} is stuck on a survey/approval prompt', f'/tmp/sc_audit_hc_{n}.txt')
                    if s['quota']: F('amber', proj, 'LANE_QUOTA', f'{n} mentions quota/rate limit', f'/tmp/sc_audit_hc_{n}.txt')
                    # cpu pinning of the agent process tree
                    pids = descendants(scr[n]); allowed = set(); cwd = None
                    for pid in pids:
                        try:
                            cl = open(f'/proc/{pid}/cmdline', 'rb').read().replace(b'\0', b' ').decode('utf-8', 'replace')
                            if not re.search(r'codex|claude|agy|cargo|python|pytest|rustc|node', cl): continue
                            allowed = cpuset(re.search(r'Cpus_allowed_list:\s*(\S+)', open(f'/proc/{pid}/status').read()).group(1))
                            cwd = os.readlink(f'/proc/{pid}/cwd'); break
                        except Exception: continue
                    if cwd:
                        root = next((w for w in wt_roots if cwd == w or cwd.startswith(w + '/')), None)
                        if root: info['wt_actual'] = root; d = dict(d, wt=root)
                    if allowed:
                        info['cpus_allowed'] = fmt_cpus(allowed)
                        own = cpuset(SLICES.get(proj, '')) | overflow_claims.get(n, set())
                        reserved = cpuset(RESERVED_NON_JOIL)
                        if len(allowed) >= 120: F('amber', proj, 'LANE_UNPINNED', f'{n} is not CPU-pinned (all {len(allowed)} CPUs)')
                        elif allowed - allowed_range: F('red', proj, 'LANE_CPU_OUTSIDE_AGENT_RANGE', f'{n} may run on {fmt_cpus(allowed - allowed_range)} (outside 0-50,64-114)')
                        elif proj != 'joil' and allowed & reserved and len(allowed) < 60: F('red', proj, 'LANE_CPU_RESERVED', f'{n} pinned to {fmt_cpus(allowed)}, overlapping reserved CPUs {fmt_cpus(allowed & reserved)} (Joil lease/68-77 are off-limits per CPU_SLICES.md/COMMON.md)')
                        elif proj != 'joil' and allowed - own - cpuset(OVERFLOW): F('amber', proj, 'LANE_CPU_OUTSIDE_SLICE', f'{n} pinned to {fmt_cpus(allowed)}; not confined to {proj} slice {SLICES[proj]} (+claimed overflow)')
                wt = d.get('wt')
                if wt and info['alive']:
                    wt = wt.rstrip('/')
                    if wt in locked: F('red', proj, 'LOCKED_WORKTREE_IN_USE', f'{n} ({d.get("task")}) runs in LOCKED worktree {wt}', LOGS[proj])
                    if wt != REPOS[proj]: by_wt[wt].append(n)
            elif host in ('xbabe1', 'xbabe3'):
                rs = remote_screens(host)
                if rs is None: info['alive'] = 'unreachable'
                else:
                    info['alive'] = n in rs
                    if not info['alive']: dead.append(f'{n}@{host}({d.get("task")})')
                    else:
                        h = remote_hash(host, n)
                        key = host + ':' + n; p = prev.get(key, {})
                        newh[key] = {'hash': h, 'since': p.get('since') if p.get('hash') == h else now.isoformat(timespec='seconds')}
                        age = (now - datetime.datetime.fromisoformat(newh[key]['since'])).total_seconds() / 60
                        info['unchanged_min'] = round(age)
                        if h and age >= 30: F('amber', proj, 'LANE_STALLED', f'{n}@{host} ({d.get("task")}) screen unchanged {round(age)} min while LANE says running')
            out[n] = info
        if dead: F('amber', proj, 'LANE_LINES_STALE', f'{len(dead)} LANE lines still say running but their screens are gone: {" ".join(dead[:25])}; captain must post closing LANE lines', LOGS[proj])
        for wt, ns in by_wt.items():
            if len(ns) > 1:
                F('amber', proj, 'TWO_WRITERS_ONE_WORKTREE', f'{len(ns)} running lanes share worktree {wt}: {", ".join(ns)} (one writer per worktree; read-only verify lanes must use their own checkout)', LOGS[proj])
        # process cwd check: anything running inside a locked worktree
        for w in locked:
            users = []
            for pdir in glob.glob('/proc/[0-9]*'):
                try:
                    c = os.readlink(pdir + '/cwd')
                    if c == w or c.startswith(w + '/'): users.append(int(pdir[6:]))
                except Exception: pass
            if users: F('red', proj, 'LOCKED_WORKTREE_PROCESS', f'{len(users)} process(es) have cwd inside locked worktree {w} (pids {users[:5]})', w)
        report['lanes'][proj] = out

    # 3. ledgers / receipts recomputed
    report['projects']['jopedime'] = audit_jopedime(None)
    report['projects']['dope'] = audit_dope()
    report['projects']['jarambit'] = audit_jarambit()
    report['projects']['joil'] = audit_joil()
    if not a.no_gh:
        for proj in CAPTAINS:
            report['projects'][proj]['open_prs'] = open_prs(proj)
            report['projects'][proj]['main_ci'] = main_ci(proj)

    # 4. super_captain's own last report vs recomputed numbers
    scs = rj(KICK + '/super_captain_status.json') or {}
    report['super_captain_status_utc'] = scs.get('updated_utc')
    for proj, r in report['projects'].items():
        claimed = ((scs.get('captains') or {}).get(proj) or {}).get('done')
        r['super_captain_reported_done'] = claimed
        cands = {r.get('done_recomputed'), r.get('packets_accepted')} - {None}
        if isinstance(claimed, int) and cands and claimed > max(cands):
            F('red', proj, 'SC_REPORTED_OVERCLAIM', f'super_captain_status.json reports done={claimed} but the ledger gives {r.get("done_recomputed")}/{r.get("total")}' + (f' (packets {r.get("packets_accepted")}/{r.get("packets_total")})' if r.get('packets_total') else '') + ' - super_captain must take done counts from this audit, not from captain claims', KICK + '/super_captain_status.json')
    try:
        upd = datetime.datetime.fromisoformat(str(scs.get('updated_utc')).replace('Z', '+00:00'))
        if (now - upd).total_seconds() > 40 * 60: F('red', 'super_captain', 'SC_STALE', f'super_captain_status.json not updated for {int((now - upd).total_seconds() / 60)} min')
    except Exception:
        F('amber', 'super_captain', 'SC_STATUS_UNREADABLE', 'super_captain_status.json missing updated_utc')

    sev_rank = {'red': 0, 'amber': 1, 'info': 2}
    findings.sort(key=lambda f: (sev_rank.get(f['severity'], 3), f['scope'], f['code']))
    report['findings'] = findings
    report['summary'] = dict(collections.Counter(f['severity'] for f in findings))
    state['hashes'] = newh; state['last_utc'] = report['utc']
    json.dump(state, open(a.out + '/state.json.tmp', 'w'), indent=1); os.replace(a.out + '/state.json.tmp', a.out + '/state.json')
    json.dump(report, open(a.out + '/latest.json.tmp', 'w'), indent=1, default=str); os.replace(a.out + '/latest.json.tmp', a.out + '/latest.json')
    with open(a.out + '/audit.jsonl', 'a') as f:
        f.write(json.dumps({'utc': report['utc'], 'summary': report['summary'], 'findings': findings,
                            'done': {p: r.get('done_recomputed') for p, r in report['projects'].items()}}, default=str) + '\n')
    md = [f'# super_captain audit {report["utc"]}', '',
          '| Project | done (recomputed) / total | super_captain reported | captain claim | source |', '|---|---|---|---|---|']
    for p, r in report['projects'].items():
        md.append(f'| {p} | {r.get("done_recomputed")} / {r.get("total")} | {r.get("super_captain_reported_done")} | {r.get("captain_claim") or r.get("captain_claim_max_recent") or r.get("declared_counter") or ""} | {r.get("source", "")} |')
    md += ['', f'## Findings ({report["summary"]})', '']
    md += [f'- **{f["severity"]}** `{f["scope"]}` {f["code"]}: {f["text"]}' + (f' (evidence: `{f["evidence"]}`)' if f.get('evidence') else '') for f in findings] or ['- none']
    open(a.out + '/latest.md', 'w').write('\n'.join(md) + '\n')
    if not a.quiet: print('\n'.join(md))
    return 0

if __name__ == '__main__':
    sys.exit(main())
