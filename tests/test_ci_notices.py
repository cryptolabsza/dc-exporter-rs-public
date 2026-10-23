# TRACEWEAVER: file-role=locked-notice-fixtures; req=REQ-PUB-007; trace=TRACE-PUB-007; ver=VER-PUB-007
# TRACEWEAVER: verifies=VER-PUB-007; req=REQ-PUB-007; trace=TRACE-PUB-007
"""Locked resolved closure and licence files; fixtures claim no actual crate licensing."""
import copy
import hashlib
import importlib.util
from pathlib import Path
import re
import tempfile
import tomllib
import unittest
import urllib.request
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]


class LockedNoticeTests(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.scratch = Path(scratch.name)
        self.lock = tomllib.loads((ROOT / 'Cargo.lock').read_text())
        project = tomllib.loads((ROOT / 'Cargo.toml').read_text())['package']
        self.root_lock = next(p for p in self.lock['package'] if p['name'] == project['name'] and p['version'] == project['version'] and p.get('source') is None)
        # Small real lock subgraph: project -> hyper -> http -> bytes.
        self.locked = {name: next(p for p in self.lock['package'] if p['name'] == name and p.get('source', '').startswith('registry+')) for name in ['hyper', 'http', 'bytes']}
        self.ids = {name: f"{p['source']}#{name}@{p['version']}" for name, p in self.locked.items()}
        self.root_id = f"path+{ROOT.as_uri()}#{project['name']}@{project['version']}"
        self.texts, self.crates, packages = {}, {}, []
        for name, locked in self.locked.items():
            crate = self.scratch / name
            crate.mkdir()
            # Distinct actual fixture files; no statement about upstream licensing.
            text = f"SYNTHETIC TEST LICENCE FOR {name} {locked['version']}\nThis distinct fixture has no upstream licensing authority.\n"
            (crate / 'LICENSE').write_text(text)
            (crate / 'Cargo.toml').write_text(f"[package]\nname = \"{name}\"\nversion = \"{locked['version']}\"\n")
            self.texts[name], self.crates[name] = text, crate
            packages.append({'id': self.ids[name], 'name': name, 'version': locked['version'], 'source': locked['source'], 'license': 'MIT', 'license_file': None, 'manifest_path': str(crate / 'Cargo.toml')})
        packages.insert(0, {'id': self.root_id, 'name': project['name'], 'version': project['version'], 'source': None, 'license': project['license'], 'license_file': None, 'manifest_path': str(ROOT / 'Cargo.toml')})
        links = [(self.root_id, 'hyper'), (self.ids['hyper'], 'http'), (self.ids['http'], 'bytes')]
        nodes = [{'id': node, 'dependencies': [self.ids[child]], 'deps': [{'name': child, 'pkg': self.ids[child], 'dep_kinds': [{'kind': None, 'target': None}]}], 'features': []} for node, child in links]
        nodes.append({'id': self.ids['bytes'], 'dependencies': [], 'deps': [], 'features': []})
        self.metadata = {'version': 1, 'workspace_members': [self.root_id], 'workspace_default_members': [self.root_id], 'packages': packages, 'resolve': {'root': self.root_id, 'nodes': nodes}}

    def generate(self, metadata=None):
        path = ROOT / 'scripts/ci/notice_inventory.py'
        self.assertTrue(path.is_file(), 'locked Linux runtime dependency notice generator missing')
        spec = importlib.util.spec_from_file_location('notice_inventory', path)
        notices = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(notices)
        return notices.generate(metadata or self.metadata, self.lock, (ROOT / 'THIRD-PARTY-NOTICES.md').read_text())

    def omitted_nvml_fixture(self, name='nvml-wrapper'):
        # These exact locked releases omit all licence files from their archives.
        # Text below is synthetic; upstream provenance is pinned in the helper.
        spec = importlib.util.spec_from_file_location('notice_inventory', ROOT / 'scripts/ci/notice_inventory.py')
        notices = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(notices)
        locked = next(p for p in self.lock['package'] if p['name'] == name)
        crate = self.crates['hyper']
        (crate / 'LICENSE').unlink(missing_ok=True)
        package = {'id': f"{locked['source']}#{name}@{locked['version']}",
                   'name': name, 'version': locked['version'], 'source': locked['source'],
                   'license': 'MIT OR Apache-2.0', 'license_file': None,
                   'manifest_path': str(crate / 'Cargo.toml')}
        return notices, locked, package

    def test_exact_omitted_nvml_releases_preserve_both_pinned_upstream_texts(self):
        for name in ['nvml-wrapper', 'nvml-wrapper-sys']:
            with self.subTest(name=name):
                notices, locked, package = self.omitted_nvml_fixture(name)
                texts = [b'SYNTHETIC MIT fixture; no upstream licensing authority.\n',
                         b'SYNTHETIC Apache fixture; no upstream licensing authority.\n']
                pins = [(filename, hashlib.sha256(data).hexdigest())
                        for filename, data in zip(['LICENSE-MIT', 'LICENSE-APACHE'], texts)]
                metadata = copy.deepcopy(self.metadata)
                metadata['packages'] = [metadata['packages'][0], package]
                metadata['resolve']['nodes'] = [
                    {'id': self.root_id, 'dependencies': [package['id']], 'deps': [{'pkg': package['id'], 'dep_kinds': [{'kind': None, 'target': None}]}]},
                    {'id': package['id'], 'dependencies': [], 'deps': []}]
                with mock.patch.object(notices, 'UPSTREAM_FILES', pins, create=True), mock.patch.object(urllib.request, 'urlopen') as fetch:
                    fetch.return_value.__enter__.return_value.read.side_effect = texts
                    text, records, _ = notices.collect(metadata, self.lock, (ROOT / 'THIRD-PARTY-NOTICES.md').read_text())
                    self.assertEqual([r['name'] for r in records], [name])
                    files = records[0]['files']
                    for body in texts:
                        self.assertIn(body.decode().strip(), text)
                self.assertEqual([f['text'] for f in files], [t.decode() for t in texts])
                self.assertEqual([f['sha256'] for f in files], [p[1] for p in pins])
                for result, call, (filename, _) in zip(files, fetch.call_args_list, pins):
                    url = 'https://raw.githubusercontent.com/rust-nvml/nvml-wrapper/940354559b6131b23734a97050c929483c9b4ad0/' + filename
                    self.assertEqual(result['upstream_url'], url)
                    self.assertEqual(result['path'], 'upstream/' + filename)
                    self.assertEqual(call.args[0], url)
                (self.crates['hyper'] / 'LICENSE').write_text(self.texts['hyper'])

    def test_upstream_supplement_rejects_hash_mismatch(self):
        notices, locked, package = self.omitted_nvml_fixture()
        with mock.patch.object(urllib.request, 'urlopen') as fetch:
            fetch.return_value.__enter__.return_value.read.return_value = b'incorrect upstream bytes'
            with self.assertRaisesRegex(ValueError, 'checksum'):
                notices.licence_files(package, locked['checksum'])

    def test_upstream_supplement_is_exact_identity_expression_and_archive_only(self):
        notices, locked, package = self.omitted_nvml_fixture()
        for field, value in [('name', 'unknown-crate'), ('version', '0.10.1'),
                             ('source', 'registry+https://invalid.example/index'),
                             ('license', 'MIT')]:
            changed = dict(package, **{field: value})
            with self.subTest(field=field), mock.patch.object(urllib.request, 'urlopen') as fetch:
                with self.assertRaises(ValueError):
                    notices.licence_files(changed, locked['checksum'])
                fetch.assert_not_called()
        with mock.patch.object(urllib.request, 'urlopen') as fetch:
            with self.assertRaises(ValueError):
                notices.licence_files(package, '0' * 64)
            fetch.assert_not_called()

    def test_missing_licence_error_identifies_exact_package(self):
        (self.crates['hyper'] / 'LICENSE').unlink()
        with self.assertRaisesRegex(ValueError, re.escape('hyper ' + self.locked['hyper']['version'])):
            self.generate()

    def test_fixture_root_direct_and_transitive_identities_match_actual_lock(self):
        self.assertEqual(self.metadata['packages'][0]['name'], self.root_lock['name'])
        self.assertEqual(self.metadata['packages'][0]['version'], self.root_lock['version'])
        for parent, child in [(self.root_lock, 'hyper'), (self.locked['hyper'], 'http'), (self.locked['http'], 'bytes')]:
            self.assertIn(child, [dep.split()[0] for dep in parent['dependencies']])
        self.assertEqual(len(set(self.texts.values())), 3)

    def test_exact_resolved_coverage_all_actual_file_texts_and_determinism(self):
        first = self.generate()
        self.assertEqual(first, self.generate())
        expected = {f"{name} {package['version']}" for name, package in self.locked.items()}
        self.assertEqual(set(re.findall(r'^## Runtime dependency: (.+)$', first, re.MULTILINE)), expected)
        for text in self.texts.values():
            self.assertIn(text.strip(), first)
        self.assertNotIn(str(self.scratch), first)
        self.assertNotIn(str(ROOT), first)

    def test_absent_referenced_package_is_fatal(self):
        metadata = copy.deepcopy(self.metadata)
        metadata['packages'] = [p for p in metadata['packages'] if p['id'] != self.ids['http']]
        with self.assertRaises(ValueError):
            self.generate(metadata)

    def test_absent_transitive_resolve_node_is_fatal(self):
        metadata = copy.deepcopy(self.metadata)
        metadata['resolve']['nodes'] = [p for p in metadata['resolve']['nodes'] if p['id'] != self.ids['bytes']]
        with self.assertRaises(ValueError):
            self.generate(metadata)

    def test_missing_direct_or_transitive_license_is_fatal(self):
        for name in ['hyper', 'http', 'bytes']:
            with self.subTest(name=name):
                path = self.crates[name] / 'LICENSE'
                path.unlink()
                try:
                    with self.assertRaises(ValueError):
                        self.generate()
                finally:
                    path.write_text(self.texts[name])

    def test_unresolved_direct_or_transitive_license_is_fatal(self):
        for name in self.locked:
            metadata = copy.deepcopy(self.metadata)
            next(p for p in metadata['packages'] if p['id'] == self.ids[name])['license'] = None
            with self.subTest(name=name), self.assertRaises(ValueError):
                self.generate(metadata)

    def test_explicit_unresolved_licensing_markers_fail_closed(self):
        for expression in ['NOASSERTION', 'NONE', 'UNLICENSED', 'LicenseRef-unresolved']:
            metadata = copy.deepcopy(self.metadata)
            metadata['packages'][1]['license'] = expression
            with self.subTest(expression=expression), self.assertRaises(ValueError):
                self.generate(metadata)

    def test_root_or_registry_lock_identity_mismatch_is_fatal(self):
        for identity in [self.root_id, *self.ids.values()]:
            for field, value in [('name', 'not-in-locked-source'), ('version', '9999.0.0'), ('source', 'registry+https://invalid.example/index')]:
                metadata = copy.deepcopy(self.metadata)
                next(p for p in metadata['packages'] if p['id'] == identity)[field] = value
                with self.subTest(identity=identity, field=field), self.assertRaises(ValueError):
                    self.generate(metadata)

    def test_unreachable_metadata_package_is_not_reported(self):
        metadata = copy.deepcopy(self.metadata)
        unused = next(p for p in self.lock['package'] if p['name'] not in self.locked and p.get('source', '').startswith('registry+'))
        metadata['packages'].append({'id': f"{unused['source']}#{unused['name']}@{unused['version']}", 'name': unused['name'], 'version': unused['version'], 'source': unused['source'], 'license': None, 'license_file': None, 'manifest_path': str(self.scratch / 'missing' / 'Cargo.toml')})
        result = self.generate(metadata)
        expected = {f"{name} {package['version']}" for name, package in self.locked.items()}
        self.assertEqual(set(re.findall(r'^## Runtime dependency: (.+)$', result, re.MULTILINE)), expected)

    def test_project_and_upstream_notices_are_preserved(self):
        text = self.generate()
        for expected in ['Copyright (c) CryptoLabs ZA', 'Copyright (c) 2026 Philip Langdale.', 'NVIDIA CORPORATION & AFFILIATES.', 'Permission is hereby granted, free of charge']:
            self.assertIn(expected, text)


if __name__ == '__main__':
    unittest.main()
