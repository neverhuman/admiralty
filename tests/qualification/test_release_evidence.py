from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import tempfile
import unittest

import release_evidence as release


class ReleaseEvidenceTests(unittest.TestCase):
    def setUp(self):
        # Keep qualification artifacts in the lane's durable target, never /tmp.
        target = Path(__file__).resolve().parents[2] / 'target' / 'p31-qualification'
        target.mkdir(parents=True, exist_ok=True)
        self.scratch = tempfile.TemporaryDirectory(dir=target)
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        self.revision = 'a' * 40
        artifact = self.root / 'evidence.json'
        artifact.write_text('{"synthetic_fixture":true}\n')
        self.digest = hashlib.sha256(artifact.read_bytes()).hexdigest()
        self.now = datetime(2026, 10, 19, 14, 20, tzinfo=timezone.utc)

    def manifest(self):
        return {'schema_version': 1, 'source_revision': self.revision, 'checks': [
            {'id': name, 'status': 'passed', 'kind': 'actual' if name in release.ACTUAL_REQUIRED else 'synthetic',
             'revision': self.revision, 'started_utc': '2026-10-18T14:00:00Z', 'finished_utc': '2026-10-19T14:00:00Z',
             'evidence': 'evidence.json', 'sha256': self.digest, 'savings_claim': False}
            for name in release.REQUIRED]}

    def qualify(self, manifest):
        return release.qualify(manifest, self.root, self.revision, self.now)

    def test_complete_synthetic_metadata_fixture_only_admits_independent_review(self):
        result = self.qualify(self.manifest())
        self.assertEqual(result['status'], 'ready_for_independent_review')
        self.assertEqual(len(result['verified_artifacts']), 12)
        self.assertFalse(result['full_packet_acceptance'])
        self.assertFalse(result['deployment_authorized'])
        self.assertFalse(result['savings_claim'])

    def test_missing_failed_unknown_and_short_soak_remain_incomplete(self):
        manifest = self.manifest()
        manifest['checks'].pop()
        manifest['checks'][0]['status'] = 'failed'
        manifest['checks'][1]['status'] = 'unavailable'
        manifest['checks'][2]['started_utc'] = '2026-10-19T13:00:00Z'
        result = self.qualify(manifest)
        self.assertEqual(result['status'], 'incomplete')
        self.assertEqual(len(result['remaining']), 4)

    def test_stale_head_future_timestamp_and_synthetic_provider_cannot_pass(self):
        for change in ({'revision': 'b' * 40}, {'finished_utc': '2026-10-20T14:00:00Z'}, {'kind': 'synthetic'}):
            manifest = self.manifest()
            manifest['checks'][4].update(change)
            self.assertEqual(self.qualify(manifest)['status'], 'incomplete')

    def test_modified_evidence_and_savings_assertions_are_not_accepted(self):
        manifest = self.manifest()
        manifest['checks'][-1]['savings_claim'] = True
        self.assertIn('cannot authorize a savings claim', self.qualify(manifest)['remaining'][0])
        (self.root / 'evidence.json').write_text('changed')
        self.assertEqual(self.qualify(self.manifest())['verified_artifacts'], [])

    def test_duplicate_unknown_and_wrong_manifest_revision_are_invalid(self):
        manifest = self.manifest()
        manifest['checks'].append(manifest['checks'][0])
        with self.assertRaises(ValueError):
            self.qualify(manifest)
        manifest = self.manifest()
        manifest['source_revision'] = 'b' * 40
        with self.assertRaises(ValueError):
            self.qualify(manifest)
        with self.assertRaises(ValueError):
            json.loads('{"checks":[],"checks":[]}', object_pairs_hook=release.unique_object)

    def test_traversal_symlink_and_nonregular_artifacts_are_refused(self):
        (self.root / 'alias').symlink_to(self.root / 'evidence.json')
        (self.root / 'directory').mkdir()
        (self.root / 'dir-alias').symlink_to(self.root, target_is_directory=True)
        for path in ('../evidence.json', '/etc/passwd', 'alias', 'dir-alias/evidence.json', 'directory'):
            with self.subTest(path=path), self.assertRaises((OSError, ValueError)):
                release.artifact_hash(self.root, path)

    def test_zero_checks_reports_every_missing_gate(self):
        result = self.qualify({'schema_version': 1, 'source_revision': self.revision, 'checks': []})
        self.assertEqual(len(result['remaining']), 12)


if __name__ == '__main__':
    unittest.main()
