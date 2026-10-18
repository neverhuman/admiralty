#!/usr/bin/env python3
"""One explicit, bounded OpenCode SSH qualification; never reads credential files."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import uuid

ORACLE = '''import random, sys, unittest
sys.path.insert(0, sys.argv.pop(1))
from gaps import uncovered_ranges
class Oracle(unittest.TestCase):
    def test_empty(self): self.assertEqual(uncovered_ranges(0, 10, []), [(0, 10)])
    def test_empty_window(self): self.assertEqual(uncovered_ranges(3, 3, [(1, 9)]), [])
    def test_clips_sorts_merges(self): self.assertEqual(uncovered_ranges(0, 20, [(8, 12), (-4, 2), (2, 5), (10, 15), (18, 30)]), [(5, 8), (15, 18)])
    def test_zero_width_and_duplicates(self): self.assertEqual(uncovered_ranges(-5, 5, [(-5, 0), (0, 0), (-5, 0), (2, 5)]), [(0, 2)])
    def test_no_mutation(self):
        source = [[5, 8], [1, 3]]; before = [x[:] for x in source]
        self.assertEqual(uncovered_ranges(0, 10, source), [(0, 1), (3, 5), (8, 10)])
        self.assertEqual(source, before)
    def test_invalid_window(self):
        for start, end in [(5, 2), (True, 8), (1, False), (0.5, 3)]:
            with self.subTest(start=start, end=end), self.assertRaises((TypeError, ValueError)): uncovered_ranges(start, end, [])
    def test_invalid_coverage(self):
        for source in [[(4, 2)], [(True, 4)], [(1, 2, 3)], [(0, 2.5)], [1]]:
            with self.subTest(source=source), self.assertRaises((TypeError, ValueError)): uncovered_ranges(0, 10, source)
    def test_seeded_reference(self):
        rng = random.Random(882)
        for _ in range(80):
            start = rng.randint(-20, 5); end = rng.randint(start, 25)
            covered = []
            for _ in range(rng.randrange(10)):
                a = rng.randint(-25, 25); covered.append((a, rng.randint(a, 30)))
            expected = []; begin = None
            for point in range(start, end):
                free = not any(a <= point < b for a, b in covered)
                if free and begin is None: begin = point
                if not free and begin is not None: expected.append((begin, point)); begin = None
            if begin is not None: expected.append((begin, end))
            self.assertEqual(uncovered_ranges(start, end, covered), expected)
if __name__ == '__main__': unittest.main()
'''

PROMPT = '''Implement only gaps.py: uncovered_ranges(start, end, covered) returns sorted disjoint half-open integer intervals in [start,end) not covered by any supplied interval. Clip and union coverage, merging touching/overlapping ranges; ignore zero-width coverage; support negative endpoints and empty windows. Reject non-integer endpoints including bool, malformed pairs, and reversed intervals/windows with TypeError or ValueError. Accept pairs as lists or tuples. Do not mutate input. Python standard library only. All required contract and visible tests are inside this checkout: read README.md and test_gaps.py; run python3 test_gaps.py . after implementing. Do not access paths outside this candidate checkout. README.md and test_gaps.py are protected: do not modify them. Owner already pinned a baseline Git commit; do not create or alter commits. Do not install packages, use network, inspect credentials, or modify any other file or existing project. Stop with a concise result. No delegation or model switching.'''

REMOTE = r'''
import hashlib,json,os,re,resource,signal,subprocess,sys,time
from pathlib import Path
p=json.load(sys.stdin);fixture=Path(p['root']);corrected=p.get('corrected',False)
if corrected:
    if not fixture.is_dir() or not fixture.name.startswith('admiralty-qualification-'):raise RuntimeError('unknown qualification fixture')
    root=fixture/'attempt-1';root.mkdir(mode=0o700)
else:
    root=fixture;root.mkdir(mode=0o700)
candidate=fixture/'candidate';oracle=fixture/'owner-oracle.py'
if not corrected:
    candidate.mkdir();oracle.write_text(p['oracle']);oracle.chmod(0o444)
    target=candidate/'gaps.py';target.write_text('def uncovered_ranges(start, end, covered):\n    raise NotImplementedError("qualification stub")\n')
    git=subprocess.run(['/usr/bin/git','init',str(candidate)],capture_output=True,text=True,timeout=15)
    if git.returncode: raise RuntimeError('private checkout initialization failed')
else:target=candidate/'gaps.py'
(candidate/'README.md').write_text(p['prompt']+'\n')
(candidate/'test_gaps.py').write_text(p['oracle']);(candidate/'test_gaps.py').chmod(0o444)
for a in [['add','gaps.py','README.md','test_gaps.py'],['-c','user.name=Admiralty Qualification','-c','user.email=qualification@localhost','commit','-m','Pin owner qualification fixture and visible tests']]:
    r=subprocess.run(['/usr/bin/git','-C',str(candidate),*a],capture_output=True,text=True,timeout=15)
    if r.returncode:raise RuntimeError('fixture-only baseline commit failed')
sha=lambda path:hashlib.sha256(path.read_bytes()).hexdigest()
baseline=sha(target);oracle_before=sha(oracle)
protected_before={name:sha(candidate/name) for name in ['README.md','test_gaps.py']}
head_before=subprocess.run(['/usr/bin/git','-C',str(candidate),'rev-parse','HEAD'],capture_output=True,text=True,timeout=10).stdout.strip()
before=subprocess.run(['/usr/bin/python3',str(oracle),str(candidate)],capture_output=True,text=True,timeout=10)
argv=[p['helper'],'--dir',str(candidate),'--json',p['prompt']]
events=root/'events.ndjson';errors=root/'helper.stderr';started=time.monotonic();timed_out=False
def limits(): resource.setrlimit(resource.RLIMIT_FSIZE,(8*1024*1024,8*1024*1024))
with events.open('wb') as out,errors.open('wb') as err:
    proc=subprocess.Popen(argv,stdout=out,stderr=err,start_new_session=True,preexec_fn=limits)
    try:code=proc.wait(timeout=120)
    except subprocess.TimeoutExpired:
        timed_out=True
        # No poll/reap between these signals: the direct child's PID remains held.
        os.killpg(proc.pid,signal.SIGTERM);time.sleep(0.2)
        try:os.killpg(proc.pid,signal.SIGKILL)
        except ProcessLookupError:pass
        code=proc.wait(timeout=5)
elapsed=time.monotonic()-started
after=subprocess.run(['/usr/bin/python3',str(oracle),str(candidate)],capture_output=True,text=True,timeout=10)
(root/'verifier.stdout').write_text(after.stdout);(root/'verifier.stderr').write_text(after.stderr)
oracle_after=sha(oracle)
shapes={};sessions=set();usage=[];invalid=0;tool_count=0
for raw in events.read_bytes().splitlines():
    if len(raw)>262144:invalid+=1;continue
    try:event=json.loads(raw)
    except Exception:invalid+=1;continue
    if not isinstance(event,dict):invalid+=1;continue
    kind=event.get('type','unknown');part=event.get('part',{})
    shapes.setdefault(kind,{'event_keys':sorted(event),'part_keys':sorted(part) if isinstance(part,dict) else []})
    if isinstance(event.get('sessionID'),str):sessions.add(event['sessionID'])
    if kind=='tool_use':tool_count+=1
    if kind=='step_finish' and isinstance(part,dict):
        tokens=part.get('tokens');usage.append({'tokens':tokens if isinstance(tokens,dict) else None,'reason':part.get('reason'),'cost_reported':part.get('cost') if isinstance(part.get('cost'),(int,float)) else None})
identities=[]
for session in sorted(sessions):
    if not re.fullmatch(r'[A-Za-z0-9_-]{1,160}',session):continue
    exported=subprocess.run([p['opencode'],'export','--sanitize',session],capture_output=True,text=True,timeout=15)
    if exported.returncode or len(exported.stdout)>8*1024*1024:continue
    try:data=json.loads(exported.stdout)
    except Exception:continue
    for message in data.get('messages',[]):
        info=message.get('info',{})
        if info.get('role')=='assistant':
            identity={k:info[k] for k in ['providerID','modelID'] if isinstance(info.get(k),str)}
            if identity and identity not in identities:identities.append(identity)
changes=subprocess.run(['/usr/bin/git','-C',str(candidate),'status','--porcelain','--untracked-files=all'],capture_output=True,text=True,timeout=10)
files=sorted(str(path.relative_to(candidate)) for path in candidate.rglob('*') if path.is_file() and '.git' not in path.relative_to(candidate).parts and '__pycache__' not in path.relative_to(candidate).parts)
protected_after={name:sha(candidate/name) for name in protected_before}
head_after=subprocess.run(['/usr/bin/git','-C',str(candidate),'rev-parse','HEAD'],capture_output=True,text=True,timeout=10).stdout.strip()
expected=['README.md','gaps.py','test_gaps.py']
receipt={'schema_version':1,'qualification_id':fixture.name,'attempt':1 if corrected else 0,'remote_root':str(root),'candidate':str(candidate),'model_requested':p['model'],'model_observed':identities,'helper_exit':code,'timed_out':timed_out,'elapsed_s':round(elapsed,3),'baseline_verifier_exit':before.returncode,'verifier_exit':after.returncode,'oracle_sha256_before':oracle_before,'oracle_sha256_after':oracle_after,'oracle_unchanged':oracle_before==oracle_after,'baseline_source_sha256':baseline,'candidate_source_sha256':sha(target),'candidate_files':files,'scope_valid':files==expected and protected_before==protected_after and head_before==head_after,'protected_files_before':protected_before,'protected_files_after':protected_after,'baseline_head':head_before,'candidate_head':head_after,'event_shapes':shapes,'event_sha256':sha(events),'event_bytes':events.stat().st_size,'malformed_events':invalid,'tool_events':tool_count,'sessions':sorted(sessions),'usage_step_reports':usage,'usage_assurance':'provider-reported step values; aggregation and billing semantics unqualified','artifacts':{name:{'path':str(root/name),'sha256':sha(root/name)} for name in ['events.ndjson','helper.stderr','verifier.stdout','verifier.stderr']},'oracle_artifact':{'path':str(oracle),'sha256':sha(oracle)},'runtime_managed':False,'restart_adoption_qualified':False}
(root/'qualification.json').write_text(json.dumps(receipt,indent=2)+'\n')
print(json.dumps(receipt))
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--node', required=True)
    parser.add_argument('--remote-parent', required=True)
    parser.add_argument('--remote-helper', required=True)
    parser.add_argument('--opencode', required=True)
    parser.add_argument('--model', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--correct-fixture', help='explicitly authorized setup correction, one new task in this existing owned fixture')
    parser.add_argument('--qualify', action='store_true', help='explicitly authorize exactly one bounded real coding task')
    args = parser.parse_args()
    if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.-]{0,127}', args.node):
        parser.error('node must be a registered safe SSH alias')
    for value in [args.remote_parent, args.remote_helper, args.opencode]:
        if not value.startswith('/') or '..' in value.split('/') or '\0' in value:
            parser.error('remote paths must be absolute, non-traversing paths on that node')
    if not args.qualify:
        parser.error('real qualification requires --qualify; use installed CLI --help for read-only probes')
    output = args.output.expanduser().absolute()
    if output.exists() or output.is_symlink() or not output.parent.is_dir():
        parser.error('output must be a new receipt file in an existing owned directory')
    if output.parent.stat().st_uid != os.getuid():
        parser.error('output parent must be owned by current user')
    root = args.remote_parent.rstrip('/') + '/admiralty-qualification-' + uuid.uuid4().hex[:16]
    prompt = PROMPT
    if args.correct_fixture:
        if not re.fullmatch(re.escape(args.remote_parent.rstrip('/')) + r'/admiralty-qualification-[0-9a-f]{16}', args.correct_fixture):
            parser.error('correction must name an owned qualification fixture immediately under remote parent')
        root = args.correct_fixture
    payload = {'root':root,'helper':args.remote_helper,'opencode':args.opencode,'model':args.model,'oracle':ORACLE,'prompt':prompt,'corrected':bool(args.correct_fixture)}
    command = ['/usr/bin/ssh','-T','-o','BatchMode=yes','-o','ConnectTimeout=8',args.node,shlex.join(['/usr/bin/python3','-c',REMOTE])]
    result = subprocess.run(command, input=json.dumps(payload), capture_output=True, text=True, timeout=180)
    if result.returncode:
        print(json.dumps({'state':'unresolved','remote_root':root,'ssh_exit':result.returncode,'stderr_sha256':hashlib.sha256(result.stderr.encode()).hexdigest()}))
        return 2
    receipt = json.loads(result.stdout)
    receipt['oracle_owner_expected_sha256'] = hashlib.sha256(ORACLE.encode()).hexdigest()
    receipt['owner_oracle_matches'] = receipt['oracle_sha256_before'] == receipt['oracle_owner_expected_sha256'] == receipt['oracle_sha256_after']
    receipt['assurance'] = 'One trusted-local helper task and independent owner oracle; no isolation, fleet scheduling, provider restart or throughput qualification.'
    with output.open('x') as handle:
        json.dump(receipt,handle,indent=2);handle.write('\n')
    print(json.dumps({'receipt':str(output),'remote_root':root,'helper_exit':receipt['helper_exit'],'verifier_exit':receipt['verifier_exit'],'oracle_unchanged':receipt['owner_oracle_matches'],'scope_valid':receipt['scope_valid'],'model_observed':receipt['model_observed'],'elapsed_s':receipt['elapsed_s']}))
    return 0 if receipt['helper_exit'] == receipt['verifier_exit'] == 0 and receipt['owner_oracle_matches'] and receipt['scope_valid'] and not receipt['timed_out'] else 1


if __name__ == '__main__':
    sys.exit(main())
