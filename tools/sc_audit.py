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
CAPTAINS = {'joil': 'captain_joil', 'dope': 'captain_dope', 'jopedime': 'captain_jopedime', 'jarambit': 'captain_jarambit',
            'admiralty': 'agy_captain_admiralty'}   # agy captain (Jepson 5:25 AM MDT Oct 18); the Codex screen captain_admiralty is its worker
LOGS = {'joil': HOME + '/joil/COORD.md', 'dope': HOME + '/dope/COMMS.md',
        'jopedime': HOME + '/JopeDime/COMMS.md', 'jarambit': HOME + '/jarambit/COMMS.md',
        'admiralty': HOME + '/admiralty-v2-build/COMMS.md'}
REPOS = {'joil': HOME + '/joil', 'dope': HOME + '/dope', 'jopedime': HOME + '/JopeDime', 'jarambit': HOME + '/jarambit',
         'admiralty': HOME + '/.admiralty-captain'}   # captain's own clone; never ~/admiralty (Codex working tree)
GHREPO = {'joil': 'neverhuman/joil', 'dope': 'neverhuman/dope', 'jopedime': 'neverhuman/JopeDime', 'jarambit': 'neverhuman/jarambit',
          'admiralty': 'neverhuman/admiralty'}
# xbabe2 CPU slices (CPU_SLICES.md); overflow claims are parsed from the file at run time
SLICES = {'jopedime': '22-31,92-101', 'dope': '78-89', 'jarambit': '6-15', 'joil': '36-39,90-91,110-111,32-35,0-5,16-21,40-43',
          'admiralty': '102-109,112-113,64-65'}
ADM = HOME + '/admiralty-v2-build'   # Admiralty v2 rollout state (read-only here; deployments/ and controller/ belong to the local Codex integrator)
ADM_DONE_STATES = ('integrated',)    # recorded provenance only; receipt contents/main acceptance are not verified here
ADM_FINISHED = ('finished', 'integrated', 'verified', 'closed', 'retired', 'released')  # exact lane states, never substrings
AGENT_RANGE = '0-50,64-114'
OVERFLOW = '44-50,64-67,102-109,112-114'
RESERVED_NON_JOIL = '0-5,16-21,32-43,68-77,90-91,110-111'
JE = HOME + '/jope-evidence/spec-integration-20261017'
JE_MR = HOME + '/jope-evidence/major-release-20261018'
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

SURVEY = re.compile(r'(feedback survey|how (would|likely|satisfied)|rate (your|this|us)|take a (quick )?survey|\bNPS\b|on a scale of|(?<![A-Za-z0-9])1\s*[=:]\s*\w+[^\n]{0,40}(?<![A-Za-z0-9])2\s*[=:]\s*\w+|\[1\] .*\[2\]|thumbs (up|down)\?)', re.I)
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

def commit_time(repo, sha):
    rc, out, _ = run(['git', '-C', repo, 'log', '-1', '--format=%ct', sha], timeout=10)
    try:
        return datetime.datetime.fromtimestamp(int(out.strip()), datetime.timezone.utc) if rc == 0 else None
    except ValueError:
        return None

def parse_utc(v):
    if not v: return None
    try:
        d = datetime.datetime.fromisoformat(str(v).replace('Z', '+00:00'))
        return d if d.tzinfo else d.replace(tzinfo=datetime.timezone.utc)
    except ValueError:
        return None

def rev(repo, ref):
    rc, out, _ = run(['git', '-C', repo, 'rev-parse', '--verify', '-q', ref + '^{commit}'], timeout=10)
    return out.strip() if rc == 0 else None

# ---------------- per-project ledgers ----------------
EXEC = re.compile(r'\b(cargo|nextest|pytest|just|npm|pnpm|make|ctest|uv run|go test|gh run|gh api|bash\s+\S+\.sh|\./\S+)\b|\bpython3?\s+(-m\s+\w+|[\w/.-]+\.py)\b')
MEASURE = re.compile(r'\b(benchmark\w*|experiment run\w*|real (run|measurement|workload)|measured (reduction|saving|improvement|speedup|information/byte|cost|bytes?)|cost (reduction|saving|improvement)|cheaper|wall.?clock|on (the )?GPU)\b', re.I)
EXEC_CRIT = re.compile(r'\b(review|CI|exact.head|provenance|receiving|merge|run|runs|tested|tests?|measured|executes?|hash|sha256|receipt)\b', re.I)

def audit_jopedime_major_release(repo, main):
    tracker_candidates = [
        repo + '/docs/plans/major-release-20261018/MAJOR_RELEASE_TRACKER.json',
        JE_MR + '/tasks.json',
        KICK + '/jopedime_major_release_20261018/MAJOR_RELEASE_TRACKER.json',
    ]
    tpath = next((p for p in tracker_candidates if os.path.isfile(p)), tracker_candidates[0])
    tdata = rj(tpath) or {}
    vdir = str(tdata.get('verify_dir') or (JE_MR + '/verify'))
    tlist = [t for t in (tdata.get('tasks') or []) if isinstance(t, dict) and t.get('id')]
    tmap = {t['id']: t for t in tlist}
    req_task_keys = ('id', 'owner_screen', 'deps', 'gate', 'exact_acceptance_test', 'receipt_format')
    bad = collections.defaultdict(list)
    for tid, t in tmap.items():
        if any(k not in t or t.get(k) in (None, '') for k in req_task_keys):
            bad['schema_incomplete_task'].append(tid)
    files = sorted(glob.glob(vdir + '/MR[0-9][0-9].json'))
    ok_pass, on_main_pass = [], []
    raw_pass = {}
    for f in files:
        tid = os.path.basename(f)[:-5]
        r = rj(f)
        if r is None:
            bad['unreadable'].append(tid); continue
        st = str(r.get('status', '')).upper()
        sha, csha = str(r.get('sha') or ''), str(r.get('checkout_sha') or '')
        if not re.fullmatch(r'[0-9a-f]{40}', sha) or not re.fullmatch(r'[0-9a-f]{40}', csha):
            bad['truncated_or_bad_sha'].append(tid); continue
        if sha != csha:
            bad['sha_mismatch'].append(tid); continue
        if st != 'PASS':
            continue
        va = parse_utc(r.get('verified_at_utc') or r.get('verified_at'))
        ct = commit_time(repo, sha)
        if va and ct and va < ct:
            bad['receipt_predates_sha'].append(tid); continue
        if main and sha != main:
            bad['sha_not_current_main'].append(tid); continue
        t_spec = tmap.get(tid) or {}
        exp_cmd = str(t_spec.get('exact_acceptance_test') or '').strip()
        exp_ac = [str(x).strip() for x in (t_spec.get('acceptance_cases') or []) if str(x).strip()]
        cases = r.get('case_to_test_map') or r.get('cases') or []
        if not cases or any(c.get('exit_code') != 0 or c.get('passed') is False for c in cases):
            bad['pass_with_nonzero_exit'].append(tid); continue
        act_ac = [str(c.get('acceptance_case') or '').strip() for c in cases]
        act_cmds = [str(c.get('command') or '').strip() for c in cases]
        if (exp_ac and not set(exp_ac).issubset(set(act_ac))) or (exp_cmd and not any(exp_cmd in c for c in act_cmds)):
            bad['unmapped_exact_acceptance_test'].append(tid); continue
        if not all(EXEC.search(c) for c in act_cmds if c):
            bad['non_executable_command'].append(tid); continue
        lbad = False
        for c in cases:
            lp, hs = c.get('log_path'), c.get('log_sha256')
            if not lp or not hs or not re.fullmatch(r'[0-9a-f]{64}', str(hs)):
                lbad = True; break
            try:
                if hashlib.sha256(open(lp, 'rb').read()).hexdigest() != hs:
                    lbad = True; break
            except OSError:
                lbad = True; break
        if lbad:
            bad['log_sha256_mismatch'].append(tid); continue
        raw_pass[tid] = sha
    for tid, sha in sorted(raw_pass.items()):
        deps = (tmap.get(tid) or {}).get('deps') or []
        if any(d not in raw_pass for d in deps):
            bad['deps_not_passed'].append(tid); continue
        ok_pass.append(tid)
        if main and sha == main:
            on_main_pass.append(tid)
    for k, ids in bad.items():
        F('red', 'jopedime', 'MR_' + k.upper(), f'major-release tracker {k}: {" ".join(sorted(set(ids)))}', vdir)
    return {
        'source': tpath,
        'verify_dir': vdir,
        'total': len(tmap),
        'receipts': len(files),
        'genuine_pass': len(ok_pass),
        'genuine_pass_on_main': len(on_main_pass),
        'done_recomputed': len(on_main_pass),
        'suspect': {k: sorted(set(v)) for k, v in bad.items()},
    }

def audit_jopedime(sc_claims):
    res = {'source': JE + '/verify/*.json'}
    vdir = JE + '/verify'
    repo = REPOS['jopedime']
    main = rev(repo, 'origin/main') or rev(repo, 'main')
    res['main_sha'] = main
    files = sorted(glob.glob(vdir + '/I[0-9][0-9].json'))
    tasks = rj(JE + '/packet-v4-921643bc3ae0/tasks.json') or {}
    tmap = {t['id']: t for t in (tasks.get('tasks') or []) if isinstance(t, dict) and t.get('id')}
    total = len(tmap) or 66
    res['total'] = total; res['receipts'] = len(files)
    present = {os.path.basename(f)[:-5] for f in files}
    expected = set(tmap.keys())
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
        mism = bool(csha and sha and csha != sha)
        if mism: bad['sha_mismatch'].append(tid)
        for k, v in r.items():
            if 'sha256' in k.lower() and isinstance(v, str) and not re.fullmatch(r'(sha256:)?[0-9a-f]{64}', v): bad['truncated_sha256'].append(tid)
        if st != 'PASS' or mism: continue  # a PASS verified on a different checkout than it names is not genuine
        # relabel guard: a receipt cannot have been verified on a commit that did not exist yet
        va = parse_utc(r.get('verified_at_utc') or r.get('verified_at'))
        ct = commit_time(repo, sha) if re.fullmatch(r'[0-9a-f]{40}', sha) else None
        if va and ct and va < ct:
            bad['receipt_predates_sha'].append(tid); continue
        proof = str(r.get('completion_criterion_proof', ''))
        if set(re.findall(r'\b[0-9a-f]{40}\b', proof)) - {sha} and sha not in proof:
            bad['proof_names_other_sha'].append(tid); continue
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
        # Audit Gap 4: A receipt sha that is only an ancestor of main (e.g. I47 on d5b18bc) is not current main
        if main and sha != main:
            bad['sha_not_current_main'].append(tid); continue
        # Audit Gap 3: Empty completion_criterion or blank/unmapped acceptance_case compared against packet-v4 tasks.json
        t_spec = tmap.get(tid) or {}
        exp_crit = str(t_spec.get('completion_criterion') or '').strip()
        exp_ac = [str(x).strip() for x in (t_spec.get('acceptance_cases') or []) if str(x).strip()]
        act_ac = [str(c.get('acceptance_case') or '').strip() for c in cases]
        if not crit.strip() or (exp_crit and crit.strip() != exp_crit) or any(not a for a in act_ac) or (exp_ac and not set(exp_ac).issubset(set(act_ac))):
            bad['empty_or_unmapped_criterion_cases'].append(tid); continue
        if len(cases) == 0: bad['pass_without_cases'].append(tid); continue
        # Audit Gap 1: Script-edited receipt without a real log_path + matching log_sha256 (e.g. finalize_66_of_66.py on I63/I64)
        exec_cases = [c for c in cases if EXEC.search(' '.join(str(c.get(k, '')) for k in ('command', 'test_or_code')))]
        if exec_cases and not any(c.get('log_path') and c.get('log_sha256') for c in cases + cmds):
            bad['relabeled_without_log'].append(tid); continue
        # Audit Gap 2: Rewritten proof SHAs (proof names only main_sha while referencing evidence dirs under other SHAs or stripping prior SHAs from *.relabeled*.json without prior_evidence_sha)
        raw_r = json.dumps(r)
        ev_shas = set(re.findall(r'work-packets/evidence/I\d+/([0-9a-f]{40})', raw_r)) - {sha}
        proof_shas = set(re.findall(r'\b[0-9a-f]{40}\b', proof))
        prior_ev = r.get('prior_evidence_sha')
        prior_set = set(prior_ev) if isinstance(prior_ev, list) else ({str(prior_ev)} if prior_ev else set())
        rewritten = False
        if ev_shas and proof_shas == {sha} and not (ev_shas <= prior_set):
            rewritten = True
        for rf in glob.glob(vdir + '/' + tid + '.relabeled*.json'):
            rr = rj(rf) or {}
            rproof = str(rr.get('completion_criterion_proof') or '')
            rshas = set(re.findall(r'\b[0-9a-f]{40}\b', rproof)) - {sha}
            if rshas and not (rshas <= (proof_shas | prior_set)):
                rewritten = True
        if rewritten:
            bad['rewritten_proof_sha'].append(tid)
        if MEASURE.search(crit) and not any('measure' in k.lower() for k in r) and 'measurement' not in raw_r.lower():
            bad['measurement_criterion_unit_tests_only'].append(tid); continue
        # proof honesty: a proof that says every command uses --exact while a cargo test command does not is a false receipt
        allcmds = [str(c.get('command', '')) for c in cases + cmds if c.get('command')]
        if re.search(r'\b(each|every|all)\b[^.]*--exact', proof, re.I) and any('cargo test' in c and '--exact' not in c for c in allcmds):
            bad['proof_claim_false_exact'].append(tid); continue
        # reused execution: a case that reuses an execution from another commit was not re-run on the named sha
        if any(c.get('reused_existing_execution') and str(c.get('source_commit') or '')[:7] and not sha.startswith(str(c.get('source_commit'))[:7]) for c in cases):
            bad['case_reused_other_commit'].append(tid)
        # retry-to-green: an earlier FAIL receipt for this task on the same sha (I##.fail.json etc.)
        for ff in glob.glob(vdir + '/' + tid + '.*fail*.json'):
            fr = rj(ff) or {}
            if str(fr.get('sha') or '') != sha or str(fr.get('status', '')).upper() == 'PASS': continue
            fva = parse_utc(fr.get('verified_at_utc') or fr.get('verified_at'))
            if va and fva and fva > va: continue
            fcmds = [str(c.get('command', '')) for c in (fr.get('case_to_test_map') or fr.get('cases') or [])]
            pcmds = [str(c.get('command', '')) for c in cases]
            if fcmds != pcmds: bad['pass_after_fail_cmd_changed'].append(tid)
            else: bad['pass_after_fail_same_cmd'].append(tid + '(' + str(fr.get('host', '?')) + '->' + str(r.get('host', '?')) + ')')
            break
        # log integrity: a case log that no longer matches the sha256 the receipt recorded was overwritten (e.g. lane log-name collision)
        lbad = False
        for c in cases:
            lp, hs = c.get('log_path'), c.get('log_sha256')
            if lp and hs and re.fullmatch(r'[0-9a-f]{64}', str(hs)):
                try:
                    if hashlib.sha256(open(lp, 'rb').read()).hexdigest() != hs: lbad = True
                except OSError:
                    lbad = True
        if lbad: bad['log_sha256_mismatch'].append(tid); continue
        ok_pass.append(tid)
        if main and sha == main: on_main_pass.append(tid)
    labels = {'truncated_or_bad_sha': ('red', 'PASS/receipt carries a truncated or malformed git sha'),
              'sha_mismatch': ('red', 'sha != checkout_sha (verified a different commit than the receipt names; not counted)'),
              'truncated_sha256': ('red', 'sha256 field is truncated'),
              'pass_with_nonzero_exit': ('red', 'status PASS but a command/case has nonzero exit or passed=false'),
              'pass_on_doc_keyword_match': ('red', 'status PASS but evidence is a keyword/doc match while the criterion needs executed evidence (review/CI/run/provenance)'),
              'pass_doc_only_doc_criterion': ('info', 'PASS on doc text only (criterion is itself a document deliverable; spot-check)'),
              'measurement_criterion_unit_tests_only': ('red', 'criterion needs a real measurement/run or explicit hypothesis qualification; receipt shows unit tests only (not counted)'),
              'sha_not_current_main': ('red', 'receipt sha is an older ancestor commit, not current main (not counted)'),
              'empty_or_unmapped_criterion_cases': ('red', 'empty completion_criterion or blank/unmapped/reworded acceptance_case vs packet-v4 tasks.json (not counted)'),
              'relabeled_without_log': ('red', 'receipt edited by script (finalize_*) without a new log_path and matching log_sha256 (relabeled; not counted)'),
              'rewritten_proof_sha': ('amber', 'completion_criterion_proof rewrote prior commit SHAs to main_sha without preserving prior_evidence_sha'),
              'pass_without_cases': ('red', 'status PASS with no acceptance cases mapped'),
              'receipt_predates_sha': ('red', 'verified_at is older than the commit the receipt names (relabeled to a newer sha, not re-run; not counted)'),
              'proof_names_other_sha': ('red', 'proof says the tests ran at a different full sha than the receipt names (not counted)'),
              'log_sha256_mismatch': ('red', 'a case log is missing or no longer matches the log_sha256 in the receipt (overwritten; not counted)'),
              'proof_claim_false_exact': ('red', 'proof says every command uses --exact but a cargo test command does not (substring filter may run other tests; not counted)'),
              'case_reused_other_commit': ('amber', 'a case reuses an execution from a different commit instead of re-running on the named sha (spot-check)'),
              'pass_after_fail_cmd_changed': ('amber', 'PASS follows a FAIL on the same sha with changed commands (retry-to-green; confirm the change is a legit fix, not a weakened check)'),
              'pass_after_fail_same_cmd': ('amber', 'PASS follows a FAIL on the same sha with identical commands (flaky or host-dependent, failing->passing host shown; root-cause before it counts toward the xbabe1-4 order)'),
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
    res['major_release'] = audit_jopedime_major_release(repo, main)
    return res

# ---------------- DOPE per-task receipts (super_captain request 2026-10-18 ~06:00 MDT) ----------------
# A DOPE task counts as done only when its own receipt proves it: receipt file exists (and matches the #sha256 pinned in
# tasks.json), status passed, integer exit code(s) == 0 (never prose), every command has a log_path that exists with a
# matching log_sha256, checkout sha == receipt sha (40-hex) and that sha is on origin/main, and the receipt is this task's
# own (a receipt shared by several tasks counts only for tasks that have their own per-task entry with their own log).
DOPE_VERIFY_REF = re.compile(r'(/\S+?/verify/\S+?\.json)(?:#sha256=([0-9a-f]{64}))?\s*$')
DOPE_GENERIC_CMD_MIN = 5      # same command proving >= this many tasks is reported (amber) as generic evidence
DOPE_CONTRADICTORY_SUBSTATUS = re.compile(r'blocked|partial|fail', re.I)
DOPE_PROBLEMS = {
    'no_receipt': 'no verify receipt referenced in tasks.json evidence (prose/typed evidence only)',
    'receipt_missing': 'referenced receipt file does not exist',
    'receipt_unreadable': 'receipt JSON unreadable',
    'receipt_sha256_mismatch': 'receipt file no longer matches the #sha256 pinned in tasks.json (edited after pinning)',
    'receipt_not_for_task': 'receipt names a different task_id',
    'shared_receipt_no_task_entry': 'receipt is shared by several tasks and has no per-task entry (with its own log) for this task',
    'status_not_passed': 'receipt status is not passed',
    'contradictory_sub_status': 'ledger sub_status explicitly says blocked, partial or failed despite status=done',
    'bad_or_missing_sha': 'receipt sha missing, truncated or malformed (need 40-hex)',
    'no_commands': 'receipt lists no executed commands',
    'exit_code_not_integer': 'exit code missing or typed as text/prose instead of an integer',
    'nonzero_exit': 'a command exited nonzero',
    'no_log_path': 'a command has no log_path',
    'log_missing': 'log_path does not exist',
    'no_log_sha256': 'log_sha256 missing or malformed',
    'log_sha256_mismatch': 'log file does not match log_sha256 (overwritten or edited)',
    'log_names_other_sha': 'log header names a different head_sha than the receipt',
    'log_exit_code_mismatch': 'log header exit_code differs from the recorded integer exit code',
    'log_shared_across_tasks': 'the same log (same log_sha256) is cited as proof for several tasks',
    'no_checkout_sha': 'no checkout_sha/head_sha recorded for the commands',
    'checkout_sha_mismatch': 'checkout sha differs from the receipt sha',
    'sha_not_on_main': 'receipt sha is not on origin/main',
    'receipt_predates_sha': 'verified_at is older than the commit it names (relabeled, not re-run)',
    'vacuous_zero_tests': 'log shows the test command ran ZERO tests (cargo "0 passed ... N filtered out", unittest "Ran 0 tests", pytest "no tests ran") - a filter that matches nothing passes vacuously',
    'not_a_test_command': 'command is not a test/validation run (sha256sum, git log, gh run list, ls, cat, echo, ...) - proves a file exists, not that acceptance passes',
    'empty_input_hash': 'log contains the SHA-256 of EMPTY input (e3b0c442...) - a hash command was run with no file arguments',
}
# GB red-team 2026-10-18 ~06:45 MDT: captain_dope receipts on 402954ed included cargo filters matching 0 tests
# (D00 D01 B00 W00 U00 R10) and re-pinned planning receipts to `sha256sum ` with no arguments (P00).
DOPE_NON_TEST_CMDS = {'sha256sum', 'sha1sum', 'md5sum', 'git', 'gh', 'ls', 'cat', 'echo', 'stat', 'wc', 'true', 'test', 'head', 'tail', 'jq', 'printf', 'find', 'du'}
EMPTY_SHA256 = 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855'

def dope_cmd_program(cmd):
    """First real program of a shell command, skipping VAR=value assignments and env/taskset/nice/timeout wrappers."""
    toks = str(cmd or '').strip().split()
    wrappers = ('env', 'nice', 'ionice', 'time', 'command', 'exec', 'taskset', 'timeout', 'stdbuf')
    wrapped = False
    for t in toks:
        if re.fullmatch(r'[A-Za-z_][A-Za-z0-9_]*=\S*', t): continue
        if t in wrappers: wrapped = True; continue
        if wrapped and (t.startswith('-') or re.fullmatch(r'[0-9][0-9,.\-]*[smhd]?', t)): continue
        return os.path.basename(t)
    return ''

def dope_log_vacuous(text):
    """True when the log proves the test runner executed zero tests."""
    cargo = re.findall(r'test result: \w+\. (\d+) passed; (\d+) failed', text)
    if cargo and sum(int(a) + int(b) for a, b in cargo) == 0: return True
    ran = [int(x) for x in re.findall(r'^Ran (\d+) tests? in', text, re.M)]
    if ran and sum(ran) == 0: return True
    if re.search(r'^(no tests ran|collected 0 items)\b', text, re.M): return True
    return False

def _sha256_file(p):
    try:
        with open(p, 'rb') as f: return hashlib.sha256(f.read()).hexdigest()
    except OSError:
        return None

def dope_receipt_refs(task):
    refs = []
    for e in task.get('evidence') or []:
        m = DOPE_VERIFY_REF.search(str(e).strip())
        if m: refs.append((m.group(1), m.group(2)))
    return refs

def check_dope_receipt(task_id, ref_path, ref_hash, on_main, commit_time_fn=None):
    """Verify one DOPE task receipt. Returns (sorted problem codes, info). on_main(sha) -> bool. Never writes."""
    probs, info = set(), {'receipt': ref_path}
    if not os.path.isfile(ref_path): return ['receipt_missing'], info
    if ref_hash and _sha256_file(ref_path) != ref_hash: probs.add('receipt_sha256_mismatch')
    r = rj(ref_path)
    if not isinstance(r, dict): return sorted(probs | {'receipt_unreadable'}), info
    entry = r
    if r.get('task_id') != task_id:
        per = r.get('tasks')
        if isinstance(per, dict) and isinstance(per.get(task_id), dict): entry = per[task_id]
        elif isinstance(per, list) and any(isinstance(x, dict) and x.get('task_id') == task_id for x in per):
            entry = next(x for x in per if isinstance(x, dict) and x.get('task_id') == task_id)
        else:
            return sorted(probs | {'receipt_not_for_task' if r.get('task_id') else 'shared_receipt_no_task_entry'}), info
        info['shared_receipt'] = True
    def g(k): return entry.get(k) if entry.get(k) is not None else (r.get(k) if entry is not r else None)
    if str(g('status') or '').strip().lower() not in ('passed', 'pass'): probs.add('status_not_passed')
    sha = str(g('sha') or g('head_sha') or '')
    info['sha'] = sha
    good_sha = bool(re.fullmatch(r'[0-9a-f]{40}', sha))
    if not good_sha: probs.add('bad_or_missing_sha')
    cmds = entry.get('commands')
    if not isinstance(cmds, list) or not cmds:
        cmds = [entry] if (entry.get('log_path') or entry.get('cmd') or entry.get('command')) else []
    if not cmds: probs.add('no_commands')
    checkout = {str(entry['checkout_sha'])} if entry.get('checkout_sha') else set()
    info['cmds'], info['log_hashes'] = [], []
    for c in cmds:
        if not isinstance(c, dict): probs.add('no_commands'); continue
        info['cmds'].append(str(c.get('cmd') or c.get('command') or ''))
        prog = dope_cmd_program(c.get('cmd') or c.get('command'))
        if prog in DOPE_NON_TEST_CMDS: probs.add('not_a_test_command')
        ec = c['exit_codes'] if 'exit_codes' in c else c.get('exit_code')
        ecs = ec if isinstance(ec, list) else [ec]
        if not ecs or any(type(x) is not int for x in ecs): probs.add('exit_code_not_integer')
        elif any(x != 0 for x in ecs): probs.add('nonzero_exit')
        lp, lh = c.get('log_path'), c.get('log_sha256')
        if not lp: probs.add('no_log_path')
        elif not os.path.isfile(str(lp)): probs.add('log_missing')
        elif not (isinstance(lh, str) and re.fullmatch(r'(sha256:)?[0-9a-f]{64}', lh)): probs.add('no_log_sha256')
        elif _sha256_file(lp) != lh.split(':')[-1]: probs.add('log_sha256_mismatch')
        else:
            info['log_hashes'].append(lh.split(':')[-1])
            with open(lp, errors='replace') as fh: full = fh.read(2000000)
            head = full[:20000]
            if dope_log_vacuous(full): probs.add('vacuous_zero_tests')
            if EMPTY_SHA256 in full: probs.add('empty_input_hash')
            ls = set(re.findall(r'^head_sha:\s*([0-9a-f]{40})\s*$', head, re.M))
            if ls and good_sha and ls != {sha}: probs.add('log_names_other_sha')
            le = {int(x) for x in re.findall(r'^exit_code:\s*(-?\d+)\s*$', head, re.M)}
            if le and all(type(x) is int for x in ecs) and not set(ecs) <= le: probs.add('log_exit_code_mismatch')
        cs = c.get('checkout_sha') or c.get('head_sha')
        if cs: checkout.add(str(cs))
    if not checkout: probs.add('no_checkout_sha')
    elif checkout != {sha}: probs.add('checkout_sha_mismatch')
    if good_sha and not on_main(sha): probs.add('sha_not_on_main')
    va = parse_utc(g('verified_at_utc') or g('verified_at'))
    ct = commit_time_fn(sha) if (commit_time_fn and good_sha) else None
    if va and ct and va < ct: probs.add('receipt_predates_sha')
    return sorted(probs), info

def verify_dope_tasks(tasks, on_main, commit_time_fn=None):
    """Per-task verification for DOPE ledger tasks marked done. Returns (verified ids, {id: problems}, generic_cmds)."""
    verified, bad, infos = [], {}, {}
    for x in tasks:
        if x.get('status') != 'done': continue
        tid = x.get('id')
        contradiction = bool(DOPE_CONTRADICTORY_SUBSTATUS.search(str(x.get('sub_status') or '')))
        refs = dope_receipt_refs(x)
        if not refs:
            bad[tid] = sorted({'no_receipt'} | ({'contradictory_sub_status'} if contradiction else set()))
            continue
        p, i = check_dope_receipt(tid, refs[-1][0], refs[-1][1], on_main, commit_time_fn)
        if contradiction: p = sorted(set(p) | {'contradictory_sub_status'})
        infos[tid] = i
        if p: bad[tid] = p
    # one receipt / one log per task: the same log cited for several tasks is not per-task proof
    by_log = collections.defaultdict(set)
    for tid, i in infos.items():
        for h in i.get('log_hashes') or []: by_log[h].add(tid)
    for h, ids in by_log.items():
        if len(ids) > 1:
            for tid in ids: bad[tid] = sorted(set(bad.get(tid, [])) | {'log_shared_across_tasks'})
    for tid in infos:
        if tid not in bad: verified.append(tid)
    by_cmd = collections.defaultdict(list)
    for tid in verified:
        for c in set(infos[tid].get('cmds') or []): by_cmd[c].append(tid)
    generic = {c: sorted(ids) for c, ids in by_cmd.items() if c and len(ids) >= DOPE_GENERIC_CMD_MIN}
    return sorted(verified), bad, generic

def audit_dope(packet=None, on_main=None, commit_time_fn=None):
    packet = packet or DOPE_P
    repo = REPOS['dope']
    res = {'source': packet + '/tasks.json (+ per-task verify receipts)'}
    t = rj(packet + '/tasks.json'); p = rj(packet + '/progress.json') or {}
    if not t: F('amber', 'dope', 'LEDGER_UNREADABLE', 'tasks.json unreadable', packet); return res
    if on_main is None:
        run(['git', '-C', repo, 'fetch', '-q', 'origin', 'main'], timeout=60)
        main = rev(repo, 'origin/main')
        res['main_sha'] = main
        on_main = (lambda s: bool(main) and is_anc(repo, s, main))
        commit_time_fn = commit_time_fn or (lambda s: commit_time(repo, s))
    st = collections.Counter(x.get('status') for x in t['tasks'])
    done = [x for x in t['tasks'] if x.get('status') == 'done']
    verified, bad, generic = verify_dope_tasks(t['tasks'], on_main, commit_time_fn)
    res.update(total=len(t['tasks']), ledger_done=len(done), done_recomputed=len(verified), status_counts=dict(st),
               unverified_done=sorted(bad), note='done_recomputed counts only ledger-done tasks whose own receipt verifies (log+sha256, int exit codes, checkout sha on main)')
    res['receipt_problems'] = {k: v for k, v in sorted(bad.items())}
    shas = collections.Counter()
    for x in done:
        refs = dope_receipt_refs(x)
        rr = (rj(refs[-1][0]) or {}) if refs else {}
        shas[str(rr.get('head_sha') or rr.get('sha') or 'none')[:9]] += 1
    res['receipt_shas'] = dict(shas)
    pc = p.get('counts') or {}
    if pc and pc.get('done') != len(done):
        F('red', 'dope', 'PROGRESS_VS_LEDGER', f'progress.json counts.done={pc.get("done")} but tasks.json has {len(done)} done', packet)
    for x in done:
        if not x.get('evidence'): F('red', 'dope', 'DONE_NO_EVIDENCE', f'{x["id"]} is done with empty evidence', packet + '/tasks.json')
    contra = sorted(x['id'] for x in done if DOPE_CONTRADICTORY_SUBSTATUS.search(str(x.get('sub_status') or '')))
    if contra:
        F('red', 'dope', 'DONE_BUT_SUBSTATUS_BLOCKED', f'{len(contra)} task(s) are status=done while their own sub_status says blocked/partial/failed: {" ".join(contra)} - mark them blocked with the precise blocker instead of done', packet + '/tasks.json')
    stale = sorted(x['id'] for x in done if re.search(r'awaiting|pending', str(x.get('sub_status') or ''), re.I) and x['id'] not in contra)
    if stale:
        F('amber', 'dope', 'DONE_SUBSTATUS_STALE', f'{len(stale)} done task(s) still carry an awaiting/pending sub_status (stale bookkeeping): {" ".join(stale[:30])}', packet + '/tasks.json')
    by_prob = collections.defaultdict(list)
    for tid, ps in bad.items():
        for q in ps: by_prob[q].append(tid)
    for q, ids in sorted(by_prob.items()):
        F('red', 'dope', 'RECEIPT_' + q.upper(), f'{DOPE_PROBLEMS.get(q, q)} (not counted as done): {len(ids)} task(s) {" ".join(sorted(ids)[:40])}', packet + '/verify')
    if bad:
        F('red', 'dope', 'DONE_UNVERIFIED', f'{len(bad)}/{len(done)} ledger-done tasks have no verifiable per-task receipt; audited done = {len(verified)}/{len(t["tasks"])}', packet + '/tasks.json')
    for c, ids in sorted(generic.items(), key=lambda kv: -len(kv[1])):
        F('amber', 'dope', 'RECEIPT_GENERIC_COMMAND', f'one generic command is the only proof for {len(ids)} different tasks ({" ".join(ids[:25])}): `{c[:120]}`; each has its own log so they count, but captain_dope must show the command exercises each task\'s acceptance', packet + '/verify')
    head = p.get('pr_269_head_sha')
    res['pr_head_in_progress_json'] = head
    for l in p.get('active_leases') or []:
        hc = l.get('head_commit')
        if head and hc and hc != head and l.get('pr'):
            F('amber', 'dope', 'LEASE_STALE_HEAD', f'lease {l.get("lease_id")} still pins head {hc[:9]} while PR #269 head is {head[:9]}; any acceptance tied to {hc[:9]} must be re-checked on the current head', packet + '/progress.json')
    claim = max_claim(LOGS['dope'], len(t['tasks']), CAPTAINS['dope'])
    res['captain_claim_max_recent'] = claim
    if claim is not None and claim > len(verified):
        F('red', 'dope', 'COUNT_OVERCLAIM', f'captain screen/log recently claimed {claim}/{len(t["tasks"])} done; audit verifies {len(verified)} (ledger says {len(done)})', LOGS['dope'])
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
    # Done counts only from what is on main. Never pick the newest-mtime WORKPLAN across worktrees:
    # stale detached /tmp worktrees and unmerged feature branches flap the count (false red 2026-10-18 10:00Z).
    repo = REPOS['jarambit']
    try: subprocess.run(['git', '-C', repo, 'fetch', '-q', 'origin', 'main'], timeout=30, capture_output=True)
    except Exception: pass
    r = subprocess.run(['git', '-C', repo, 'show', 'origin/main:docs/WORKPLAN.md'], capture_output=True, text=True)
    if r.returncode == 0 and r.stdout.strip():
        sha = subprocess.run(['git', '-C', repo, 'rev-parse', '--short=9', 'origin/main'], capture_output=True, text=True).stdout.strip()
        wp = f'{repo}@origin/main({sha}):docs/WORKPLAN.md'; lines = r.stdout.splitlines(True)
    elif os.path.isfile(repo + '/docs/WORKPLAN.md'):
        wp = repo + '/docs/WORKPLAN.md'; lines = open(wp, errors='replace').readlines()
        F('amber', 'jarambit', 'LEDGER_NOT_FROM_MAIN', 'could not read origin/main:docs/WORKPLAN.md; fell back to the main checkout file', wp)
    else:
        F('amber', 'jarambit', 'LEDGER_MISSING', 'docs/WORKPLAN.md not found on origin/main or in the main checkout'); return res
    res['source'] = wp
    header, rows = None, {}
    for line in lines:
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
    txt = ''.join(lines)
    m = re.search(r'Currently Done[^\n]*?`(\d+) / (\d+)`', txt)
    if m:
        res['declared_counter'] = f'{m.group(1)}/{m.group(2)}'
        if int(m.group(1)) > c['done'] or int(m.group(2)) != len(rows):
            F('amber', 'jarambit', 'COUNTER_MISMATCH', f'WORKPLAN counter says {m.group(1)}/{m.group(2)}, table parses {c["done"]}/{len(rows)} (BLOCKED rows not counted)', wp)
    return res

def _age_min(now, v):
    t = parse_utc(v)
    return None if t is None else (now - t).total_seconds() / 60

def admiralty_verification_record(packet, now):
    """Recognize recorded rollout provenance, never receipt contents or acceptance.

    Match the rollout reference fields. Legacy references have no outcome field;
    when one is supplied, only explicit positive outcomes are understood.
    """
    proof = packet.get('verification')
    if not isinstance(proof, dict): return False
    placeholders = ('unknown', 'unverified', 'false', 'none', 'null', 'pending', 'failed')
    for key in ('receipt', 'revision', 'verifier', 'observed_utc'):
        value = proof.get(key)
        if (not isinstance(value, str) or not 1 <= len(value) <= 1000 or not value.strip()
                or value.strip().lower() in placeholders or any(ord(c) < 32 or ord(c) == 127 for c in value)):
            return False
    if not re.fullmatch(r'[0-9a-f]{40}|[0-9a-f]{64}', proof['revision']): return False
    try:
        observed = datetime.datetime.fromisoformat(proof['observed_utc'].replace('Z', '+00:00'))
        if observed.tzinfo is None or observed > now: return False
    except (ValueError, TypeError, OverflowError):
        return False
    for key in ('status', 'result', 'outcome'):
        if key in proof and (not isinstance(proof[key], str)
                             or proof[key].strip().lower() not in ('pass', 'passed', 'success', 'verified')):
            return False
    for key in ('passed', 'verified', 'success'):
        if key in proof and proof[key] is not True: return False
    candidate = packet.get('candidate_revision')
    if candidate is not None and candidate != '' and candidate != proof['revision']: return False
    integration = packet.get('integration')
    if isinstance(integration, dict) and 'revision' in integration and integration['revision'] != proof['revision']:
        return False
    claim = packet.get('claim')
    if isinstance(claim, dict) and claim.get('owner') == proof['verifier']: return False
    return True

def audit_admiralty(scr, prev, newh, now):
    """Admiralty v2: packets from rollout.json, workers from live-lanes.json + screens. Read-only."""
    res = {'source': ADM + '/rollout.json (+ live-lanes.json)'}
    r = rj(ADM + '/rollout.json')
    if not r or not isinstance(r.get('packets'), list):
        F('amber', 'admiralty', 'LEDGER_UNREADABLE', 'rollout.json unreadable', ADM + '/rollout.json'); return res
    pk = r['packets']
    st = collections.Counter(str(x.get('status')) for x in pk)
    done = [x for x in pk if x.get('status') in ADM_DONE_STATES and admiralty_verification_record(x, now)]
    claimed = [x for x in pk if x.get('status') in ('verified', 'integrated')]
    res.update(total=len(pk), done_recomputed=len(done), status_counts=dict(st), blocked=st.get('blocked', 0),
               verified_or_integrated_claimed=len(claimed), pr=r.get('pr'),
               note='done = integrated with a well-formed recorded verification reference; receipt contents and acceptance on main are not independently verified by this audit')
    for x in claimed:
        if not admiralty_verification_record(x, now):
            F('red', 'admiralty', 'DONE_NO_EVIDENCE', f'{x.get("id")} is {x.get("status")} with missing, invalid or contradictory verification provenance', ADM + '/rollout.json')
    age = _age_min(now, r.get('updated_utc'))
    res['rollout_age_min'] = None if age is None else round(age)
    if age is not None and age >= 30:
        F('amber', 'admiralty', 'ROLLOUT_STALE', f'rollout.json not updated for {round(age)} min (packet statuses on the dashboard are stale)', ADM + '/rollout.json')
    ll = rj(ADM + '/live-lanes.json') or {}
    lanes = ll.get('lanes') if isinstance(ll, dict) else None
    if not isinstance(lanes, list):
        F('amber', 'admiralty', 'LANES_UNREADABLE', 'live-lanes.json unreadable', ADM + '/live-lanes.json'); return res
    workers, dead, stale, unverified = {}, [], [], []
    for l in lanes:
        n = re.sub(r'^\d+\.', '', str(l.get('screen') or l.get('id') or '?'))
        status = str(l.get('status') or '')
        a = _age_min(now, l.get('updated_utc'))
        info = {'packet': l.get('packet_id') or l.get('packet') or l.get('id') or l.get('branch'), 'branch': l.get('branch'), 'status': status,
                'reported_age_min': None if a is None else round(a), 'alive': n in scr}
        finished = status.strip().lower() in ADM_FINISHED
        if not info['alive'] and not finished: dead.append(f'{n}({info["packet"]})')
        if info['alive']:
            s = screen_state(hardcopy(n, f'/tmp/sc_audit_hc_{n}.txt')); p = prev.get(n, {})
            newh[n] = {'hash': s['hash'], 'since': p.get('since') if p.get('hash') == s['hash'] else now.isoformat(timespec='seconds')}
            info['unchanged_min'] = round((now - datetime.datetime.fromisoformat(newh[n]['since'])).total_seconds() / 60)
            info['idle_prompt'] = s['idle_prompt']
            if s['survey'] or s['approval_prompt']: F('red', 'admiralty', 'LANE_PROMPT_BLOCKED', f'{n} is stuck on a survey/approval prompt', f'/tmp/sc_audit_hc_{n}.txt')
            if s['quota']: F('amber', 'admiralty', 'LANE_QUOTA', f'{n} mentions quota/rate limit', f'/tmp/sc_audit_hc_{n}.txt')
            if not finished and ((a is not None and a >= 30) or info['unchanged_min'] >= 30):
                stale.append(f'{n}({info["packet"]}, reported {info["reported_age_min"]}m ago, screen unchanged {info["unchanged_min"]}m{", idle" if s["idle_prompt"] else ""})')
        if 'unverified' in status.lower() and a is not None and a >= 60: unverified.append(f'{n}({info["packet"]})')
        workers[n] = info
    res['workers'] = workers
    if dead: F('amber', 'admiralty', 'LANE_DEAD', f'{len(dead)} live-lanes entries are not finished but their screens are gone: {" ".join(dead[:20])}', ADM + '/live-lanes.json')
    if stale: F('amber', 'admiralty', 'LANE_STALE', f'{len(stale)} workers stale (>=30 min without a reported transition or screen change): {"; ".join(stale[:20])}. agy_captain_admiralty must re-drive or close them', ADM + '/live-lanes.json')
    if unverified: F('amber', 'admiralty', 'UNVERIFIED_CANDIDATES_AGING', f'{len(unverified)} candidates committed but unverified for >=60 min: {" ".join(unverified[:20])}; drive verification on the exact commit, then hand to the integrator', ADM + '/live-lanes.json')
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
        res['status_counts'] = dict(collections.Counter(s for s, e in sts.values()))
        res['blocked'] = res['status_counts'].get('blocked', 0)
        notes = {}
        for pk in tasks['packets']:
            st = rj(JOIL_WT + '/docs/v5-plan/progress/' + pk['id'] + '.json') or {'tasks': {}}
            for t in pk['tasks']:
                v = st['tasks'].get(t['id']) or {}
                if v.get('status') == 'blocked': notes[t['id']] = v.get('note') or ''
        res['blocked_reasons'] = notes
        # done on origin/main (what is actually merged), read-only via git show
        rc, out, _ = run(['git', '-C', REPOS['joil'], 'ls-tree', '--name-only', 'origin/main', 'docs/v5-plan/progress/'], timeout=15)
        if rc == 0 and out.strip():
            dm = 0
            for f in out.split():
                rc2, js, _ = run(['git', '-C', REPOS['joil'], 'show', 'origin/main:' + f], timeout=15)
                try: dm += sum(1 for v in json.loads(js)['tasks'].values() if v.get('status') == 'done')
                except Exception: pass
            res['done_on_main'] = dm
            try:
                fa = audit_joil_flips(REPOS['joil'], 'origin/main')
                res['flip_audit'] = fa
                res['done_on_main_unflagged'] = fa['done_unflagged_on_main']
            except Exception as e:
                F('amber', 'joil', 'FLIP_AUDIT_ERROR', str(e)[:200])
    except Exception as e:
        F('amber', 'joil', 'LEDGER_UNREADABLE', str(e)[:200])
    return res

# ---------------- Joil status-flip dones (GB red-team 2026-10-18, Joil PR #9 85->155) ----------------
# A Joil task that sat blocked on external causes (Cause B measured data / Cause E owner authority) must
# not become "done" merely because a Fleet Admiral decision unblocked it. Flags, read-only on origin/main:
#   JOIL_DONE_STATUS_FLIP        red   task turned done in a first-parent commit that added < JOIL_FLIP_MIN_PROD_LINES
#                                      production (non-test, non-doc) lines per newly-done task, after an external-cause block
#                                      or a Fleet Admiral (SC-DEC) unblock; waived only by joil_flip_waived()
#   JOIL_DONE_DECISION_SAYS_OPEN red   the disposition for a done task itself says its verification remains open / not executed
#   JOIL_SUBSTITUTE_UNLABELLED   red   disposition is substitute:/rescope: but the task's progress entry carries no such label
#   JOIL_DONE_SHARED_RECEIPT     amber no receipt names the task with <= 2 task_ids (only packet-wide aggregate receipts)
#   JOIL_LOG_SUMMARY_ONLY        amber every test log behind the task is a one-line summary (no individual test names)
JOIL_FLIP_MIN_PROD_LINES = 10
JOIL_EXTERNAL_CAUSE = re.compile(r'Cause [BE]\b|measured|metrology|unsigned|signed|owner|authority|physical|accession|SC-DEC-\d+|Fleet Admiral|unblocked by', re.I)
JOIL_OPEN_RX = re.compile(r'remains? open|not yet executed|not executed|verification remains', re.I)
JOIL_PROGRESS_DIR = 'docs/v5-plan/progress'

def joil_is_prod_path(p):
    if not p.endswith(('.rs', '.py', '.ts', '.tsx', '.js')): return False
    base = p.rsplit('/', 1)[-1]
    if p.startswith(('docs/', 'tests/', 'experiments/')) or '/tests/' in p: return False
    if base.startswith('test_') or base.endswith(('_test.py', '.test.ts', '.test.tsx', '.spec.ts')): return False
    return True

def joil_prod_added_lines(repo, base, head):
    """Added production lines base..head; Rust lines at/after the first #[cfg(test)] count as test code."""
    rc, out, _ = run(['git', '-C', repo, 'diff', '-U0', '--no-color', base, head], timeout=90)
    total, per, cur, cut = 0, {}, None, None
    for line in out.splitlines():
        if line.startswith('+++ '):
            path = line[6:] if line.startswith('+++ b/') else None
            cur = path if path and joil_is_prod_path(path) else None
            cut = None
            if cur and cur.endswith('.rs'):
                rc2, src, _ = run(['git', '-C', repo, 'show', f'{head}:{cur}'], timeout=15)
                for i, l in enumerate(src.splitlines(), 1):
                    if l.strip() == '#[cfg(test)]': cut = i; break
            continue
        if cur and line.startswith('@@'):
            m = re.search(r'\+(\d+)(?:,(\d+))?', line)
            if not m: continue
            start, n = int(m.group(1)), int(m.group(2) if m.group(2) is not None else 1)
            if cut: n = max(0, min(n, cut - start))
            if n: per[cur] = per.get(cur, 0) + n; total += n
    return total, per

def _joil_progress_at(repo, rev, files=None):
    if files is None:
        rc, out, _ = run(['git', '-C', repo, 'ls-tree', '--name-only', rev, JOIL_PROGRESS_DIR + '/'], timeout=15)
        files = out.split() if rc == 0 else []
    tasks = {}
    for f in files:
        rc, js, _ = run(['git', '-C', repo, 'show', f'{rev}:{f}'], timeout=15)
        try: tasks.update(json.loads(js).get('tasks') or {})
        except Exception: pass
    return tasks

def joil_done_transitions(repo, ref='origin/main', max_commits=200):
    """First-parent walk of progress-touching commits -> {task: {commit, prev_status, blocked_note}}."""
    rc, out, _ = run(['git', '-C', repo, 'log', '--first-parent', '--format=%H', '-n', str(max_commits), ref, '--', JOIL_PROGRESS_DIR], timeout=30)
    commits = list(reversed(out.split())) if rc == 0 else []
    trans, blocked_note = {}, {}
    for c in commits:
        rc, par, _ = run(['git', '-C', repo, 'rev-parse', c + '^1'], timeout=10)
        par = par.strip() if rc == 0 else None
        if par:
            rc, ch, _ = run(['git', '-C', repo, 'diff', '--name-only', par, c, '--', JOIL_PROGRESS_DIR], timeout=15)
            files = ch.split()
            before = _joil_progress_at(repo, par, files)
        else:
            files, before = None, {}
        after = _joil_progress_at(repo, c, files)
        for k, v in list(before.items()) + list(after.items()):
            if (v or {}).get('status') == 'blocked': blocked_note[k] = (v or {}).get('note') or ''
        for k, v in after.items():
            prev = (before.get(k) or {}).get('status')
            if (v or {}).get('status') == 'done' and prev != 'done':
                trans[k] = {'commit': c, 'parent': par, 'prev_status': prev, 'prev_note': (before.get(k) or {}).get('note') or ''}
    for k in trans: trans[k]['blocked_note'] = blocked_note.get(k, '')
    return trans

def _joil_receipts_at(repo, ref):
    rc, out, _ = run(['git', '-C', repo, 'ls-tree', '-r', '--name-only', ref, 'docs/receipts'], timeout=15)
    recs = {}
    for f in (out.split() if rc == 0 else []):
        if not f.endswith('.json'): continue
        rc2, js, _ = run(['git', '-C', repo, 'show', f'{ref}:{f}'], timeout=15)
        try:
            d = json.loads(js)
            if isinstance(d, dict) and d.get('task_ids'): recs[f] = d
        except Exception: pass
    return recs

def _joil_dispositions_at(repo, ref):
    rc, out, _ = run(['git', '-C', repo, 'ls-tree', '-r', '--name-only', ref, 'docs/release'], timeout=15)
    by_task = {}
    for f in (out.split() if rc == 0 else []):
        if 'DISPOSITIONS' not in f.upper() or not f.endswith('.json'): continue
        rc2, js, _ = run(['git', '-C', repo, 'show', f'{ref}:{f}'], timeout=15)
        try: rows = json.loads(js).get('decisions') or []
        except Exception: rows = []
        for r in rows:
            for t in re.findall(r'V5-\d\d-\d\d', str(r.get('task_id', '')) + ' ' + ' '.join(r.get('task_ids') or [])):
                by_task.setdefault(t, []).append(r)
    return by_task

def _joil_log_is_summary(path):
    try:
        with open(path, errors='replace') as fh: txt = fh.read()
    except Exception:
        return None
    return not re.search(r'^test \S+ \.\.\. ', txt, re.M) and not re.search(r'^(test_\w+|\w+ \(\S+\)) \.\.\. ', txt, re.M)

def joil_flip_waived(mine):
    """A decision-unblocked done is cleared only by its own receipt (<= 2 task_ids) that says how the external
    blocker was satisfied (decision_resolution) and carries >= 1 passing test command with a full per-test log."""
    for f, d in mine:
        if len(d.get('task_ids', [])) > 2 or not str(d.get('decision_resolution') or '').strip(): continue
        for cmd in d.get('commands') or []:
            s = cmd.get('command') or ' '.join(cmd.get('argv') or [])
            if ('cargo test' in s or 'unittest' in s or 'pytest' in s) and cmd.get('exit_code') == 0 and _joil_log_is_summary(cmd.get('log_path') or '') is False:
                return True
    return False

def audit_joil_flips(repo, ref='origin/main', emit=True):
    progress = _joil_progress_at(repo, ref)
    done = sorted(k for k, v in progress.items() if (v or {}).get('status') == 'done')
    trans = joil_done_transitions(repo, ref)
    recs = _joil_receipts_at(repo, ref)
    disp = _joil_dispositions_at(repo, ref)
    flags = {k: [] for k in done}
    prod_cache = {}
    by_commit = {}
    for k in done:
        t = trans.get(k)
        if t: by_commit.setdefault(t['commit'], []).append(k)
    for c, ks in by_commit.items():
        par = trans[ks[0]]['parent']
        prod_cache[c] = joil_prod_added_lines(repo, par, c)[0] if par else 10 ** 9
    for k in done:
        t = trans.get(k)
        mine = [(f, d) for f, d in recs.items() if k in d.get('task_ids', [])]
        if t and t['parent'] and JOIL_EXTERNAL_CAUSE.search((t['blocked_note'] or '') + ' ' + (t.get('prev_note') or '')):
            per_task = prod_cache[t['commit']] / max(1, len(by_commit[t['commit']]))
            if per_task < JOIL_FLIP_MIN_PROD_LINES and not joil_flip_waived(mine): flags[k].append('JOIL_DONE_STATUS_FLIP')
        entry = json.dumps(progress.get(k) or {})
        for r in disp.get(k, []):
            mode = str(r.get('disposition_mode') or r.get('disposition') or '')
            if JOIL_OPEN_RX.search(str(r.get('summary', '')) + ' ' + str(r.get('rationale', ''))):
                flags[k].append('JOIL_DONE_DECISION_SAYS_OPEN')
            if re.search(r'substitute:|rescope:', mode) and not re.search(r'substitute:|rescope:', entry):
                flags[k].append('JOIL_SUBSTITUTE_UNLABELLED')
        if mine and not any(len(d.get('task_ids', [])) <= 2 for f, d in mine):
            flags[k].append('JOIL_DONE_SHARED_RECEIPT')
        logs = []
        for f, d in mine:
            for cmd in d.get('commands') or []:
                s = cmd.get('command') or ' '.join(cmd.get('argv') or [])
                if ('cargo test' in s or 'unittest' in s or 'pytest' in s) and cmd.get('log_path'):
                    logs.append(_joil_log_is_summary(cmd['log_path']))
        known = [x for x in logs if x is not None]
        if known and all(known): flags[k].append('JOIL_LOG_SUMMARY_ONLY')
        flags[k] = sorted(set(flags[k]))
    red_codes = {'JOIL_DONE_STATUS_FLIP', 'JOIL_DONE_DECISION_SAYS_OPEN', 'JOIL_SUBSTITUTE_UNLABELLED'}
    red = sorted(k for k, v in flags.items() if red_codes & set(v))
    if emit:
        agg = {}
        for k, v in flags.items():
            for code in v: agg.setdefault(code, []).append(k)
        for code, ks in sorted(agg.items()):
            sev = 'red' if code in red_codes else 'amber'
            F(sev, 'joil', code, f'{len(ks)} done-on-main task(s): {" ".join(sorted(ks)[:40])}{" ..." if len(ks) > 40 else ""}', repo + ' ' + ref)
    return {'done_on_main': len(done), 'done_red_flagged': len(red), 'done_unflagged_on_main': len(done) - len(red),
            'red_flagged_tasks': red, 'flags': {k: v for k, v in flags.items() if v}}

# ---------------- progress accountability (Jepson 2026-10-18 04:59 MDT) ----------------
# "The job of the super_captain is to hold captains accountable if they haven't made serious progress since the latest check-in."
BLOCK_CAUSES = [
    ('final_gate_or_ci', r'final (exact-head )?gate|exact-head|head CI|eight-suite|final source/head|full gate'),
    ('defect_control', r'defect closure|defect control|\bD-\d\d'),
    ('owner_decision', r'unsigned|unapproved|owner review|owner acceptance|disclosure|approval|spending|authority'),
    ('external_measured_data', r'measur|metrology|specimen|fatigue data|wet loads|calibration data|accession|labels unavailable|carrier-field|manufacturing/load|as-built|scientific observations|interface dataset|process/material inputs'),
    ('native_compute_or_lease_scope', r'native|GPU|CFD|solver|Blender|render|campaign|parity|benchmark|mutation|not run|have not run|unexecuted|not executed|frozen corpus|material partition|drills|browser|lease'),
    ('upstream_admission_dependency', r'admission|admitted|observer|registry|DecisionProfile|accepted|qualif|preregistration|statistical|dependency'),
]
BOILERPLATE = re.compile(r'^(Integrated software checkpoint \w+; full gate/CI and acceptance pending\.|Software integrated with positive/negative controls; acceptance open:)\s*', re.I)
def classify_block(note):
    note = BOILERPLATE.sub('', note or '')
    for name, rx in BLOCK_CAUSES:
        if re.search(rx, note or '', re.I): return name
    return 'other_or_unstated'

BLOCKED_SHARE_RED = 0.25      # red when more than this share of a project's tasks are blocked
WINDOW_MIN = 25               # compare against the newest snapshot at least this old (one check-in)

def merged_since(project, since_iso):
    rc, out, _ = run([GH, 'pr', 'list', '-R', GHREPO[project], '--state', 'merged', '--json', 'number,mergedAt', '-L', '20'], timeout=30)
    try: prs = json.loads(out)
    except Exception: return None
    return [p['number'] for p in prs if (p.get('mergedAt') or '') >= since_iso.replace('+00:00', 'Z')[:19]]

def progress_accountability(report, state, now, no_gh=False):
    hist = state.setdefault('progress', {})
    if not hist:   # seed done-history from audit.jsonl so the first run already has a baseline
        try:
            for l in open(os.path.join(os.path.dirname(state.get('_path', OUT_DEFAULT + '/x')), 'audit.jsonl')):
                d = json.loads(l)
                for p, v in (d.get('done') or {}).items():
                    if isinstance(v, int): hist.setdefault(p, []).append({'utc': d['utc'], 'done': v, 'blocked': None})
        except Exception: pass
    cutoff = (now - datetime.timedelta(hours=8)).isoformat(timespec='seconds')
    out = {}
    for proj, r in report['projects'].items():
        done, total = r.get('done_recomputed'), r.get('total')
        blocked = r.get('blocked')
        if blocked is None and isinstance(r.get('status_counts'), dict): blocked = r['status_counts'].get('blocked')
        h = [x for x in hist.get(proj, []) if x['utc'] >= cutoff]
        base = None
        for x in h:
            if x['utc'] <= (now - datetime.timedelta(minutes=WINDOW_MIN)).isoformat(timespec='seconds'): base = x
        last_up = None   # last time done increased
        for a, b in zip(h, h[1:]):
            if isinstance(a.get('done'), int) and isinstance(b.get('done'), int) and b['done'] > a['done']: last_up = b['utc']
        info = {'done': done, 'blocked': blocked, 'total': total, 'baseline_utc': base and base['utc'],
                'baseline_done': base and base.get('done'), 'baseline_blocked': base and base.get('blocked'), 'last_done_increase_utc': last_up}
        if base and isinstance(done, int):
            merges = None if no_gh else merged_since(proj, base['utc'])
            info['merged_prs_since_baseline'] = merges
            if done <= (base.get('done') or 0) and not merges:
                mins = int((now - datetime.datetime.fromisoformat(base['utc'])).total_seconds() / 60)
                stuck_since = last_up or (h[0]['utc'] if h else base['utc'])
                F('red', proj, 'NO_PROGRESS', f'done {base.get("done")} -> {done} over the last {mins} min and no PR merged to main; done has not increased since {stuck_since}. '
                  f'super_captain: demand a concrete unblock plan + ETA from {CAPTAINS.get(proj, proj)} this check, decide reversible blockers yourself as owner, escalate only irreversible ones to Jepson')
            if isinstance(blocked, int) and isinstance(base.get('blocked'), int) and blocked > base['blocked']:
                F('red', proj, 'BLOCKED_ROSE', f'blocked {base["blocked"]} -> {blocked} since {base["utc"]}; every new blocker needs an owner, a cause and an unblock action this check')
        if isinstance(blocked, int) and total and blocked / total > BLOCKED_SHARE_RED:
            groups = collections.Counter(classify_block(n) for n in (r.get('blocked_reasons') or {}).values())
            g = ', '.join(f'{k} {v}' for k, v in groups.most_common()) if groups else 'reasons not machine-readable; captain must list them'
            info['blocked_by_cause'] = dict(groups)
            F('red', proj, 'BLOCKED_SHARE', f'{blocked}/{total} tasks ({100 * blocked // total}%) are blocked (> {int(BLOCKED_SHARE_RED * 100)}%). By cause (keyword grouping): {g}. '
              f'Captain owes a per-cause unblock plan; super_captain decides reversible causes (lease/compute scope, caps, lane launches, dependency order) as owner now')
        if proj == 'joil' and isinstance(r.get('done_on_main'), int):
            info['done_on_main'] = r['done_on_main']
        h.append({'utc': report['utc'], 'done': done, 'blocked': blocked})
        hist[proj] = h[-200:]
        out[proj] = info
    report['progress'] = out

P1_PROJECTS = ('jopedime', 'admiralty')   # Jepson 6:24 AM MDT Oct 18: JopeDime GPU testing + Admiralty come first
def stuck_queued_checks(roll, now, mins=30):
    """Checks still QUEUED/PENDING/WAITING (no runner picked them up) for >= mins. Returns [(name, age_min)]."""
    out = []
    for c in roll or []:
        if c.get('conclusion'): continue
        st = str(c.get('status') or c.get('state') or '').upper()
        if st not in ('QUEUED', 'PENDING', 'WAITING', 'EXPECTED', 'REQUESTED'): continue
        t = c.get('startedAt') or c.get('createdAt')
        try: age = (now - datetime.datetime.fromisoformat(str(t).replace('Z', '+00:00'))).total_seconds() / 60
        except Exception: continue
        if age >= mins: out.append((c.get('name') or c.get('context') or '?', round(age)))
    return out

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
        stuck = stuck_queued_checks(roll, datetime.datetime.now(datetime.timezone.utc))
        if stuck and not p['isDraft']:
            F('red' if project in P1_PROJECTS else 'amber', project, 'PR_CI_QUEUED_STUCK',
              f'PR #{p["number"]} @ {p["headRefOid"][:9]}: {len(stuck)} check(s) queued with no runner for {max(a for _, a in stuck)} min ({", ".join(f"{n} {a}m" for n, a in stuck[:6])}). '
              'A queued check is not progress: find out why no runner picks it up (runs-on label vs available runners, GitHub-hosted minutes/billing for private repos) and fix it or decide the merge path this check; do not keep re-reporting an ETA',
              f'gh pr checks {p["number"]} -R {GHREPO[project]}')
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

# ---------------- watchdog: misses that let workers idle (GB 2026-10-18 09:35 MDT, Jepson: "why wasn't this caught automatically") ----------------
# Root causes fixed here: (1) only the 5 captain screens + LANE lines with an explicit state=running were watched, so workers super_captain
# itself manages (jopedime_flywheel 'parked', jopedime_agy on a positional '| active |' LANE line) were invisible; (2) idle detection needed
# the Claude-Code '? for shortcuts' footer (Claude in auto mode / Codex never show it) and was amber at 25 min; (3) ETAs/deadlines
# super_captain wrote into its own status were never checked; (4) 're-drove X' claims were never checked against X's terminal input;
# (5) no per-project last-push age; (6) super_captain's free-text state/workers_idle were never compared with the audit.
# Every finding below carries must_act=True: super_captain's tick treats it as must-act-this-tick.
IDLE_RED_MIN = 20          # managed screen idle (not busy) and unchanged this long -> red
NO_PUSH_RED_MIN = 30       # project with open work and no push to any branch this long -> red
ETA_GRACE_MIN = 5          # an ETA in super_captain_status.json this far in the past -> red (owed deliverable overdue)
REDRIVE_GRACE_MIN = 15     # a 're-drove X' claim needs terminal input on X no earlier than this before the claim
BUSY_RE = re.compile(r'esc to (cancel|interrupt)', re.I)
# A count/menu entry or prose about a monitor is not current worker activity.
# Recognize only a positive, explicitly running Claude duration/status footer.
# These remain screen hints, never durable progress or task acceptance evidence.
CLAUDE_STATUS_PREFIX = (
    r'^[ \t]*[A-Za-z]+(?:[ \t]+[A-Za-z]+){0,2}[ \t]+for[ \t]+'
    r'[0-9]+(?:\.[0-9]+)?[hms](?:[ \t]*[0-9]+(?:\.[0-9]+)?[hms])*'
    r'[ \t]*·[ \t]*done[ \t]+[0-9]{1,2}:[0-9]{2}(?:[ \t]*[AP]M)?')
BACKGROUND_STATUS_RE = re.compile(CLAUDE_STATUS_PREFIX + r'(?:[ \t]*·[^\n]*)?[ \t]*$', re.I)
BACKGROUND_BUSY_RE = re.compile(
    CLAUDE_STATUS_PREFIX +
    r'[ \t]*·[ \t]*[1-9][0-9]*[ \t]+(?:monitors?|background tasks?|shells?)'
    r'[ \t]+(?:still[ \t]+)?running[ \t]*$', re.I)
SCREEN_ID_RE = re.compile(r'\b(\d{3,8})\.([A-Za-z][\w-]+)\b')
NOT_WORKER = re.compile(r'^(sc_|disk_|captains_dash|super_captain)')
ETA_LIVE_KEYS = ('captains', 'overall', 'summary', 'needs_jepson', 'escalations', 'issues', 'actions')
ETA_RE = re.compile(r'\b(?:ETA|due|by)\s*`?\s*(\d{1,2}):(\d{2})\s*(Z|UTC|AM MDT|PM MDT|MDT)\b`?', re.I)

def MA(sev, scope, code, text, evidence=None):
    F(sev, scope, code, text, evidence); findings[-1]['must_act'] = True

def tail_busy(text):
    raw_lines = text.rstrip('\n').split('\n')
    recent_indexes = [i for i, line in enumerate(raw_lines) if line.strip()][-12:]
    if BUSY_RE.search('\n'.join(raw_lines[i] for i in recent_indexes)):
        return True
    recent_start = recent_indexes[0] if recent_indexes else len(raw_lines)
    # Do not interpret quoted documentation or fenced examples as background UI.
    fence = None; visible = []
    for index, line in enumerate(raw_lines):
        stripped = line.lstrip()
        marker = re.match(r'(`{3,}|~{3,})', stripped)
        if marker:
            kind = marker.group(1)[0]
            if fence is None: fence = kind
            elif fence == kind: fence = None
            continue
        if index >= recent_start and fence is None and line.strip() and not stripped.startswith(('>', '`', '"', "'")):
            visible.append(line)
    for line in reversed(visible):
        # A newer completed status or explicit zero-count footer supersedes the
        # old running footer still visible in terminal scrollback.
        if re.search(r'\bauto mode on\b[^\n]*·[ \t]*0[ \t]+(?:monitors?|background tasks?|shells?)\b', line, re.I):
            return False
        if BACKGROUND_STATUS_RE.fullmatch(line):
            return bool(BACKGROUND_BUSY_RE.fullmatch(line))
    return False

def tail_hash(text):
    lines = [l.rstrip() for l in text.rstrip('\n').split('\n') if l.strip()][-25:]
    return hashlib.sha256('\n'.join(lines).encode()).hexdigest()[:16]

def pty_last_input(pid):
    """atime of the screen child's pty slave = last time the program read input (a pasted nudge + CR updates it)."""
    for k in descendants(pid)[:1] or []:
        try:
            tty = os.readlink(f'/proc/{k}/fd/0')
            if tty.startswith('/dev/pts/'): return datetime.datetime.fromtimestamp(os.stat(tty).st_atime, datetime.timezone.utc)
        except OSError: pass
    return None

def managed_screens(scs, lanes_by_proj):
    """screen name -> project, for every worker super_captain or a captain says it manages."""
    out = {}
    for proj, blk in ((scs or {}).get('captains') or {}).items():
        for m in SCREEN_ID_RE.finditer(json.dumps(blk)): out.setdefault(m.group(2), proj)
    for proj, lanes in (lanes_by_proj or {}).items():
        for n in lanes: out.setdefault(n, proj)
    for proj, name in CAPTAINS.items(): out[name] = proj
    return {n: p for n, p in out.items() if not NOT_WORKER.match(n)}

def lane_screens_any_format(project):
    """Latest LANE line per screen in any format (key=value or positional '| active |'); drops closed/done/retired ones."""
    txt = tail_text(LOGS[project], 600000); last = {}
    for raw in txt.split('\n'):
        if 'LANE' not in raw: continue
        l = raw.replace('`', '').strip().lstrip('-* ').strip()
        if not l.startswith('LANE'): continue
        for m in SCREEN_ID_RE.finditer(l): last[m.group(2)] = l
    closed = re.compile(r'(\||=|\s)(closed|done|retired|exited|killed|finished|merged)(\s|\||$)', re.I)
    return {n: l for n, l in last.items() if not closed.search(l)}

def idle_watch(scr, managed, prev, newh, now, hc=None):
    hc = hc or hardcopy; res = {}; idle = {}
    for n, proj in sorted(managed.items()):
        if n not in scr: continue
        txt = hc(n, f'/tmp/sc_watch_hc_{n}.txt')
        if not txt: continue
        h = tail_hash(txt); key = 'watch:' + n; p = prev.get(key, {})
        since = p.get('since') if p.get('hash') == h else now.isoformat(timespec='seconds')
        newh[key] = {'hash': h, 'since': since}
        unchanged = (now - datetime.datetime.fromisoformat(since)).total_seconds() / 60
        busy = tail_busy(txt)
        li = pty_last_input(scr[n]) if isinstance(scr.get(n), int) else None
        res[n] = {'project': proj, 'busy': busy, 'unchanged_min': round(unchanged),
                  'last_input_utc': li and li.isoformat(timespec='seconds')}
        if not busy and unchanged >= IDLE_RED_MIN:
            idle.setdefault(proj, []).append(f'{n} {round(unchanged)}m' + (f' (last input {li.strftime("%H:%MZ")})' if li else ''))
        elif busy and unchanged >= IDLE_RED_MIN:
            MA('amber', proj, 'MANAGED_SCREEN_FROZEN', f'{n} shows busy but screen unchanged {round(unchanged)} min (hung tool call?)', f'/tmp/sc_watch_hc_{n}.txt')
    for proj, lst in idle.items():
        MA('red', proj, 'MANAGED_SCREEN_IDLE', f'{len(lst)} managed screen(s) idle (no "esc to cancel/interrupt") and unchanged >= {IDLE_RED_MIN} min: {"; ".join(lst[:12])}. '
           'Must act this tick: give each its next concrete task (file pointer + CR + alt+m) and confirm from a fresh hardcopy that it is busy, '
           'or close it with a LANE ... | closed line (parked is not a state for a worker you are counting on)', '/tmp/sc_watch_hc_<screen>.txt')
    return res

def eta_overdue(scs, now, grace=ETA_GRACE_MIN):
    """ETAs/deadlines super_captain wrote into its own status that are already past = owed deliverables overdue."""
    hits = []
    def walk(o, path):
        if path and (path[0] not in ETA_LIVE_KEYS or any(re.search(r'resolved|history|decision|done_log|archive', x, re.I) for x in path)): return
        if isinstance(o, dict):
            for k, v in o.items(): walk(v, path + [str(k)])
        elif isinstance(o, list):
            for i, v in enumerate(o): walk(v, path + [str(i)])
        elif isinstance(o, str):
            for m in ETA_RE.finditer(o):
                hh, mm, z = int(m.group(1)), int(m.group(2)), m.group(3).upper()
                if 'PM' in z and hh < 12: hh += 12
                if 'AM' in z and hh == 12: hh = 0
                if 'MDT' in z: hh = (hh + 6) % 24
                if hh > 23 or mm > 59: continue
                t = now.replace(hour=hh, minute=mm, second=0, microsecond=0)
                if t - now > datetime.timedelta(hours=12): t -= datetime.timedelta(days=1)
                late = (now - t).total_seconds() / 60
                if re.search(r'\b(MERGED|COMPLETED|CLOSED|CLEARED|DONE)\b', o[max(0, m.start() - 60):m.start()]): continue
                if late >= grace and late < 18 * 60:
                    hits.append((path[1] if len(path) > 1 and path[0] == 'captains' else path[0], round(late), o[max(0, m.start() - 140):m.end()].replace('\n', ' ')))
    walk(scs or {}, [])
    return hits

def check_eta_overdue(scs, now):
    by = collections.OrderedDict()
    for scope, late, ctx in eta_overdue(scs, now):
        sc_ = scope if scope in CAPTAINS else 'super_captain'
        by.setdefault(sc_, {})
        by[sc_].setdefault(ctx[-110:], late)
    for sc_, items in by.items():
        top = sorted(items.items(), key=lambda kv: -kv[1])
        MA('red', sc_, 'OWED_ETA_OVERDUE', f'{len(top)} ETA(s) in super_captain_status.json are already past (up to {top[0][1]} min): '
           + ' | '.join(f'"...{c}" ({l}m late)' for c, l in top[:3])
           + '. Must act this tick: for each, verify the deliverable exists (file/commit/PR); if not, ask the owner screen why, re-drive it and record a NEW ETA; never carry a passed ETA forward',
           KICK + '/super_captain_status.json')

def redrive_claims(log_text, now, window_min=60):
    """[(claim_utc, screen_name, snippet)] for 're-drove/re-driven X' claims in super_captain_LOG.md blocks newer than window_min."""
    out = []; hdr = re.compile(r'^##\s.*?(\d{4}-\d\d-\d\dT\d\d:\d\d(?::\d\d)?Z)', re.M)
    marks = list(hdr.finditer(log_text))
    for i, m in enumerate(marks):
        t = parse_utc(m.group(1))
        if not t or (now - t).total_seconds() > window_min * 60: continue
        body = log_text[m.end(): marks[i + 1].start() if i + 1 < len(marks) else len(log_text)]
        for line in body.split('\n'):
            for sent in re.split(r'(?<=[.;])\s', line):
                if not re.search(r'\bre-?(dr(o|i)ve\w*|nudged|kicked)', sent, re.I): continue
                for s in SCREEN_ID_RE.finditer(sent): out.append((t, s.group(2), sent.strip()[:200]))
    return out

def check_redrive_delivered(log_text, scr, now, last_input=None):
    last_input = last_input or (lambda n: pty_last_input(scr[n]) if isinstance(scr.get(n), int) else None)
    done = set()
    for t, n, snip in redrive_claims(log_text, now):
        if n in done or n not in scr: continue
        li = last_input(n)
        if li is None: continue
        if li < t - datetime.timedelta(minutes=REDRIVE_GRACE_MIN):
            done.add(n)
            MA('red', 'super_captain', 'REDRIVE_NOT_DELIVERED', f'super_captain_LOG {t.strftime("%H:%MZ")} claims it re-drove {n}, but {n} has had no terminal input since {li.strftime("%H:%MZ")} '
               f'(pty atime): the nudge was never submitted. Claim: "{snip}". Must act this tick: paste the pointer, send CR, then alt+m, and confirm a busy hardcopy before logging "re-driven"')

def last_push(project):
    rc, out, _ = run([GH, 'api', f'repos/{GHREPO[project]}/activity?per_page=10'], timeout=30)
    try: acts = json.loads(out)
    except Exception: return None
    ts = [parse_utc(a.get('timestamp')) for a in acts if a.get('activity_type') in ('push', 'force_push', 'pr_merge', 'merge_queue_merge', 'branch_creation')]
    ts = [t for t in ts if t]
    return max(ts) if ts else None

def check_no_push(report, now, push_fn=None):
    push_fn = push_fn or last_push
    for proj, r in report['projects'].items():
        done, total = r.get('done_recomputed'), r.get('total')
        open_work = not (isinstance(done, int) and isinstance(total, int) and done >= total) or bool(r.get('open_prs'))
        if not open_work or proj not in GHREPO: continue
        t = push_fn(proj)
        if t is None: continue
        age = (now - t).total_seconds() / 60; r['last_push_utc'] = t.isoformat(timespec='seconds')
        if age >= NO_PUSH_RED_MIN:
            MA('red', proj, 'NO_PUSH_30M', f'{GHREPO[proj]} has open work ({done}/{total}) but nothing pushed to any branch for {round(age)} min (last {t.strftime("%H:%MZ")}). '
               f'Must act this tick: find which worker owes a push and why it has not pushed; every worker pushes its branch every 30 min')

STATE_COUNT_RE = re.compile(r'\b(\d{1,4})\s*/\s*(\d{1,4})\b(?=[^.;|]{0,30}\b(done|accepted|integrated|complete))', re.I)
def check_sc_claims(report, scs, watch):
    caps = (scs or {}).get('captains') or {}
    for proj, r in report['projects'].items():
        blk = caps.get(proj) or {}; done, total = r.get('done_recomputed'), r.get('total')
        state = str(blk.get('state') or '')
        if isinstance(done, int) and isinstance(total, int):
            bad = [m.group(0) for m in STATE_COUNT_RE.finditer(state) if int(m.group(2)) == total and int(m.group(1)) > done
                   and not (r.get('packets_accepted') is not None and int(m.group(1)) <= r['packets_accepted'])]
            if bad: MA('red', proj, 'SC_STATE_TEXT_OVERCLAIM', f'super_captain_status.json captains.{proj}.state still says "{bad[0]} ..." while the audit gives {done}/{total}. '
                       'The headline text must use the audit number', KICK + '/super_captain_status.json')
        idle = sorted(n for n, w in (watch or {}).items() if w['project'] == proj and not w['busy'] and w['unchanged_min'] >= IDLE_RED_MIN)
        if idle and blk.get('workers_idle') == 0:
            MA('red', proj, 'SC_IDLE_CLAIM_FALSE', f'super_captain_status.json says workers_idle=0 for {proj} but {", ".join(idle)} idle >= {IDLE_RED_MIN} min. Count idle workers from this audit, not from memory',
               KICK + '/super_captain_status.json')

def watchdog(report, scr, prev, newh, now, no_gh=False):
    scs = rj(KICK + '/super_captain_status.json') or {}
    managed = managed_screens(scs, {p: lane_screens_any_format(p) for p in CAPTAINS})
    watch = idle_watch(scr, managed, prev, newh, now)
    report['watch'] = watch
    check_eta_overdue(scs, now)
    check_redrive_delivered(tail_text(KICK + '/super_captain_LOG.md', 300000), scr, now)
    if not no_gh: check_no_push(report, now)
    check_sc_claims(report, scs, watch)

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
    report['projects']['admiralty'] = audit_admiralty(scr, prev, newh, now)
    if not a.no_gh:
        for proj in CAPTAINS:
            report['projects'][proj]['open_prs'] = open_prs(proj)
            report['projects'][proj]['main_ci'] = main_ci(proj)
    state['_path'] = a.out + '/state.json'
    progress_accountability(report, state, now, a.no_gh)
    state.pop('_path', None)
    watchdog(report, scr, prev, newh, now, a.no_gh)   # must-act detectors (idle workers, overdue ETAs, undelivered re-drives, no push, false claims)

    # 4. super_captain's own last report vs recomputed numbers
    scs = rj(KICK + '/super_captain_status.json') or {}
    report['super_captain_status_utc'] = scs.get('updated_utc')
    for proj, r in report['projects'].items():
        claimed = ((scs.get('captains') or {}).get(proj) or {}).get('done')
        r['super_captain_reported_done'] = claimed
        cands = {r.get('done_recomputed'), r.get('packets_accepted')} - {None}
        if isinstance(claimed, int) and cands and claimed > max(cands):
            MA('red', proj, 'SC_REPORTED_OVERCLAIM', f'super_captain_status.json reports done={claimed} but the ledger gives {r.get("done_recomputed")}/{r.get("total")}' + (f' (packets {r.get("packets_accepted")}/{r.get("packets_total")})' if r.get('packets_total') else '') + ' - super_captain must take done counts from this audit, not from captain claims', KICK + '/super_captain_status.json')
    try:
        upd = datetime.datetime.fromisoformat(str(scs.get('updated_utc')).replace('Z', '+00:00'))
        if (now - upd).total_seconds() > 40 * 60: F('red', 'super_captain', 'SC_STALE', f'super_captain_status.json not updated for {int((now - upd).total_seconds() / 60)} min')
    except Exception:
        F('amber', 'super_captain', 'SC_STATUS_UNREADABLE', 'super_captain_status.json missing updated_utc')

    sev_rank = {'red': 0, 'amber': 1, 'info': 2}
    findings.sort(key=lambda f: (sev_rank.get(f['severity'], 3), f['scope'], f['code']))
    report['findings'] = findings
    report['summary'] = dict(collections.Counter(f['severity'] for f in findings))
    report['must_act'] = [f for f in findings if f.get('must_act')]
    report['summary']['must_act'] = len(report['must_act'])
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
    md += ['', '## Progress since last check-in (accountability)', '',
           '| Project | done now (baseline) | blocked now (baseline) | PRs merged since baseline | done last increased | baseline |', '|---|---|---|---|---|---|']
    for p, i in (report.get('progress') or {}).items():
        md.append(f'| {p} | {i.get("done")} ({i.get("baseline_done")}) | {i.get("blocked")} ({i.get("baseline_blocked")}) | {i.get("merged_prs_since_baseline")} | {i.get("last_done_increase_utc")} | {i.get("baseline_utc")} |'
                  + (f' done on origin/main: {i["done_on_main"]}' if 'done_on_main' in i else ''))
    md += ['', f'## MUST ACT THIS TICK ({len(report["must_act"])})', ''] + ([f'- **{f["severity"]}** `{f["scope"]}` {f["code"]}: {f["text"]}' for f in report['must_act']] or ['- none'])
    md += ['', f'## Findings ({report["summary"]})', '']
    md += [f'- **{f["severity"]}** `{f["scope"]}` {f["code"]}: {f["text"]}' + (f' (evidence: `{f["evidence"]}`)' if f.get('evidence') else '') for f in findings] or ['- none']
    open(a.out + '/latest.md', 'w').write('\n'.join(md) + '\n')
    if not a.quiet: print('\n'.join(md))
    return 0

if __name__ == '__main__':
    sys.exit(main())
