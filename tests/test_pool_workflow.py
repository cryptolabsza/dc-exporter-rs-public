# TRACEWEAVER: file-role=public-pool-boundary-fixtures; req=REQ-PUB-003; trace=TRACE-PUB-003; ver=VER-PUB-003
# TRACEWEAVER: verifies=VER-PUB-003; req=REQ-PUB-003; trace=TRACE-PUB-003
"""Public pool producer contract. Offline only: no compiler, container or GPU runs."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
CONTRACT = json.loads(Path(os.environ['PUBLIC_ARTIFACT_CONTRACT']).read_text()) if os.environ.get('PUBLIC_ARTIFACT_CONTRACT') else {
    'source_repository': 'cryptolabsza/dc-exporter-rs-public', 'source_repository_id': 1416116330,
    'fresh_archive_entries': ['dc-exporter-rs', 'dc-exporter-rs_amd64.deb', 'dc-exporter-rs_0.2.7_amd64.deb', 'BUILD_INFO.txt', 'VERSION.txt', 'HELP.txt', 'SYMBOLS.txt', 'THIRD-PARTY-NOTICES.md', 'SHA256SUMS', 'manifest.json'],
    'limits': {'file': 134217728, 'total': 209715200}}


def load(path):
    spec = importlib.util.spec_from_file_location(path.stem, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def producer_path(name):
    candidate = ROOT / 'scripts/ci' / name
    if candidate.exists():
        return candidate
    # RED can exercise the actual retained producer before its approved adaptation.
    baseline = os.environ.get('POOL_BASELINE_DIR')
    return Path(baseline) / name if baseline else candidate


class PublicPoolManifestTests(unittest.TestCase):
    def setUp(self):
        self.assertTrue(producer_path('pool_manifest.py').is_file(), 'public pool manifest producer missing')
        self.producer = load(producer_path('pool_manifest.py'))
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.dist = Path(temp.name) / 'dist'
        self.dist.mkdir()
        self.env = {'GITHUB_REPOSITORY': CONTRACT['source_repository'], 'GITHUB_REPOSITORY_ID': str(CONTRACT['source_repository_id']), 'GITHUB_SHA': 'a' * 40, 'GITHUB_REF': 'refs/heads/dev', 'GITHUB_RUN_ID': '17', 'GITHUB_RUN_ATTEMPT': '2', 'GITHUB_EVENT_NAME': 'push', 'GITHUB_REF_PROTECTED': 'true'}
        for name in CONTRACT['fresh_archive_entries']:
            if name not in {'manifest.json', 'SHA256SUMS'}:
                (self.dist / name).write_bytes(b'dc-exporter 0.2.7\n' if name == 'VERSION.txt' else b'fixture bytes\n')
        sums = ''.join(f'{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.name}\n' for p in sorted(self.dist.iterdir()))
        (self.dist / 'SHA256SUMS').write_text(sums)

    def produce(self):
        return self.producer.manifest(self.dist, ROOT / 'Cargo.toml', self.env)

    def test_exact_public_identity_and_nine_hashed_assets(self):
        try:
            result = self.produce()
        except ValueError as error:
            self.fail(f'valid public-source artifact rejected: {error}')
        self.assertEqual(result['source']['repository_id'], CONTRACT['source_repository_id'])
        self.assertEqual(result['source']['artifact_name'], f"dc-exporter-1416116330-17-2-{'a' * 40}")
        self.assertEqual(set(result['files']), set(CONTRACT['fresh_archive_entries']) - {'manifest.json'})
        self.assertEqual(len((self.dist / 'SHA256SUMS').read_text().splitlines()), 8)
        for name, item in result['files'].items():
            self.assertEqual(item, {'size': (self.dist / name).stat().st_size, 'sha256': hashlib.sha256((self.dist / name).read_bytes()).hexdigest()})

    def test_retained_private_source_is_rejected_even_with_legacy_assets(self):
        self.env.update(GITHUB_REPOSITORY='cryptolabsza/dc-exporter-rs', GITHUB_REPOSITORY_ID='1141426952')
        (self.dist / 'THIRD-PARTY-NOTICES.md').unlink()
        with self.assertRaises(ValueError):
            self.produce()

    def test_untrusted_refs_events_and_unprotected_heads_rejected(self):
        for change in [{'GITHUB_REF': 'refs/pull/1/merge'}, {'GITHUB_EVENT_NAME': 'pull_request'}, {'GITHUB_REF': 'refs/heads/topic'}, {'GITHUB_REF_PROTECTED': 'false'}, {'GITHUB_EVENT_NAME': 'workflow_run'}]:
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.producer.manifest(self.dist, ROOT / 'Cargo.toml', self.env | change)

    def test_manifest_rejects_missing_extra_symlink_and_bad_run(self):
        for key, value in [('GITHUB_RUN_ID', '0'), ('GITHUB_RUN_ATTEMPT', '0'), ('GITHUB_SHA', 'not-a-sha')]:
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.producer.manifest(self.dist, ROOT / 'Cargo.toml', self.env | {key: value})
        (self.dist / 'extra').write_text('unapproved')
        with self.assertRaises(ValueError):
            self.produce()
        (self.dist / 'extra').unlink()
        (self.dist / 'THIRD-PARTY-NOTICES.md').unlink()
        with self.assertRaises(ValueError):
            self.produce()
        (self.dist / 'THIRD-PARTY-NOTICES.md').symlink_to(self.dist / 'HELP.txt')
        with self.assertRaises(ValueError):
            self.produce()


class PublicWorkflowAndPackagingTests(unittest.TestCase):
    def test_caller_is_fully_qualified_immutable_public_recipe_without_inherited_secrets(self):
        text = (ROOT / '.github/workflows/build.yml').read_text()
        self.assertRegex(text, r'uses: cryptolabsza/dc-exporter-rs-public/\.github/workflows/ci-pool-build-recipe\.yml@[0-9a-f]{40}(?:\s|$)')
        self.assertNotRegex(text, re.compile(r'^\s*(?:pull_request|pull_request_target|workflow_run|secrets):', re.M))

    def test_recipe_has_public_identity_protected_ref_and_visibility_routes(self):
        path = ROOT / '.github/workflows/ci-pool-build-recipe.yml'
        self.assertTrue(path.is_file(), 'public pinned reusable recipe missing')
        text = path.read_text()
        for expected in ['1416116330', 'cryptolabsza/dc-exporter-rs-public', 'github.ref_protected', 'toJSON(github.event.repository.private)', 'github.event.repository.id', 'ci-build-ephemeral', 'ci-build-public-ephemeral', 'ci-linux-cpu-v1-r1416116330']:
            self.assertIn(expected, text)
        self.assertNotIn('pull_request', text)
        self.assertNotIn('secrets: inherit', text)

    def test_typed_event_repository_private_public_and_unknown_routes(self):
        producer = load(producer_path('pool_manifest.py'))
        route = getattr(producer, 'runner_group', None)
        self.assertTrue(callable(route), 'typed event repository visibility guard missing')
        base = {'id': 1416116330, 'full_name': 'cryptolabsza/dc-exporter-rs-public'}
        self.assertEqual(route(base | {'private': True, 'visibility': 'private'}), 'ci-build-ephemeral')
        self.assertEqual(route(base | {'private': False, 'visibility': 'public'}), 'ci-build-public-ephemeral')
        for changed in [{}, {'private': None}, {'private': 'false'}, {'private': 0}, {'private': True, 'visibility': 'public'}, {'private': False, 'visibility': 'internal'}, {'private': False, 'id': 1141426952}]:
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                route(base | changed)

    def test_build_container_abi_before_cargo_and_notices_before_manifest(self):
        text = producer_path('pool-build.sh').read_text()
        commands = '\n'.join(line for line in text.splitlines() if not line.lstrip().startswith('#'))
        self.assertIn('scripts/ci/rusd_abi.py', commands, 'driver-free ABI check absent from actual pool command')
        self.assertLess(commands.index('scripts/ci/rusd_abi.py'), commands.index('cargo test'))
        for flag in ['--cap-drop=ALL', '--security-opt=no-new-privileges', '--cpus=2', '--memory=3g', '--pids-limit=256', '--user', '@sha256:']:
            self.assertIn(flag, commands)
        self.assertIn('scripts/ci/notice_inventory.py', commands)
        self.assertLess(commands.index('scripts/ci/notice_inventory.py'), commands.rindex('scripts/ci/pool_manifest.py'))
        self.assertIn('usr/share/doc/dc-exporter-rs', commands)
        self.assertRegex(commands, r'cp\s+dist/THIRD-PARTY-NOTICES\.md\s+[^\n]*copyright')
        self.assertIn('--tui', commands)

    def test_outer_build_failure_propagates_without_running_container_or_compiler(self):
        path = producer_path('pool-build.sh')
        with tempfile.TemporaryDirectory() as temporary:
            binpath = Path(temporary)
            for name, body in {'id': 'echo 1001', 'uname': 'echo Linux', 'git': 'if [[ "$1" == rev-parse ]]; then echo "$GITHUB_SHA"; fi', 'docker': 'exit 23'}.items():
                target = binpath / name
                target.write_text('#!/bin/bash\n' + body + '\n')
                target.chmod(0o755)
            event = binpath / 'event.json'
            event.write_text(json.dumps({'repository': {'id': 1416116330, 'full_name': CONTRACT['source_repository'], 'private': True, 'visibility': 'private'}}))
            env = os.environ | {'GITHUB_EVENT_NAME': 'push', 'GITHUB_REF_PROTECTED': 'true', 'GITHUB_REF': 'refs/heads/dev', 'GITHUB_EVENT_PATH': str(event), 'PATH': str(binpath) + ':' + os.environ['PATH'], 'GITHUB_REPOSITORY': CONTRACT['source_repository'], 'GITHUB_REPOSITORY_ID': str(CONTRACT['source_repository_id']), 'GITHUB_SHA': 'a' * 40}
            result = subprocess.run(['/bin/bash', str(path)], cwd=ROOT, env=env, capture_output=True)
            self.assertEqual(result.returncode, 23, 'public identity must reach mocked Docker; its failure must propagate')

    def test_maintained_service_matches_binary_install_and_port(self):
        text = (ROOT / 'systemd/dc-exporter.service').read_text()
        self.assertRegex(text, re.compile(r'^ExecStart=/usr/local/bin/dc-exporter-rs --port 9835(?:\s|$)', re.M))


class PublicPoolExportTests(unittest.TestCase):
    def test_exact_new_paths_are_selected_with_per_file_change_authority(self):
        exporter = load(ROOT / 'scripts/public_export.py')
        added = {'.github/workflows/ci-pool-build-recipe.yml', 'scripts/ci/pool-build.sh', 'scripts/ci/pool_manifest.py', 'scripts/ci/notice_inventory.py', 'tests/test_pool_workflow.py', 'tests/test_ci_notices.py', 'CONTRIBUTING.md', 'SECURITY.md', 'THIRD-PARTY-NOTICES.md'}
        self.assertTrue(added <= exporter.PUBLIC_PATHS, sorted(added - exporter.PUBLIC_PATHS))
        self.assertTrue(added <= exporter.REVIEWED_CHANGE_PATHS)
        vm_runtime_exceptions = {'src/metrics/collector.rs', 'src/metrics/vm.rs', 'src/metrics/pci.rs',
                                 'src/metrics/prometheus.rs'}
        self.assertEqual({path for path in exporter.REVIEWED_CHANGE_PATHS if path.startswith('src/')},
                         vm_runtime_exceptions)
        for path in ('src/main.rs', 'src/state.rs', 'src/metrics/thermal.rs',
                     'src/metrics/nvidia_rm/rusd.rs', 'src/tui/ui.rs', 'src/metrics/new_vm_path.rs'):
            self.assertNotIn(path, exporter.REVIEWED_CHANGE_PATHS)

    def test_tui_source_and_version_are_frozen(self):
        frozen = {'src/tui/app.rs': 'a105c80a8294205b73b0e562044c05337d81a4a679f4d2a63c30742fd8ae728e', 'src/tui/mod.rs': 'd29db51ead2a40eb7450acd3828a5cc5064fab0f8f137c4065826e07e9181175', 'src/tui/ui.rs': '29d863fdbb67e7524ee8d29716f53fff9edb704ea700e3d85a0b0e20bd421ff1'}
        for path, digest in frozen.items():
            self.assertEqual(hashlib.sha256((ROOT / path).read_bytes()).hexdigest(), digest, path)
        self.assertIn('version = "0.2.7"', (ROOT / "Cargo.toml").read_text())


if __name__ == '__main__':
    unittest.main()
