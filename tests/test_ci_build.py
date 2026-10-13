# TRACEWEAVER: file-role=native-ci-boundary-fixtures; req=REQ-PUB-003; trace=TRACE-PUB-003; ver=VER-PUB-003
# TRACEWEAVER: verifies=VER-PUB-003; req=REQ-PUB-003; trace=TRACE-PUB-003
"""Offline CI boundary tests; only synthetic metadata and bytes are used."""
import hashlib
import importlib.util
import pathlib
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]


def helper():
    path = ROOT / 'scripts/ci/build_contract.py'
    spec = importlib.util.spec_from_file_location('build_contract', path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class BuildBoundaryTests(unittest.TestCase):
    def setUp(self):
        self.ci = helper()

    def test_nonroot_host_and_only_no_device_fixture_pass(self):
        self.ci.validate_host(uid=1001, euid=1001, groups=['ci-build'], devices=[], sockets=[], python=True, sudo=False, capabilities=0)

    def test_privilege_and_device_exposure_fail_closed(self):
        safe = dict(uid=1001, euid=1001, groups=['ci-build'], devices=[], sockets=[], python=True, sudo=False, capabilities=0)
        for key, value in [('uid', 0), ('euid', 0), ('groups', ['docker']), ('groups', ['sudo']), ('groups', ['libvirt']), ('groups', ['lxd']), ('devices', ['/dev/nvidia0']), ('devices', ['/dev/mem']), ('sockets', ['/var/run/docker.sock']), ('python', False), ('sudo', True), ('capabilities', 1)]:
            with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                self.ci.validate_host(**(safe | {key: value}))

    def test_service_cgroup_resolution_and_hard_resource_bounds(self):
        self.assertEqual(str(self.ci.cgroup_directory('0::/system.slice/ci-build.service\n')), '/sys/fs/cgroup/system.slice/ci-build.service')
        self.ci.validate_resource_bounds('8589934592', '256', memory_limit=8 * 1024**3, pids_limit=256)
        for memory, pids in [('max', '256'), ('8589934593', '256'), ('8589934592', 'max'), ('8589934592', '257'), ('0', '128')]:
            with self.assertRaises(ValueError):
                self.ci.validate_resource_bounds(memory, pids, memory_limit=8 * 1024**3, pids_limit=256)
        for text in ['0::relative', '0::/../../escape', '1:memory:/service']:
            with self.assertRaises(ValueError):
                self.ci.cgroup_directory(text)

    def test_effective_parent_cgroup_bounds_and_missing_evidence(self):
        self.ci.validate_cgroup_hierarchy([{'memory.max': 'max', 'pids.max': 'max'}, {'memory.max': '8589934592', 'pids.max': '256'}], memory_limit=8 * 1024**3, pids_limit=256)
        for levels in [[{'memory.max': 'max', 'pids.max': 'max'}], [{'memory.max': '8589934593', 'pids.max': '256'}], [{'memory.max': '8589934592'}]]:
            with self.assertRaises(ValueError):
                self.ci.validate_cgroup_hierarchy(levels, memory_limit=8 * 1024**3, pids_limit=256)

    def test_actual_binary_version_format_and_required_tracked_lock(self):
        self.assertEqual(self.ci.parse_binary_version('dc-exporter 0.2.7\n'), '0.2.7')
        for output in ['0.2.7', 'dc-exporter 0.2.8 extra', 'dc-exporter nonsense']:
            with self.assertRaises(ValueError):
                self.ci.parse_binary_version(output)
        with tempfile.TemporaryDirectory() as temporary:
            directory = pathlib.Path(temporary)
            with self.assertRaises(ValueError):
                self.ci.validate_lock(directory, tracked=False)
            (directory / 'Cargo.lock').write_text('version = 3\n[[package]]\nname = "fixture"\nversion = "0.2.7"\n')
            self.ci.validate_lock(directory, tracked=True)
            with self.assertRaises(ValueError):
                self.ci.validate_lock(directory, tracked=False)

    def test_glibc_import_ceiling_uses_numeric_versions(self):
        self.ci.validate_glibc('symbol GLIBC_2.9\nsymbol GLIBC_2.34\n')
        for symbols in ['', 'symbol GLIBC_2.35', 'symbol GLIBC_2.100', 'symbol GLIBC_PRIVATE']:
            with self.subTest(symbols=symbols), self.assertRaises(ValueError):
                self.ci.validate_glibc(symbols)

    def test_manifest_binds_source_run_attempt_and_exact_file_bytes(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = pathlib.Path(tmp)
            (directory / 'dc-exporter-rs').write_bytes(b'ELF fixture')
            (directory / 'BUILD_INFO.txt').write_text('fixture\n')
            (directory / 'SHA256SUMS').write_text('fixture\n')
            manifest = self.ci.make_manifest(directory, repository='cryptolabsza/dc-exporter-rs-public', repository_id=1416116330, sha='a' * 40, ref='refs/heads/dev', run_id=10, run_attempt=2, artifact_name='dc-exporter-1416116330-10-2-' + 'a' * 40, cargo_version='0.2.7', binary_version='0.2.7')
            self.assertEqual(manifest['source']['run_attempt'], 2)
            self.assertEqual(manifest['source']['repository_id'], 1416116330)
            self.assertEqual(manifest['files']['dc-exporter-rs']['sha256'], hashlib.sha256(b'ELF fixture').hexdigest())
            self.assertEqual(manifest['files']['dc-exporter-rs']['size'], 11)
            (directory / 'unexpected.sh').write_text('do not run')
            with self.assertRaises(ValueError):
                self.ci.make_manifest(directory, **manifest['source'], cargo_version='0.2.7', binary_version='0.2.7')

    def test_build_version_mismatch_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp, self.assertRaises(ValueError):
            self.ci.make_manifest(pathlib.Path(tmp), repository='cryptolabsza/dc-exporter-rs-public', repository_id=1416116330, sha='a' * 40, ref='refs/heads/dev', run_id=10, run_attempt=2, artifact_name='dc-exporter-1416116330-10-2-' + 'a' * 40, cargo_version='0.2.7', binary_version='0.2.8')


if __name__ == '__main__':
    unittest.main()
