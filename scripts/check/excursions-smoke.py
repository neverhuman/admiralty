#!/usr/bin/env python3
"""Exercise an isolated real admiraltyd incident journal, including restart.

Use a new scratch directory under protected, owner-controlled ancestors.
This starts only the supplied binary on an ephemeral loopback port; no deployment.
"""
import argparse
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import time
import urllib.error
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--scratch', type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    scratch = args.scratch.absolute()
    scratch.mkdir(mode=0o700, parents=False, exist_ok=False)
    token = secrets.token_urlsafe(32)
    scope = dict(project='p27-smoke', root='root', task='task', contract_revision=1,
                 contract_digest='sha256:' + 'a' * 64,
                 protected_oracle_digest='sha256:' + 'b' * 64)
    for name, text in [('token', token), ('config.json', json.dumps(dict(
            schema_version=1, operator=None, agents=[], excursions=[dict(
                scope=scope, actions=['read', 'observe', 'propose'], evidence_digests=[])])))]:
        fd = os.open(scratch / name, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        with os.fdopen(fd, 'w') as stream:
            stream.write(text)
    with socket.socket() as available:
        available.bind(('127.0.0.1', 0))
        port = available.getsockname()[1]
    base = f'http://127.0.0.1:{port}'

    def post(path, value, expected=200, credential=token):
        request = urllib.request.Request(base + '/v2/incidents/' + path,
            data=json.dumps(value).encode(), headers={
                'Authorization': 'Bearer ' + credential,
                'Content-Type': 'application/json'}, method='POST')
        try:
            response = urllib.request.urlopen(request, timeout=10)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            assert response.status == expected, (path, response.status, expected)
            data = response.read(2 * 1024 * 1024 + 1)
            assert len(data) <= 2 * 1024 * 1024
            return json.loads(data)

    def start(log):
        process = subprocess.Popen([str(binary), '--database', str(scratch / 'incidents.db'),
            '--listen', f'127.0.0.1:{port}', '--token-file', str(scratch / 'token'),
            '--native-config', str(scratch / 'config.json')], stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError('isolated binary exited before readiness; inspect scratch/server.log')
                try:
                    with urllib.request.urlopen(base + '/healthz', timeout=1) as response:
                        if response.status == 200:
                            return process
                except (OSError, urllib.error.URLError):
                    pass
                time.sleep(0.1)
            raise RuntimeError('isolated binary readiness deadline')
        except BaseException:
            stop(process)
            raise

    def stop(process):
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)

    def incident(identity, parent=None):
        return dict(scope=scope, event=dict(event_id=identity, family_signature='storage-timeout',
            kind='escalation' if parent else 'failure', cause_parent=parent,
            evidence_digests=[], observed_tick=None))

    with (scratch / 'server.log').open('wb') as log:
        process = start(log)
        try:
            assert post('scopes', {}) == [scope]
            post('read', scope, 401, credential='wrong-token')
            post('read', dict(scope, root='foreign'), 403)
            initial = dict(scope=scope, command_id='initial', expected_revision=0,
                operation=dict(kind='observe', incidents=[incident('failure'), incident('escalation', 'failure')]))
            receipt = post('commands', initial)
            assert len(post('read', scope)['state']['episodes']) == 1
            recurrence = dict(scope=scope, command_id='recurrence', expected_revision=1,
                operation=dict(kind='observe', incidents=[incident('recurrence')]))
            post('commands', recurrence)
            state = post('read', scope)
            assert len(state['state']['episodes']) == 2
            fields = dict(scope='p27-smoke/root/task', steps='Replay recorded failure and escalation',
                expected='One incident per causal lineage', actual='Writer contention observed',
                tried_approaches='Do not add retries; prior retry failed',
                candidate_fix='Inspect bounded queue ownership', measurable_outcome='No duplicate episodes')
            proposal = dict(scope=scope, command_id='proposal', expected_revision=2,
                operation=dict(kind='propose', episode_id=state['state']['episodes'][0]['episode_id'],
                               reproduction=dict(fields=fields)))
            unsafe = json.loads(json.dumps(proposal))
            unsafe['operation']['reproduction']['fields']['actual'] = 'token=secret-fixture'
            post('commands', unsafe, 422)
            post('commands', proposal)
            replay = post('commands', initial)
            assert replay['replayed'] and replay['receipt'] == receipt['receipt']
            post('commands', dict(initial, command_id='stale'), 409)
            expected = post('read', scope)
            local = expected['proposals'][0]['proposal']
            assert local['execution_granted'] is False and local['external_publication_allowed'] is False
            assert local['proposed_investigator_max_model_turns'] == 1
            assert 'unknown' in local['uncertainty']
            assert local['reproduction_contract']['fields'] == fields
        finally:
            stop(process)
        process = start(log)
        try:
            assert post('read', scope) == expected
            assert post('commands', proposal)['replayed'] is True
        finally:
            stop(process)
    result = dict(status='passed', checks=['explicit_startup_grants', 'scope_isolation',
        'causal_lineage', 'recurrence', 'secret_rejection', 'bounded_local_proposal',
        'exact_retry', 'stale_cas', 'real_binary_restart'], scratch=str(scratch))
    (scratch / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))


if __name__ == '__main__':
    main()
