#!/usr/bin/env python3
"""Read-only global resource sampler for xbabe1, xbabe2, xbabe3 (NEVER xbabe0).
Writes global_resources.json next to this file and prints a table.
Remote hosts are reached from xbabe2 with: ssh -o BatchMode=yes ubuntu@xbabe1 / ubuntu@xbabe3
(/etc/hosts names, default key ~/.ssh/id_ed25519). Note: the ~/.ssh/config alias "xbabe2" is a Mac; xbabe2 is local here.
Usage: python3 globalres.py [--quiet]"""
import json, os, subprocess, sys, time, datetime
BASE = os.environ.get('CDASH_RUNTIME', '/home/ubuntu/xbabe2_kicker/captains-dashboard')  # runtime data dir (status.json, history, tokens.db); code lives in admiralty/dashboard
OUT = os.path.join(BASE, 'global_resources.json')
HIST = os.path.join(BASE, '.res_history.json')
HOSTS = ['xbabe1', 'xbabe2', 'xbabe3']   # never xbabe0
assert 'xbabe0' not in HOSTS
PROBE = r'''
echo "@@HOST $(hostname)"; echo "@@NPROC $(nproc)"; echo "@@LOAD $(cut -d' ' -f1-3 /proc/loadavg)"
echo "@@MEM"; grep -E '^(MemTotal|MemAvailable|SwapTotal|SwapFree):' /proc/meminfo
echo "@@DF"; df -B1 -P -x tmpfs -x devtmpfs -x overlay -x squashfs -x efivarfs -x nfs -x nfs4 -x fuse.sshfs 2>/dev/null | awk 'NR>1 && $2>5000000000 {print $6, $2, $3, $4}'
echo "@@GPU"; nvidia-smi --query-gpu=index,name,utilization.gpu,memory.used,memory.total,temperature.gpu --format=csv,noheader,nounits 2>/dev/null
echo "@@DSTATE $(ps -eo stat= | grep -c '^D')"
echo "@@TOP"; ps -eo pid=,user=,rss=,pcpu=,etimes=,comm= --sort=-rss | head -6
echo "@@SCREENS"; screen -ls 2>/dev/null | awk '/\t[0-9]+\./{print $1}'
echo "@@END"
'''
def now(): return datetime.datetime.now(datetime.timezone.utc)
def iso(t=None): return (t or now()).strftime('%Y-%m-%dT%H:%M:%SZ')
def probe(h):
    cmd = ['bash', '-c', PROBE] if h == 'xbabe2' else ['ssh', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=6', '-o', 'ServerAliveInterval=5', 'ubuntu@' + h, PROBE]
    try:
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=25)
        return p.stdout, (p.stderr or '')[-200:]
    except Exception as e:
        return '', str(e)[:200]
def parse(h, out, err):
    r = {'host': h, 'reachable': '@@END' in out, 'error': None if '@@END' in out else (err.strip() or 'no output'),
         'disks': [], 'gpus': [], 'top_rss': [], 'screens': []}
    sec = None; mem = {}
    for line in out.splitlines():
        if line.startswith('@@'):
            parts = line[2:].split(' ', 1); sec = parts[0]; val = parts[1] if len(parts) > 1 else ''
            if sec == 'NPROC': r['nproc'] = int(val or 0)
            elif sec == 'LOAD': r['load'] = [float(x) for x in val.split()]
            elif sec == 'DSTATE': r['d_state_procs'] = int(val or 0)
            elif sec == 'HOST': r['hostname'] = val
            continue
        if sec == 'MEM':
            k, v = line.split(':'); mem[k] = int(v.split()[0]) * 1024
        elif sec == 'DF':
            m, size, used, avail = line.split()
            r['disks'].append({'mount': m, 'size': int(size), 'used': int(used), 'avail': int(avail),
                               'pct': round(100.0 * int(used) / max(1, int(used) + int(avail)), 1)})
        elif sec == 'GPU' and line.strip():
            f = [x.strip() for x in line.split(',')]
            try: r['gpus'].append({'idx': int(f[0]), 'name': f[1], 'util': float(f[2]), 'mem_used_mib': float(f[3]), 'mem_total_mib': float(f[4]), 'temp_c': float(f[5])})
            except Exception: pass
        elif sec == 'TOP' and line.strip():
            f = line.split(None, 5)
            r['top_rss'].append({'pid': int(f[0]), 'user': f[1], 'rss_gb': round(int(f[2]) / 1048576, 1), 'pcpu': float(f[3]), 'etime_s': int(f[4]), 'comm': f[5][:40]})
        elif sec == 'SCREENS' and line.strip():
            r['screens'].append(line.strip())
    if mem:
        r['mem'] = {'total_gb': round(mem.get('MemTotal', 0) / 2**30, 1), 'avail_gb': round(mem.get('MemAvailable', 0) / 2**30, 1),
                    'swap_total_gb': round(mem.get('SwapTotal', 0) / 2**30, 1), 'swap_used_gb': round((mem.get('SwapTotal', 0) - mem.get('SwapFree', 0)) / 2**30, 1)}
    return r
def main():
    quiet = '--quiet' in sys.argv
    t = now(); ts = t.timestamp()
    try: hist = json.load(open(HIST))
    except Exception: hist = {}
    hosts = []
    for h in HOSTS:
        out, err = probe(h); r = parse(h, out, err); w = []
        for d in r['disks']:
            key = h + ':' + d['mount']; hl = [x for x in hist.get(key, []) if ts - x[0] < 6 * 3600]
            hl.append([ts, d['used']]); hist[key] = hl
            base = next((x for x in hl if ts - x[0] <= 3 * 3600 and ts - x[0] >= 20 * 60), None)
            rate = None
            if base and ts - base[0] > 0: rate = (d['used'] - base[1]) / (ts - base[0]) * 3600  # bytes/hour
            d['growth_gb_per_h'] = round(rate / 1e9, 2) if rate is not None else None
            d['hours_to_full'] = round(d['avail'] / rate, 1) if rate and rate > 0 else None
            d['rate_window_min'] = round((ts - base[0]) / 60) if base else None
            lvl = 'ok'
            if d['pct'] >= 92 or (d['hours_to_full'] is not None and d['hours_to_full'] < 12): lvl = 'red'
            elif d['pct'] >= 85 or (d['hours_to_full'] is not None and d['hours_to_full'] < 24): lvl = 'amber'
            d['level'] = lvl
            if lvl != 'ok':
                w.append({'level': lvl, 'text': '%s %s %.1f%% used, %.0f GB free%s' % (h, d['mount'], d['pct'], d['avail'] / 1e9,
                          (', ~%.1f h to full at %.1f GB/h' % (d['hours_to_full'], d['growth_gb_per_h'])) if d['hours_to_full'] else '')})
        m = r.get('mem')
        if m:
            ap = 100.0 * m['avail_gb'] / max(1, m['total_gb'])
            m['avail_pct'] = round(ap, 1)
            m['level'] = 'red' if ap < 8 else 'amber' if ap < 15 else 'ok'
            if m['level'] != 'ok': w.append({'level': m['level'], 'text': '%s RAM low: %.1f GB available of %.0f GB' % (h, m['avail_gb'], m['total_gb'])})
            if m['swap_total_gb'] and m['swap_used_gb'] >= 0.95 * m['swap_total_gb']:
                w.append({'level': 'red' if m['level'] != 'ok' else 'amber', 'text': '%s swap full (%.0f/%.0f GB)' % (h, m['swap_used_gb'], m['swap_total_gb'])})
        if r.get('load') and r.get('nproc'):
            l1 = r['load'][0]; r['load_level'] = 'red' if l1 > r['nproc'] else 'amber' if l1 > 0.85 * r['nproc'] else 'ok'
            if r['load_level'] != 'ok': w.append({'level': r['load_level'], 'text': '%s load %.1f on %d CPUs' % (h, l1, r['nproc'])})
        for g in r['gpus']:
            if g['temp_c'] >= 85: w.append({'level': 'amber', 'text': '%s GPU%d %d°C' % (h, g['idx'], g['temp_c'])})
        if not r['reachable']: w.append({'level': 'amber', 'text': '%s unreachable: %s' % (h, r['error'])})
        r['warnings'] = w; hosts.append(r)
    json.dump(hist, open(HIST + '.tmp', 'w')); os.replace(HIST + '.tmp', HIST)
    doc = {'generated_utc': iso(t), 'hosts': hosts, 'thresholds': 'disk red >=92% or <12h to full; amber >=85% or <24h; RAM red <8% avail, amber <15%; growth from samples 20min-3h old',
           'warnings': [x for r in hosts for x in r['warnings']], 'note': 'xbabe0 is deliberately never probed'}
    json.dump(doc, open(OUT + '.tmp', 'w'), indent=1); os.replace(OUT + '.tmp', OUT)
    if not quiet:
        for r in hosts:
            m = r.get('mem') or {}
            print('== %s reachable=%s load=%s/%s RAM avail %s/%s GB swap %s/%s GB' % (r['host'], r['reachable'], (r.get('load') or ['?'])[0], r.get('nproc'), m.get('avail_gb'), m.get('total_gb'), m.get('swap_used_gb'), m.get('swap_total_gb')))
            for d in r['disks']: print('   %-40s %5.1f%% used, %6.0f GB free  growth %s GB/h  to-full %s h  [%s]' % (d['mount'], d['pct'], d['avail'] / 1e9, d['growth_gb_per_h'], d['hours_to_full'], d['level']))
            for g in r['gpus']: print('   GPU%d %s util %s%% mem %s/%s MiB %s C' % (g['idx'], g['name'], g['util'], g['mem_used_mib'], g['mem_total_mib'], g['temp_c']))
        print('WARNINGS:'); [print('  [%s] %s' % (x['level'], x['text'])) for x in doc['warnings']] or print('  none')
if __name__ == '__main__': main()
