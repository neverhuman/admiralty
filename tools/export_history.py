#!/usr/bin/env python3
"""Export the captains dashboard's running history into a sanitized, git-friendly dataset.

Reads (read-only) the live runtime files of the captains dashboard on xbabe2 and writes
JSONL/CSV under <out>/ (default: <repo>/data/history). Append-style datasets are UNION-MERGED
with what is already in <out>, because several sources are rolling windows (history.jsonl is
downsampled after 6 h, disk history keeps 6 h, quota history 12 h). Re-running is idempotent.

  python3 tools/export_history.py [--out DIR] [--dash DIR] [--capd DIR] [--check]

Never exports raw prompts, screen hardcopies, credentials, or message ids. Absolute home paths
are rewritten to "~/". A secret-pattern scan runs over every output file; any hit aborts with
exit code 3 and nothing should be committed.
"""
import argparse, csv, datetime, glob, hashlib, io, json, os, re, sqlite3, sys, collections

HOME = os.path.expanduser('~')
REPO = os.path.dirname(os.path.dirname(os.path.realpath(__file__)))
PROJECTS = ['joil', 'dope', 'jopedime', 'jarambit']
HOSTS = ['xbabe1', 'xbabe2', 'xbabe3']
TIERS = ['luna', 'sol', 'astra', 'claude', 'agy', 'other']
SECRET_RES = [re.compile(p) for p in (
    r'gh[pousr]_[A-Za-z0-9]{20,}', r'github_pat_[A-Za-z0-9_]{20,}', r'\bsk-[A-Za-z0-9_\-]{20,}', r'\bAKIA[0-9A-Z]{16}\b',
    r'\bxox[abprs]-[A-Za-z0-9\-]{10,}', r'-----BEGIN [A-Z ]*PRIVATE KEY-----', r'\bAIza[0-9A-Za-z_\-]{35}\b',
    r'(?i)\b(api[_-]?key|secret|passwd|password|bearer)\b["\']?\s*[:=]\s*["\']?[A-Za-z0-9/+_\-]{16,}',
    r'\beyJ[A-Za-z0-9_\-]{20,}\.[A-Za-z0-9_\-]{20,}\.', r'(?i)authorization:\s*\S{12,}')]

def iso(ts): return datetime.datetime.fromtimestamp(float(ts), datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')
def day(ts): return iso(ts)[:10]
def parse_iso(s):
    try: return datetime.datetime.fromisoformat(str(s).replace('Z', '+00:00')).timestamp()
    except Exception: return None
def clean(v):
    """Recursively rewrite home paths; cap long strings."""
    if isinstance(v, str):
        v = v.replace(HOME + '/', '~/').replace(HOME, '~')
        return v[:400]
    if isinstance(v, dict): return {k: clean(x) for k, x in v.items()}
    if isinstance(v, list): return [clean(x) for x in v]
    return v
def rj(p, default=None):
    try:
        with open(p) as f: return json.load(f)
    except Exception: return default
def project_of(cwd):
    c = (cwd or '').lower()
    for key, proj in (('/joil', 'joil'), ('/dope', 'dope'), ('jopedime', 'jopedime'), ('jope-dime', 'jopedime'), ('/jarambit', 'jarambit')):
        if key in c: return proj
    return 'other'

# ---------------- merge helpers ----------------
def read_jsonl_dir(d, prefix):
    rows = []
    for f in sorted(glob.glob(os.path.join(d, prefix + '_*.jsonl'))):
        with open(f) as fh:
            for line in fh:
                line = line.strip()
                if line:
                    try: rows.append(json.loads(line))
                    except Exception: pass
    return rows
def write_jsonl_by_day(d, prefix, rows, tskey):
    os.makedirs(d, exist_ok=True)
    by = collections.defaultdict(list)
    for r in rows: by[r[tskey][:10]].append(r)
    for f in glob.glob(os.path.join(d, prefix + '_*.jsonl')):
        if os.path.basename(f)[len(prefix) + 1:-6] not in by: os.remove(f)
    for dd, rs in by.items():
        rs.sort(key=lambda r: (r[tskey], json.dumps(r, sort_keys=True)))
        tmp = os.path.join(d, '.%s_%s.tmp' % (prefix, dd))
        with open(tmp, 'w') as fh:
            for r in rs: fh.write(json.dumps(r, sort_keys=True, separators=(',', ':')) + '\n')
        os.replace(tmp, os.path.join(d, '%s_%s.jsonl' % (prefix, dd)))
    return len(rows)
def read_csv(p):
    try:
        with open(p, newline='') as f: return list(csv.DictReader(f))
    except Exception: return []
def write_csv(p, rows, fields):
    os.makedirs(os.path.dirname(p), exist_ok=True)
    tmp = p + '.tmp'
    with open(tmp, 'w', newline='') as f:
        w = csv.DictWriter(f, fieldnames=fields, extrasaction='ignore', lineterminator='\n'); w.writeheader()
        for r in rows: w.writerow({k: ('' if r.get(k) is None else r.get(k)) for k in fields})
    os.replace(tmp, p)
    return len(rows)

# ---------------- datasets ----------------
STAGES = ['done', 'on_main', 'in_pr', 'implemented', 'in_progress', 'blocked', 'not_started']
def ex_timeseries(dash, out):
    d = os.path.join(out, 'timeseries')
    have = {r['ts_utc']: r for r in read_jsonl_dir(d, 'history')}
    try:
        with open(os.path.join(dash, 'history.jsonl')) as fh:
            for line in fh:
                try: r = json.loads(line)
                except Exception: continue
                if not isinstance(r, dict) or 'ts' not in r: continue
                r = clean(r); r['ts_utc'] = iso(r['ts']); have[r['ts_utc']] = r
    except FileNotFoundError: pass
    rows = sorted(have.values(), key=lambda r: r['ts_utc'])
    write_jsonl_by_day(d, 'history', rows, 'ts_utc')
    # flat CSV
    fields = ['ts_utc', 'ts', 'cost_index', 'cost_basis', 'quota_pct', 'credits_h', 'luna_fail_total', 'luna_fp_rate'] + ['tier_' + t for t in TIERS]
    for p in PROJECTS: fields += [p + '_done', p + '_total', p + '_in_progress', p + '_ci', p + '_pr', p + '_hygiene'] + [p + '_stage_' + s for s in STAGES]
    for h in HOSTS: fields += [h + '_load_pct', h + '_disk_root_pct']
    fields += ['ty_tokens', 'ty_output', 'ty_credits', 'ty_integration_tokens', 'ty_main_tokens', 'ty_yield_integration_pct', 'ty_yield_main_pct', 'ty_coverage_pct']
    for p in PROJECTS + ['other']: fields += ['ty_%s_tokens' % p, 'ty_%s_integration_tokens' % p, 'ty_%s_main_tokens' % p, 'ty_%s_credits_per_done_todo' % p]
    flat = []
    for r in rows:
        f = {k: r.get(k) for k in ('ts_utc', 'ts', 'cost_index', 'cost_basis', 'quota_pct', 'credits_h', 'luna_fail_total', 'luna_fp_rate')}
        for t in TIERS: f['tier_' + t] = (r.get('tiers') or {}).get(t, 0)
        for p in PROJECTS:
            pp = (r.get('proj') or {}).get(p) or {}
            f.update({p + '_done': pp.get('done'), p + '_total': pp.get('total'), p + '_in_progress': pp.get('ip'), p + '_ci': pp.get('ci'), p + '_pr': pp.get('pr'),
                      p + '_hygiene': (r.get('hyg') or {}).get(p)})
            st = pp.get('st') or {}
            for s in STAGES: f[p + '_stage_' + s] = st.get(s)
        for h in HOSTS:
            hh = (r.get('hosts') or {}).get(h) or {}; f[h + '_load_pct'] = hh.get('load'); f[h + '_disk_root_pct'] = hh.get('disk')
        ty = r.get('ty') or {}
        f.update({'ty_tokens': ty.get('tok'), 'ty_output': ty.get('out'), 'ty_credits': ty.get('cr'), 'ty_integration_tokens': ty.get('int'), 'ty_main_tokens': ty.get('main'),
                  'ty_yield_integration_pct': ty.get('yi'), 'ty_yield_main_pct': ty.get('ym'), 'ty_coverage_pct': ty.get('cov')})
        for p, v in (ty.get('p') or {}).items():
            if p in PROJECTS + ['other'] and isinstance(v, list):
                v = (v + [None] * 4)[:4]
                f.update({'ty_%s_tokens' % p: v[0], 'ty_%s_integration_tokens' % p: v[1], 'ty_%s_main_tokens' % p: v[2], 'ty_%s_credits_per_done_todo' % p: v[3]})
        flat.append(f)
    write_csv(os.path.join(d, 'history.csv'), flat, fields)
    return len(rows), (rows[0]['ts_utc'] if rows else None), (rows[-1]['ts_utc'] if rows else None)

LEDGER_FIELDS = ['ts', 'project', 'task', 'screen', 'host', 'tier', 'model', 'effort', 'outcome', 'escalated_to', 'reason', 'tokens']
def norm_ledger(d):
    legacy = ('utc' in d and 'ts' not in d) or 'to_tier' in d
    ts = d.get('ts') or d.get('utc')
    esc = d.get('escalated_to') if d.get('escalated_to') is not None else d.get('to_tier')
    r = {'ts': ts, 'project': d.get('project'), 'task': d.get('task'), 'screen': d.get('screen'), 'host': d.get('host'), 'tier': d.get('tier'),
         'model': d.get('model'), 'effort': d.get('effort'), 'outcome': str(d.get('outcome') or '').lower() or None, 'escalated_to': esc,
         'reason': d.get('reason') or d.get('note'), 'tokens': d.get('tokens'), 'source_format': 'legacy_s9' if legacy else 'canonical'}
    for k in ('class', 'attempt', 'minutes'):
        if d.get(k) is not None: r[k] = d.get(k)
    return clean(r)
def ex_ledger(capd, out):
    d = os.path.join(out, 'ledger')
    have = {}
    def key(r): return hashlib.sha1(json.dumps(r, sort_keys=True).encode()).hexdigest()
    for r in read_jsonl_dir(d, 'model_usage'): have[key(r)] = r
    bad = 0
    try:
        with open(os.path.join(capd, 'model_usage.jsonl')) as fh:
            for line in fh:
                line = line.strip()
                if not line: continue
                try: raw = json.loads(line)
                except Exception: bad += 1; continue
                if not isinstance(raw, dict): bad += 1; continue
                r = norm_ledger(raw)
                if not r.get('ts') or parse_iso(r['ts']) is None: bad += 1; continue
                r['ts'] = iso(parse_iso(r['ts'])); have[key(r)] = r
    except FileNotFoundError: pass
    rows = sorted(have.values(), key=lambda r: r['ts'])
    write_jsonl_by_day(d, 'model_usage', rows, 'ts')
    write_csv(os.path.join(d, 'model_usage.csv'), rows, LEDGER_FIELDS + ['source_format', 'class', 'attempt', 'minutes'])
    return len(rows), bad, (rows[0]['ts'] if rows else None), (rows[-1]['ts'] if rows else None)

def ex_quota(dash, out):
    p = os.path.join(out, 'quota', 'codex_quota.csv')
    have = {r['ts_utc']: r for r in read_csv(p)}
    for x in rj(os.path.join(dash, '.quota_history.json'), []) or []:
        try: have[iso(x[0])] = {'ts_utc': iso(x[0]), 'ts': round(float(x[0]), 1), 'used_pct': x[1], 'credits_balance': x[2] if len(x) > 2 else None}
        except Exception: pass
    rows = sorted(have.values(), key=lambda r: r['ts_utc'])
    return write_csv(p, rows, ['ts_utc', 'ts', 'used_pct', 'credits_balance'])

def ex_resources(dash, out):
    p = os.path.join(out, 'resources', 'disk_used.csv')
    have = {(r['ts_utc'], r['host'], r['mount']): r for r in read_csv(p)}
    for k, series in (rj(os.path.join(dash, '.res_history.json'), {}) or {}).items():
        host, _, mount = k.partition(':')
        if host == 'xbabe0': continue
        for ts, used in series:
            r = {'ts_utc': iso(ts), 'host': host, 'mount': clean(mount), 'used_bytes': int(used)}
            have[(r['ts_utc'], host, r['mount'])] = r
    rows = sorted(have.values(), key=lambda r: (r['ts_utc'], r['host'], r['mount']))
    n1 = write_csv(p, rows, ['ts_utc', 'host', 'mount', 'used_bytes'])
    # host snapshot (one row per host per export run)
    g = rj(os.path.join(dash, 'global_resources.json'), {}) or {}
    p2 = os.path.join(out, 'resources', 'host_snapshots.csv')
    have2 = {(r['ts_utc'], r['host']): r for r in read_csv(p2)}
    ts = g.get('generated_utc')
    for h in g.get('hosts') or []:
        if not ts or h.get('host') == 'xbabe0': continue
        m = h.get('mem') or {}; root = next((x for x in h.get('disks', []) if x.get('mount') == '/'), {}) or {}
        r = {'ts_utc': ts, 'host': h.get('host'), 'reachable': h.get('reachable'), 'nproc': h.get('nproc'),
             'load1': (h.get('load') or [None])[0], 'mem_total_gb': m.get('total_gb'), 'mem_avail_gb': m.get('avail_gb'), 'swap_used_gb': m.get('swap_used_gb'),
             'disk_root_pct': root.get('pct'), 'disk_root_avail_bytes': root.get('avail'), 'gpus': len(h.get('gpus') or []),
             'gpu_util_avg': (round(sum(x.get('util', 0) or 0 for x in h['gpus']) / len(h['gpus']), 1) if h.get('gpus') else None)}
        have2[(ts, r['host'])] = r
    rows2 = sorted(have2.values(), key=lambda r: (r['ts_utc'], r['host']))
    write_csv(p2, rows2, ['ts_utc', 'host', 'reachable', 'nproc', 'load1', 'mem_total_gb', 'mem_avail_gb', 'swap_used_gb', 'disk_root_pct', 'disk_root_avail_bytes', 'gpus', 'gpu_util_avg'])
    return n1, len(rows2)

def ex_tokens(dash, out):
    db = os.path.join(dash, 'tokens.db')
    if not os.path.exists(db): return 0, 0
    c = sqlite3.connect('file:%s?mode=ro' % db, uri=True, timeout=10)
    p = os.path.join(out, 'tokens', 'codex_sessions.csv')
    have = {r['session']: r for r in read_csv(p)}
    for sid, tool, model, eff, cwd, host, start, end, inp, cac, outp, rea, cr in c.execute(
            'SELECT id,tool,model,effort,cwd,host,start,end,input,cached,output,reasoning,credits FROM sessions'):
        have[sid] = {'session': sid, 'tool': tool, 'model': model, 'effort': eff, 'project': project_of(cwd), 'host': host,
                     'start_utc': iso(start) if start else None, 'end_utc': iso(end) if end else None,
                     'input': inp, 'cached': cac, 'output': outp, 'reasoning': rea, 'credits_est': round(cr, 3) if cr is not None else None}
    rows = sorted(have.values(), key=lambda r: (r.get('start_utc') or '', r['session']))
    n1 = write_csv(p, rows, ['session', 'tool', 'model', 'effort', 'project', 'host', 'start_utc', 'end_utc', 'input', 'cached', 'output', 'reasoning', 'credits_est'])
    p2 = os.path.join(out, 'tokens', 'claude_hourly.csv')
    agg = {}
    for r in read_csv(p2): agg[(r['hour_utc'], r['project'], r['model'])] = r
    fresh = collections.defaultdict(lambda: [0, 0, 0, 0])
    for ts, model, cwd, inp, cac, outp in c.execute('SELECT ts,model,cwd,input,cached,output FROM claude_msgs'):
        if ts is None: continue
        k = (iso(ts)[:13] + ':00Z', project_of(cwd), model or 'unknown'); a = fresh[k]
        a[0] += 1; a[1] += inp or 0; a[2] += cac or 0; a[3] += outp or 0
    for k, a in fresh.items():   # db is cumulative: fresh hourly totals replace older exports of the same hour
        agg[k] = {'hour_utc': k[0], 'project': k[1], 'model': k[2], 'messages': a[0], 'input': a[1], 'cached': a[2], 'output': a[3]}
    rows2 = sorted(agg.values(), key=lambda r: (r['hour_utc'], r['project'], r['model']))
    write_csv(p2, rows2, ['hour_utc', 'project', 'model', 'messages', 'input', 'cached', 'output'])
    return n1, len(rows2)

def ex_token_yield(dash, out):
    ty = rj(os.path.join(dash, 'token_yield.json'))
    d = os.path.join(out, 'token_yield')
    rows = read_jsonl_dir(d, 'token_yield')
    if ty and ty.get('generated_utc'):
        snap = clean({'generated_utc': ty['generated_utc'], 'window_hours': ty.get('window_hours'), 'total': ty.get('total'), 'by_tier': ty.get('by_tier'),
                      'projects': ty.get('projects'), 'method': ty.get('method'), 'hosts_covered': ty.get('hosts_covered'), 'not_covered': ty.get('not_covered')})
        if not any(r.get('generated_utc') == snap['generated_utc'] for r in rows): rows.append(snap)
    write_jsonl_by_day(d, 'token_yield', rows, 'generated_utc')
    return len(rows)

def ex_snapshots(dash, capd, out):
    """Compact per-run project + hygiene snapshot (no free text, no hardcopies)."""
    st = rj(os.path.join(dash, 'status.json'), {}) or {}
    sc = rj(os.path.join(capd, 'super_captain_status.json'), {}) or {}
    d = os.path.join(out, 'snapshots')
    rows = read_jsonl_dir(d, 'projects')
    ts = st.get('generated_utc')
    if ts:
        ts = iso(parse_iso(ts))
        snap = {'ts_utc': ts, 'projects': {}}
        for k in PROJECTS:
            p = (st.get('projects') or {}).get(k) or {}
            pr = p.get('pr') or {}; ci = pr.get('ci') or {}
            h = ((sc.get('captains') or {}).get(k) or {}).get('hygiene') or {}
            snap['projects'][k] = {'done': p.get('done'), 'total': p.get('total'), 'counts': p.get('counts'), 'stages': p.get('stages'),
                                   'stage_meta': {x: (p.get('stage_meta') or {}).get(x) for x in ('pr_number', 'pr_open', 'main_sha', 'main_ci_green', 'accepted_in_pr', 'blocked_code_stage')} if p.get('stage_meta') else None, 'packets_accepted': p.get('packets_accepted'),
                                   'packets_total': p.get('packets_total'), 'blocked': len(p.get('blocked_ids') or []), 'workers': len(p.get('workers') or []),
                                   'pr_number': pr.get('number'), 'pr_head': (pr.get('head') or pr.get('head_sha') or '')[:12] or None, 'ci_state': ci.get('state'),
                                   'needs_jepson': len(p.get('needs_jepson') or []) if isinstance(p.get('needs_jepson'), list) else None,
                                   'hygiene': {x: h.get(x) for x in ('score', 'grade', 'stale_merged_worktrees', 'uncommitted_over_45m', 'lanes_behind', 'temp_growth_gb')} if h else None}
        snap = clean(snap)
        if not any(r.get('ts_utc') == ts for r in rows): rows.append(snap)
    write_jsonl_by_day(d, 'projects', rows, 'ts_utc')
    return len(rows)

def ex_rates(out):
    rd = os.path.join(REPO, 'captains', 'rates')
    idx = []
    for f in sorted(glob.glob(os.path.join(rd, '*_rates_*.json'))):
        b = open(f, 'rb').read(); d = json.loads(b)
        idx.append({'file': 'captains/rates/' + os.path.basename(f), 'vendor': d.get('vendor'), 'as_of': d.get('as_of'), 'unit': d.get('unit'),
                    'currency': d.get('currency'), 'baseline_model': d.get('baseline_model'), 'models': len(d.get('models') or []), 'sha256': hashlib.sha256(b).hexdigest()})
    p = os.path.join(out, 'rates_index.json'); os.makedirs(out, exist_ok=True)
    with open(p + '.tmp', 'w') as f: json.dump({'note': 'Rate files live in captains/rates/ (single source of truth). Vendors use different currencies; never compare across files.', 'files': idx}, f, indent=1)
    os.replace(p + '.tmp', p)
    return len(idx)

def secret_scan(out):
    hits = []
    for root, _, files in os.walk(out):
        for fn in files:
            p = os.path.join(root, fn)
            try: t = open(p, errors='replace').read()
            except Exception: continue
            for rx in SECRET_RES:
                m = rx.search(t)
                if m: hits.append((os.path.relpath(p, out), rx.pattern[:40], m.start()))
            if HOME in t: hits.append((os.path.relpath(p, out), 'absolute home path', t.index(HOME)))
    return hits

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--out', default=os.path.join(REPO, 'data', 'history'))
    ap.add_argument('--dash', default=os.environ.get('CDASH_RUNTIME', HOME + '/xbabe2_kicker/captains-dashboard'))
    ap.add_argument('--capd', default=HOME + '/xbabe2_kicker/captains-20261017')
    ap.add_argument('--check', action='store_true', help='only run the secret scan on --out')
    a = ap.parse_args()
    if not a.check:
        s = {}
        s['timeseries'] = ex_timeseries(a.dash, a.out)
        s['ledger'] = ex_ledger(a.capd, a.out)
        s['quota_rows'] = ex_quota(a.dash, a.out)
        s['resources'] = ex_resources(a.dash, a.out)
        s['tokens'] = ex_tokens(a.dash, a.out)
        s['token_yield_snapshots'] = ex_token_yield(a.dash, a.out)
        s['project_snapshots'] = ex_snapshots(a.dash, a.capd, a.out)
        s['rate_files'] = ex_rates(a.out)
        print(json.dumps(s))
    hits = secret_scan(a.out)
    if hits:
        for h in hits[:50]: print('SECRET-SCAN HIT', h, file=sys.stderr)
        sys.exit(3)
    print('secret scan: clean')

if __name__ == '__main__':
    main()
