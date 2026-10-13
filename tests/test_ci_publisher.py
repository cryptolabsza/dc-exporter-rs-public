# TRACEWEAVER: file-role=publisher-provenance-fixtures; req=REQ-PUB-006; trace=TRACE-PUB-006; ver=VER-PUB-006
# TRACEWEAVER: verifies=VER-PUB-006; req=REQ-PUB-006; trace=TRACE-PUB-006
"""Bytes-only publisher tests, with mocked authoritative GitHub metadata."""
import contextlib
import copy
import hashlib
import importlib.util
import io
import json
import pathlib
import os
import unittest
from unittest import mock
import zipfile
import urllib.request

DRAFT = pathlib.Path(__file__).resolve().parents[1] / 'scripts/ci/publish.py'


def helper():
    spec = importlib.util.spec_from_file_location('publisher', DRAFT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class PublisherBoundaryTests(unittest.TestCase):
    def setUp(self):
        self.publisher = helper()
        self.sha = 'a' * 40
        self.repository_id = 1416116330
        self.name = f'dc-exporter-{self.repository_id}-10-2-{self.sha}'
        self.source = dict(repository='cryptolabsza/dc-exporter-rs-public', repository_id=self.repository_id, sha=self.sha, ref='refs/heads/dev', run_id=10, run_attempt=2, artifact_name=self.name)
        self.files = {'dc-exporter-rs': b'never execute me', 'BUILD_INFO.txt': b'fixture\n', 'SHA256SUMS': b'fixture\n'}
        self.manifest = dict(schema=1, source=self.source, cargo_version='0.2.7', binary_version='0.2.7', files={name: dict(size=len(data), sha256=hashlib.sha256(data).hexdigest()) for name, data in self.files.items()})
        self.run = dict(id=10, run_attempt=2, repository=dict(id=self.repository_id, full_name=self.source['repository']), head_repository=dict(id=self.repository_id, full_name=self.source['repository']), workflow_id=7, path='.github/workflows/build.yml', head_sha=self.sha, head_branch='dev', event='push', status='completed', conclusion='success')
        self.artifact = dict(id=90, name=self.name, expired=False, workflow_run=dict(id=10, head_sha=self.sha), size_in_bytes=100)

    def archive(self, additions=None, symlink=False):
        stream = io.BytesIO()
        with zipfile.ZipFile(stream, 'w') as archive:
            for name, data in self.files.items():
                if symlink and name == 'dc-exporter-rs':
                    link = zipfile.ZipInfo(name)
                    link.create_system = 3
                    link.external_attr = 0o120777 << 16
                    archive.writestr(link, data)
                else:
                    archive.writestr(name, data)
            archive.writestr('manifest.json', json.dumps(self.manifest))
            for name, data in (additions or {}).items():
                archive.writestr(name, data)
        return stream.getvalue()

    def test_validated_files_are_bytes_and_no_executable_is_invoked(self):
        files, manifest = self.publisher.validate_archive(self.archive(), self.source)
        self.assertEqual(files['dc-exporter-rs'], b'never execute me')
        self.assertEqual(manifest, self.manifest)

    def test_archive_rejects_paths_links_unexpected_duplicates_and_bombs(self):
        for name in ['../escape', '/escape', 'nested/dc-exporter-rs', 'postinst', 'dc-exporter-rs']:
            with self.subTest(name=name), self.assertRaises(ValueError):
                self.publisher.validate_archive(self.archive({name: b'bad'}), self.source)
        with self.assertRaises(ValueError):
            self.publisher.validate_archive(self.archive(), self.source, max_total=10)
        # Both archives have identical required names, bytes and hash-consistent
        # manifests. Only the required binary entry's Unix mode differs.
        self.publisher.validate_archive(self.archive(), self.source)
        with self.assertRaises(ValueError):
            self.publisher.validate_archive(self.archive(symlink=True), self.source)

    def test_manifest_mismatch_hash_size_version_extra_keys_fail_closed(self):
        mutations = [lambda m: m['source'].update(run_attempt=1), lambda m: m['source'].update(sha='b' * 40), lambda m: m.update(binary_version='0.2.8'), lambda m: m['files']['dc-exporter-rs'].update(sha256='0' * 64), lambda m: m['files']['dc-exporter-rs'].update(size=1), lambda m: m.update(untrusted_command='execute')]
        for mutate in mutations:
            old = copy.deepcopy(self.manifest)
            mutate(self.manifest)
            with self.assertRaises(ValueError):
                self.publisher.validate_archive(self.archive(), self.source)
            self.manifest = old

    def test_exact_successful_run_workflow_attempt_artifact_and_fresh_tip_required(self):
        self.publisher.validate_receipt(self.run, self.artifact, expected=self.source, workflow_id=7, artifact_id=90, tip=self.sha)
        for key, value in [('workflow_id', 8), ('path', '.github/workflows/untrusted.yml'), ('run_attempt', 1), ('event', 'pull_request'), ('conclusion', 'failure'), ('head_branch', 'main'), ('head_repository', dict(id=99))]:
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.publisher.validate_receipt(self.run | {key: value}, self.artifact, expected=self.source, workflow_id=7, artifact_id=90, tip=self.sha)
        for artifact in [self.artifact | {'expired': True}, self.artifact | {'id': 91}, self.artifact | {'name': 'wrong'}, self.artifact | {'workflow_run': {'id': 11, 'head_sha': self.sha}}]:
            with self.assertRaises(ValueError):
                self.publisher.validate_receipt(self.run, artifact, expected=self.source, workflow_id=7, artifact_id=90, tip=self.sha)
        with self.assertRaises(ValueError):
            self.publisher.validate_receipt(self.run, self.artifact, expected=self.source, workflow_id=7, artifact_id=90, tip='b' * 40)

    def test_authoritative_repository_run_sha_and_missing_identity_fields(self):
        self.publisher.validate_receipt(self.run, self.artifact, expected=self.source, workflow_id=7, artifact_id=90, tip=self.sha)
        # Retained private source: even a matching SHA/artifact name cannot
        # make its successful run eligible for the new public-source pipeline.
        old_private = dict(id=41, full_name='cryptolabsza/dc-exporter-rs')
        run_changes = [
            {'repository': old_private},
            {'repository': dict(id=41, full_name=self.source['repository'])},
            {'repository': dict(id=self.repository_id, full_name='cryptolabsza/dc-exporter-rs')},
            {'head_repository': dict(id=self.repository_id, full_name='cryptolabsza/dc-exporter-rs')},
            {'head_sha': 'b' * 40}, {'id': 11}, {'status': 'in_progress'},
        ]
        for change in run_changes:
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.publisher.validate_receipt(self.run | change, self.artifact, expected=self.source, workflow_id=7, artifact_id=90, tip=self.sha)
        for field in ['repository', 'head_repository', 'id', 'head_sha', 'status', 'run_attempt', 'workflow_id', 'path', 'head_branch', 'event', 'conclusion']:
            run = copy.deepcopy(self.run)
            del run[field]
            with self.subTest(missing=field), self.assertRaises(ValueError):
                self.publisher.validate_receipt(run, self.artifact, expected=self.source, workflow_id=7, artifact_id=90, tip=self.sha)
        for repository_field in ['repository', 'head_repository']:
            for missing in ['id', 'full_name']:
                run = copy.deepcopy(self.run)
                del run[repository_field][missing]
                with self.subTest(repository_field=repository_field, missing=missing), self.assertRaises(ValueError):
                    self.publisher.validate_receipt(run, self.artifact, expected=self.source, workflow_id=7, artifact_id=90, tip=self.sha)
        artifact = self.artifact | {'workflow_run': {'id': 10, 'head_sha': 'b' * 40}}
        with self.assertRaises(ValueError):
            self.publisher.validate_receipt(self.run, artifact, expected=self.source, workflow_id=7, artifact_id=90, tip=self.sha)
        for field in ['id', 'name', 'expired', 'workflow_run']:
            artifact = copy.deepcopy(self.artifact)
            del artifact[field]
            with self.subTest(artifact_missing=field), self.assertRaises(ValueError):
                self.publisher.validate_receipt(self.run, artifact, expected=self.source, workflow_id=7, artifact_id=90, tip=self.sha)
        for field in ['id', 'head_sha']:
            artifact = copy.deepcopy(self.artifact)
            del artifact['workflow_run'][field]
            with self.subTest(artifact_run_missing=field), self.assertRaises(ValueError):
                self.publisher.validate_receipt(self.run, artifact, expected=self.source, workflow_id=7, artifact_id=90, tip=self.sha)

    def test_entrypoint_never_executes_artifacts_or_logs_api_error_payloads(self):
        test = self
        canary = 'CANARY-TOKEN ' + '/Us' + 'ers/operator/hidden CANARY-API-BODY'
        class API:
            def __init__(self, failure=None):
                self.failure = failure
                self.assets = {}
                self.latest = []
            def get_run(self, run_id, attempt):
                if self.failure == 'api':
                    raise RuntimeError(canary)
                return test.run | ({'id': canary} if self.failure == 'metadata' else {})
            def get_artifact(self, artifact_id):
                return test.artifact
            def tip(self, branch):
                return test.sha
            def download_artifact(self, artifact_id):
                return canary.encode() if self.failure == 'archive' else test.archive()
            def list_assets(self, tag):
                return self.assets.get(tag, {})
            def upload(self, tag, name, data):
                self.assets.setdefault(tag, {})[name] = data
            def replace_latest(self, files):
                self.latest.append(files.copy())
        args = ['--run-id', '10', '--run-attempt', '2', '--artifact-id', '90', '--sha', self.sha, '--branch', 'dev']
        env = {'SOURCE_WORKFLOW_ID': '7', 'SOURCE_READER_TOKEN': canary, 'GITHUB_TOKEN': canary, 'GITHUB_REPOSITORY': 'cryptolabsza/dc-exporter-releases', 'GITHUB_DEFAULT_BRANCH': 'main', 'GITHUB_REF': 'refs/heads/main', 'GITHUB_EVENT_NAME': 'workflow_dispatch', 'GITHUB_WORKFLOW_REF': 'cryptolabsza/dc-exporter-releases/.github/workflows/publish.yml@refs/heads/main', 'GITHUB_SHA': 'c' * 40}
        for failure in [None, 'api', 'metadata', 'archive']:
            stdout, stderr = io.StringIO(), io.StringIO()
            api = API(failure)
            with contextlib.ExitStack() as stack:
                stack.enter_context(mock.patch.dict(os.environ, env, clear=True))
                traps = [stack.enter_context(mock.patch(target, side_effect=AssertionError('artifact execution attempted'))) for target in ['subprocess.run', 'subprocess.Popen', 'os.system', 'os.execv', 'os.execve', 'os.execvp', 'os.execvpe', 'builtins.eval', 'builtins.exec']]
                stack.enter_context(contextlib.redirect_stdout(stdout))
                stack.enter_context(contextlib.redirect_stderr(stderr))
                result = self.publisher.main(args, api=api)
                for trap in traps:
                    trap.assert_not_called()
            self.assertEqual(result, 0 if failure is None else 1)
            logs = stdout.getvalue() + stderr.getvalue()
            for private in [canary, 'CANARY', '/Us' + 'ers/', 'Traceback', 'never execute me']:
                self.assertNotIn(private, logs)
            if failure is not None:
                self.assertEqual(api.assets, {})
                self.assertEqual(api.latest, [])
            else:
                self.assertEqual(api.latest[0]['dc-exporter-rs'], self.files['dc-exporter-rs'])

    def test_immutable_upload_is_idempotent_and_never_clobbers(self):
        class API:
            def __init__(self):
                self.assets = {}
                self.writes = []
            def list_assets(self, tag):
                return self.assets.get(tag, {})
            def upload(self, tag, name, data):
                self.writes.append((tag, name))
                self.assets.setdefault(tag, {})[name] = data
        api = API()
        tag = 'dev-' + self.sha
        self.publisher.publish_immutable(api, tag, self.files)
        writes = len(api.writes)
        self.publisher.publish_immutable(api, tag, self.files)
        self.assertEqual(len(api.writes), writes)
        api.assets[tag]['dc-exporter-rs'] = b'changed'
        with self.assertRaises(ValueError):
            self.publisher.publish_immutable(api, tag, self.files)
        self.assertEqual(len(api.writes), writes)

    def test_latest_promotion_rechecks_tip_and_overwrites_build_info(self):
        class API:
            def __init__(self, tip):
                self.current_tip = tip
                self.writes = []
            def tip(self, branch):
                return self.current_tip
            def replace_latest(self, files):
                self.writes.append(files.copy())
        stale = API('b' * 40)
        with self.assertRaises(ValueError):
            self.publisher.promote_latest(stale, self.source, self.files)
        self.assertEqual(stale.writes, [])
        api = API(self.sha)
        self.publisher.promote_latest(api, self.source, self.files)
        self.assertEqual(api.writes[0]['BUILD_INFO.txt'], self.files['BUILD_INFO.txt'])
        api.current_tip = 'b' * 40
        with self.assertRaises(ValueError):
            self.publisher.promote_latest(api, self.source, self.files)
        self.assertEqual(len(api.writes), 1)

    def test_stable_gates_require_main_tag_version_and_hardware_hold(self):
        for source, tag, ancestry, enabled in [(self.source, 'v0.2.7', True, True), (self.source | {'ref': 'refs/heads/main'}, 'v0.2.8', True, True), (self.source | {'ref': 'refs/heads/main'}, 'v0.2.7', False, True), (self.source | {'ref': 'refs/heads/main'}, 'v0.2.7', True, False)]:
            with self.assertRaises(ValueError):
                self.publisher.validate_stable(source, tag, '0.2.7', tag_sha=self.sha, main_ancestor=ancestry, hardware_accepted=enabled)
        self.publisher.validate_stable(self.source | {'ref': 'refs/heads/main'}, 'v0.2.7', '0.2.7', tag_sha=self.sha, main_ancestor=True, hardware_accepted=True)

    def test_signed_download_redirect_strips_api_token_and_refuses_http(self):
        request = urllib.request.Request('https://api.github.com/repos/allowed/artifact', headers={'Authorization': 'Bearer CANARY-CREDENTIAL'})
        redirect = self.publisher.SafeRedirect()
        forwarded = redirect.redirect_request(request, None, 302, 'Found', {}, 'https://storage.example/signed')
        self.assertIsNone(forwarded.get_header('Authorization'))
        with self.assertRaises(ValueError):
            redirect.redirect_request(request, None, 302, 'Found', {}, 'http://storage.example/signed')

    def test_entrypoint_argument_errors_are_public_safe(self):
        stdout, stderr = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            result = self.publisher.main(['--run-id', 'CANARY-SECRET'])
        self.assertEqual(result, 1)
        self.assertNotIn('CANARY', stdout.getvalue() + stderr.getvalue())

    def test_latest_transport_checks_freshness_before_each_api_mutation(self):
        api = object.__new__(self.publisher.GithubAPI)
        api.reader = 'reader'
        api.writer = 'writer'
        api.latest_expected_sha = self.sha
        api.tip = mock.Mock(return_value='b' * 40)
        api.opener = mock.Mock()
        for method in ['POST', 'PATCH', 'DELETE']:
            with self.assertRaises(ValueError):
                api.request(method, '/repos/cryptolabsza/dc-exporter-releases/releases/1/assets')
        api.opener.open.assert_not_called()
        self.assertEqual(api.tip.call_count, 3)

    def test_publisher_host_is_nonroot_bounded_and_without_devices_or_sockets(self):
        safe = dict(uid=1001, euid=1001, groups=['ci-publish'], devices=[], sockets=[], sudo=False, capabilities=0, memory='2147483648', pids='128', cpu=2)
        self.publisher.validate_runtime(**safe)
        for key, value in [('uid', 0), ('euid', 0), ('groups', ['docker']), ('groups', ['sudo']), ('devices', ['/dev/mem']), ('sockets', ['/var/run/docker.sock']), ('sudo', True), ('capabilities', 1), ('memory', 'max'), ('memory', '2147483649'), ('pids', '129'), ('cpu', 4)]:
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.publisher.validate_runtime(**(safe | {key: value}))

    def test_publisher_preflight_can_run_before_any_credential_or_api_use(self):
        for failure in [None, ValueError('CANARY-PRIVATE-HOST')]:
            stdout, stderr = io.StringIO(), io.StringIO()
            with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(self.publisher, 'preflight', side_effect=failure) as preflight, contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                result = self.publisher.main(['--preflight'])
            preflight.assert_called_once_with()
            self.assertEqual(result, 0 if failure is None else 1)
            self.assertNotIn('CANARY', stdout.getvalue() + stderr.getvalue())

    def test_protected_entrypoint_supports_stale_stage_recovery_without_tip_gate(self):
        class Recovery:
            def __init__(self): self.recovered = []
            def recover_stage(self, rid): self.recovered.append(rid)
        api = Recovery()
        env = {'SOURCE_WORKFLOW_ID': '7', 'GITHUB_REPOSITORY': 'cryptolabsza/dc-exporter-releases', 'GITHUB_DEFAULT_BRANCH': 'main', 'GITHUB_REF': 'refs/heads/main', 'GITHUB_EVENT_NAME': 'workflow_dispatch', 'GITHUB_WORKFLOW_REF': 'cryptolabsza/dc-exporter-releases/.github/workflows/publish.yml@refs/heads/main'}
        with mock.patch.dict(os.environ, env, clear=True), contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(self.publisher.main(['--recover-stage-id', '50'], api=api), 0)
        self.assertEqual(api.recovered, [50])

    def test_public_logs_do_not_emit_payloads_credentials_or_operator_details(self):
        messages = self.publisher.public_failure_messages()
        self.assertTrue(messages)
        for message in messages:
            self.assertNotIn('/Us' + 'ers/', message)
            self.assertNotIn('/pri' + 'vate/', message)
            self.assertNotIn('token=', message)
            self.assertLess(len(message), 120)


class LatestHTTPTransitionTests(unittest.TestCase):
    """Exercise the real API class/transport with a complete synthetic server."""
    def setUp(self):
        self.publisher = helper()
        self.old = self.bundle('0.2.7', 'a' * 40)
        self.new = self.bundle('0.2.8', 'b' * 40)

    def bundle(self, version, sha):
        source = dict(repository='cryptolabsza/dc-exporter-rs-public', repository_id=1416116330, sha=sha, ref='refs/heads/dev', run_id=10, run_attempt=2, artifact_name=f'dc-exporter-1416116330-10-2-{sha}')
        files = {'dc-exporter-rs': ('binary-' + version).encode(), 'BUILD_INFO.txt': version.encode(), 'SHA256SUMS': b'synthetic-checksums', 'dc-exporter-rs_amd64.deb': ('package-' + version).encode(), f'dc-exporter-rs_{version}_amd64.deb': ('package-' + version).encode()}
        manifest = dict(schema=1, source=source, cargo_version=version, binary_version=version, files={name: dict(size=len(data), sha256=hashlib.sha256(data).hexdigest()) for name, data in files.items()})
        return files | {'manifest.json': json.dumps(manifest).encode()}

    def server(self, *, legacy=False, fault=None, advance=None, persistent=False):
        import urllib.error
        import urllib.parse
        test = self
        class Response:
            def __init__(self, data): self.data = data
            def __enter__(self): return self
            def __exit__(self, *args): pass
            def read(self, maximum): return self.data[:maximum]
        class Server:
            def __init__(self):
                self.tip = 'b' * 40
                old = test.old if not legacy else {name: data for name, data in test.old.items() if name in {'dc-exporter-rs', 'BUILD_INFO.txt', 'SHA256SUMS'}}
                self.old = old
                self.releases = {1: dict(id=1, tag_name='dev-latest', draft=False, prerelease=True, body='Existing legacy metadata' if legacy else 'previous metadata', name='Old latest', target_commitish='c' * 40)}
                self.files = {1: old.copy()}
                self.assets = {}
                self.next_asset = 100
                self.operations = []
                self.fault_used = False
                self.block_patches = False
                self.source_reads = 0
                self.denied_source = False
                self.target_identity = 1141480624
            def latest(self):
                return next((r for r in self.releases.values() if r['tag_name'] == 'dev-latest' and not r['draft']), None)
            def complete(self):
                latest = self.latest()
                if latest is not None:
                    test.assertIn(self.files[latest['id']], [self.old, test.new])
            def metadata(self, rid):
                result = []
                for name, data in self.files[rid].items():
                    key = (rid, name)
                    if key not in self.assets:
                        self.next_asset += 1
                        self.assets[key] = self.next_asset
                    result.append(dict(id=self.assets[key], name=name, size=len(data)))
                return result
            def open(self, request, timeout):
                url = urllib.parse.urlsplit(request.full_url)
                path = url.path
                method = request.get_method()
                raw = False
                payload = json.loads(request.data) if request.data and request.get_header('Content-type') == 'application/json' else None
                operation = ('publish' if method == 'PATCH' and payload.get('tag_name') == 'dev-latest' else 'retire' if method == 'PATCH' and payload.get('tag_name', '').startswith('dev-previous-') else 'upload' if method == 'POST' and url.netloc == 'uploads.github.com' else 'create' if method == 'POST' else '')
                if method in {'POST', 'PATCH', 'DELETE'}:
                    test.assertNotEqual(method, 'DELETE', 'public assets must never be deleted')
                    self.operations.append((method, path, payload))
                    if self.block_patches and method == 'PATCH':
                        raise RuntimeError('CANARY-API-OFFLINE')
                    if fault == operation + '-before' and not self.fault_used:
                        self.fault_used = True
                        raise RuntimeError('CANARY-API-BODY')
                if path == '/repos/cryptolabsza/dc-exporter-rs-public':
                    self.source_reads += 1
                    if self.denied_source:
                        raise RuntimeError('CANARY-DENIED-READER')
                    result = dict(id=1416116330, full_name='cryptolabsza/dc-exporter-rs-public')
                elif path == '/repos/cryptolabsza/dc-exporter-releases':
                    result = dict(id=self.target_identity, full_name='cryptolabsza/dc-exporter-releases')
                elif path.endswith('/git/ref/heads/dev'):

                    result = {'object': {'sha': self.tip}}
                elif path.endswith('/releases') and method == 'POST':
                    rid = max(self.releases) + 1
                    result = payload | {'id': rid}
                    self.releases[rid] = result.copy()
                    self.files[rid] = {}
                elif path.endswith('/releases') and method == 'GET':
                    result = list(self.releases.values())
                elif '/releases/tags/' in path:
                    tag = path.split('/releases/tags/')[1]
                    result = next((r for r in self.releases.values() if r['tag_name'] == tag and not r['draft']), None)
                    if result is None:
                        raise urllib.error.HTTPError(request.full_url, 404, 'missing', {}, None)
                elif '/releases/assets/' in path and method == 'GET':
                    aid = int(path.rsplit('/', 1)[1])
                    rid, name = next(key for key, value in self.assets.items() if value == aid)
                    result = self.files[rid][name]
                    raw = True
                elif path.endswith('/assets'):
                    rid = int(path.split('/releases/')[1].split('/')[0])
                    if method == 'GET':
                        result = self.metadata(rid)
                    else:
                        name = urllib.parse.parse_qs(url.query)['name'][0]
                        test.assertNotIn(name, self.files[rid])
                        self.files[rid][name] = request.data
                        result = {'id': 900}
                elif '/releases/' in path:
                    rid = int(path.rsplit('/', 1)[1])
                    if method == 'PATCH':
                        self.releases[rid].update(payload)
                    result = self.releases[rid]
                else:
                    raise AssertionError('unexpected API path')
                self.complete()
                if operation == advance:
                    self.tip = 'd' * 40
                    if persistent:
                        self.block_patches = True
                if fault == operation + '-after' and not self.fault_used:
                    self.fault_used = True
                    raise RuntimeError('CANARY-LOST-RESPONSE')
                return Response(result if raw else json.dumps(result).encode())
        server = Server()
        api = object.__new__(self.publisher.GithubAPI)
        api.reader, api.writer = 'synthetic-reader', 'synthetic-writer'
        api.opener = server
        api.latest_expected_sha = None
        return api, server

    def test_real_http_normal_version_transition_preserves_complete_old_and_new(self):
        api, server = self.server()
        with mock.patch.dict(os.environ, {'GITHUB_RUN_ID': '50', 'GITHUB_RUN_ATTEMPT': '1', 'GITHUB_SHA': 'c' * 40}), mock.patch('subprocess.run', side_effect=AssertionError('execution')) as execute:
            api.replace_latest(self.new)
        execute.assert_not_called()
        self.assertEqual(server.files[server.latest()['id']], self.new)
        self.assertEqual(server.files[1], self.old)
        self.assertEqual(server.releases[1]['tag_name'], 'dev-previous-1')

    def test_real_http_faults_and_source_advance_never_leave_half_latest(self):
        self.new = self.bundle('0.2.7', 'b' * 40)
        scenarios = [(fault, None) for fault in ['create-before', 'upload-before', 'retire-before', 'retire-after', 'publish-before', 'publish-after']] + [(None, advance) for advance in ['upload', 'retire', 'publish']]
        for fault, advance in scenarios:
            with self.subTest(fault=fault, advance=advance):
                api, server = self.server(fault=fault, advance=advance)
                with mock.patch.dict(os.environ, {'GITHUB_RUN_ID': '50', 'GITHUB_RUN_ATTEMPT': '1', 'GITHUB_SHA': 'c' * 40}):
                    with self.assertRaises(Exception):
                        api.replace_latest(self.new)
                self.assertTrue(server.operations, 'fixture must reach real mutation transport')
                self.assertEqual(server.operations[0][0], 'POST')
                self.assertTrue(server.operations[0][2]['draft'])
                server.complete()
                self.assertIsNotNone(server.latest(), 'recoverable one-shot failure must restore an alias')
                self.assertIn(server.files[server.latest()['id']], [server.old, self.new])
                self.assertEqual(server.files[1], server.old)

    def test_legacy_metadata_survives_and_durable_recovery_works_after_api_outage(self):
        api, server = self.server(legacy=True, advance='retire', persistent=True)
        with mock.patch.dict(os.environ, {'GITHUB_RUN_ID': '50', 'GITHUB_RUN_ATTEMPT': '1', 'GITHUB_SHA': 'c' * 40}):
            with self.assertRaises(Exception):
                api.replace_latest(self.new)
            self.assertEqual(server.files[1], server.old)
            self.assertEqual(server.releases[1]['body'], 'Existing legacy metadata')
            stage = next(r for r in server.releases.values() if r['id'] != 1)
            self.assertIn('previous', stage['body'])
            server.block_patches = False
            api.recover_stage(stage['id'])
        self.assertEqual(server.latest()['id'], 1)
        self.assertEqual(server.files[1], server.old)


    def test_actual_recovery_entrypoint_uses_only_release_writer_and_identity(self):
        for reader_environment in [{}, {'SOURCE_READER_TOKEN': 'CANARY-DENIED-READER', 'SOURCE_WORKFLOW_ID': 'not-available'}]:
            api, server = self.server(legacy=True, advance='retire', persistent=True)
            with mock.patch.dict(os.environ, {'GITHUB_RUN_ID': '50', 'GITHUB_RUN_ATTEMPT': '1', 'GITHUB_SHA': 'c' * 40}):
                with self.assertRaises(Exception):
                    api.replace_latest(self.new)
            stage = next(r for r in server.releases.values() if r['id'] != 1)
            server.block_patches = False
            server.denied_source = True
            env = dict(GITHUB_TOKEN='synthetic-writer', GITHUB_REPOSITORY='cryptolabsza/dc-exporter-releases', GITHUB_DEFAULT_BRANCH='main', GITHUB_REF='refs/heads/main', GITHUB_EVENT_NAME='workflow_dispatch', GITHUB_WORKFLOW_REF='cryptolabsza/dc-exporter-releases/.github/workflows/publish.yml@refs/heads/main') | reader_environment
            stdout, stderr = io.StringIO(), io.StringIO()
            with mock.patch.dict(os.environ, env, clear=True), mock.patch.object(self.publisher, 'preflight'), mock.patch('urllib.request.build_opener', return_value=server), contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                result = self.publisher.main(['--recover-stage-id', str(stage['id'])])
            self.assertEqual(result, 0)
            self.assertEqual(server.source_reads, 0)
            self.assertEqual(server.latest()['id'], 1)
            self.assertEqual(server.files[1], server.old)
            self.assertNotIn('CANARY', stdout.getvalue() + stderr.getvalue())

    def test_target_only_api_still_rejects_wrong_release_repository_identity(self):
        api, server = self.server()
        server.target_identity = 99
        with mock.patch.dict(os.environ, {'GITHUB_TOKEN': 'synthetic-writer'}, clear=True), mock.patch('urllib.request.build_opener', return_value=server), self.assertRaises(ValueError):
            self.publisher.GithubAPI(target_only=True)
        self.assertEqual(server.source_reads, 0)

    def test_normal_promotion_keeps_reader_and_workflow_configuration_gate(self):
        _, server = self.server()
        env = dict(GITHUB_TOKEN='synthetic-writer', GITHUB_REPOSITORY='cryptolabsza/dc-exporter-releases', GITHUB_DEFAULT_BRANCH='main', GITHUB_REF='refs/heads/main', GITHUB_EVENT_NAME='workflow_dispatch', GITHUB_WORKFLOW_REF='cryptolabsza/dc-exporter-releases/.github/workflows/publish.yml@refs/heads/main')
        args = ['--run-id', '10', '--run-attempt', '2', '--artifact-id', '90', '--sha', 'b' * 40, '--branch', 'dev']
        for source_env in [{}, {'SOURCE_WORKFLOW_ID': '7'}]:
            with mock.patch.dict(os.environ, env | source_env, clear=True), mock.patch.object(self.publisher, 'preflight'), mock.patch('urllib.request.build_opener', return_value=server), contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(self.publisher.main(args), 1)
        self.assertEqual(server.operations, [])

    def test_actual_publisher_workflow_skips_app_mint_for_recovery(self):
        supplied = os.environ.get('PUBLISHER_WORKFLOW_PATH')
        path = pathlib.Path(supplied) if supplied else pathlib.Path(__file__).resolve().parents[1] / '.github/workflows/publish.yml'
        if not path.exists():
            self.skipTest('Publisher workflow is tested in its separate trusted repository')
        text = path.read_text()
        start = text.index('      - name: Mint dedicated source-reader installation token')
        end = text.index('      - name:', start + 20)
        self.assertIn("if: ${{ inputs.recover_stage_id == '' }}", text[start:end])
        self.assertIn('--recover-stage-id "$INPUT_RECOVER_STAGE_ID"', text)


if __name__ == '__main__':
    unittest.main()
