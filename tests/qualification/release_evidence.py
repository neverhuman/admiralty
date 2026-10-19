#!/usr/bin/env python3
"""Read-only P31 evidence preflight. Never deploys or grants acceptance."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import stat

MAX_MANIFEST = 1024 * 1024
MAX_ARTIFACT = 64 * 1024 * 1024
REQUIRED = (
    'invariant_matrix', 'load_query', 'synthetic_soak', 'clean_install',
    'provider_transport', 'restart_continuity', 'backup_restore', 'rollback',
    'staged_canary', 'fleet_handoff', 'context_fidelity', 'complete_cost_trial',
)
ACTUAL_REQUIRED = set(REQUIRED) - {'invariant_matrix', 'load_query', 'synthetic_soak'}


def timestamp(value):
    if not isinstance(value, str) or not value.endswith('Z'):
        raise ValueError('UTC timestamps must end in Z')
    result = datetime.fromisoformat(value[:-1] + '+00:00')
    if result.utcoffset().total_seconds() != 0:
        raise ValueError('UTC timestamp required')
    return result


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError('duplicate JSON key')
        result[key] = value
    return result


def artifact_hash(root, relative):
    if not isinstance(relative, str) or not relative or len(relative) > 1024:
        raise ValueError('bounded relative artifact path required')
    parts = relative.split('/')
    if any(part in ('', '.', '..') for part in parts):
        raise ValueError('artifact path must stay within the evidence root')
    # Open each component relative to a held descriptor. A symlink or directory
    # replacement cannot redirect a later read outside this root.
    directory = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    descriptor = None
    try:
        for part in parts[:-1]:
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=directory)
            os.close(directory)
            directory = child
        descriptor = os.open(parts[-1], os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory)
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode) or before.st_size > MAX_ARTIFACT:
            raise ValueError('artifact must be a regular file up to 64 MiB')
        digest = hashlib.sha256()
        count = 0
        while chunk := os.read(descriptor, 65536):
            count += len(chunk)
            if count > MAX_ARTIFACT:
                raise ValueError('artifact exceeds 64 MiB')
            digest.update(chunk)
        after = os.fstat(descriptor)
        if count != before.st_size or (before.st_size, before.st_mtime_ns, before.st_ctime_ns) != (after.st_size, after.st_mtime_ns, after.st_ctime_ns):
            raise ValueError('artifact changed while hashing')
        return digest.hexdigest()
    finally:
        if descriptor is not None:
            os.close(descriptor)
        os.close(directory)


def qualify(manifest, root, expected_revision, now=None):
    if not re.fullmatch(r'[0-9a-f]{40}(?:[0-9a-f]{24})?', expected_revision):
        raise ValueError('full expected source revision required')
    if not isinstance(manifest, dict) or manifest.get('schema_version') != 1 or manifest.get('source_revision') != expected_revision:
        raise ValueError('manifest must bind the exact expected source revision')
    checks = manifest.get('checks')
    if not isinstance(checks, list) or len(checks) > 64:
        raise ValueError('up to 64 named checks required')
    now = now or datetime.now(timezone.utc)
    seen = set()
    failures = []
    verified = []
    for check in checks:
        if not isinstance(check, dict) or check.get('id') not in REQUIRED or check['id'] in seen:
            raise ValueError('unknown or duplicate qualification gate')
        name = check['id']
        seen.add(name)
        if check.get('status') not in ('passed', 'failed', 'unavailable'):
            raise ValueError('explicit gate status required')
        if check['status'] != 'passed':
            failures.append(f'{name}: {check["status"]}')
            continue
        if check.get('revision') != expected_revision:
            failures.append(f'{name}: stale source revision')
            continue
        kind = check.get('kind')
        if kind not in ('actual', 'synthetic') or (name in ACTUAL_REQUIRED and kind != 'actual'):
            failures.append(f'{name}: actual supported journey required')
            continue
        start, finish = timestamp(check.get('started_utc')), timestamp(check.get('finished_utc'))
        if finish < start or finish > now:
            failures.append(f'{name}: invalid or future observation interval')
            continue
        if name == 'synthetic_soak' and (finish - start).total_seconds() < 86400:
            failures.append(f'{name}: at least 24 hours of observed elapsed time required')
            continue
        digest = check.get('sha256')
        if not isinstance(digest, str) or not re.fullmatch(r'[0-9a-f]{64}', digest):
            raise ValueError('exact SHA256 required for passed evidence')
        if artifact_hash(root, check.get('evidence')) != digest:
            failures.append(f'{name}: artifact hash mismatch')
            continue
        if name == 'complete_cost_trial' and check.get('savings_claim') is not False:
            # Economics is reviewed separately. Complete evidence may retain
            # a negative/inconclusive trial without manufacturing savings.
            failures.append(f'{name}: preflight cannot authorize a savings claim')
            continue
        verified.append(name)
    failures.extend(f'{name}: missing' for name in REQUIRED if name not in seen)
    return {
        'schema_version': 1, 'source_revision': expected_revision,
        'status': 'ready_for_independent_review' if not failures else 'incomplete',
        'verified_artifacts': verified, 'remaining': failures,
        'full_packet_acceptance': False, 'deployment_authorized': False, 'savings_claim': False,
        'scope': 'Source/time/hash preflight of declared evidence; independent review of contents and actual fleet ownership is required.',
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--evidence-root', type=Path, required=True)
    parser.add_argument('--expected-revision', required=True)
    args = parser.parse_args(argv)
    try:
        with args.manifest.open('rb') as handle:
            raw = handle.read(MAX_MANIFEST + 1)
        if len(raw) > MAX_MANIFEST:
            raise ValueError('manifest exceeds 1 MiB')
        manifest = json.loads(raw, object_pairs_hook=unique_object)
        result = qualify(manifest, args.evidence_root, args.expected_revision)
    except (OSError, ValueError, TypeError) as failure:
        print(json.dumps({'status': 'invalid', 'error': str(failure), 'full_packet_acceptance': False, 'deployment_authorized': False}))
        return 2
    print(json.dumps(result, indent=2))
    return 0 if result['status'] == 'ready_for_independent_review' else 1


if __name__ == '__main__':
    raise SystemExit(main())
