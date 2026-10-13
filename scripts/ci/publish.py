#!/usr/bin/env python3
# TRACEWEAVER: file-role=bytes-only-publisher; req=REQ-PUB-006; trace=TRACE-PUB-006; ver=VER-PUB-006
"""Trusted release-repository publisher. Downloaded artifacts are only bytes.

Copy this reviewed file into the protected release repository. It must never be
loaded from a source checkout or downloaded artifact during publication.
"""
import argparse
import grp
import hashlib
import io
import json
import os
from pathlib import Path
import re
import stat
import shutil
import sys
import urllib.error
import urllib.parse
import urllib.request
import zipfile

SOURCE_REPOSITORY = 'cryptolabsza/dc-exporter-rs-public'
SOURCE_REPOSITORY_ID = 1416116330
RELEASE_REPOSITORY = 'cryptolabsza/dc-exporter-releases'
RELEASE_REPOSITORY_ID = 1141480624
WORKFLOW_PATH = '.github/workflows/build.yml'
MAX_TOTAL = 200 * 1024 * 1024
MAX_FILE = 128 * 1024 * 1024
# Hardware acceptance and stable promotion require a separately reviewed change.
HARDWARE_ACCEPTED = False
VERSION = re.compile(r'\d+\.\d+\.\d+\Z')
SHA = re.compile(r'[0-9a-f]{40}\Z')


def require(condition):
    if not condition:
        raise ValueError('Publisher validation failed')


def positive(value):
    return type(value) is int and value > 0


def validate_source(source):
    require(isinstance(source, dict) and set(source) == {'repository', 'repository_id', 'sha', 'ref', 'run_id', 'run_attempt', 'artifact_name'})
    require(source['repository'] == SOURCE_REPOSITORY and source['repository_id'] == SOURCE_REPOSITORY_ID)
    require(isinstance(source['sha'], str) and SHA.fullmatch(source['sha']) is not None)
    require(source['ref'] in {'refs/heads/dev', 'refs/heads/main'})
    require(positive(source['run_id']) and positive(source['run_attempt']))
    require(source['artifact_name'] == f"dc-exporter-{SOURCE_REPOSITORY_ID}-{source['run_id']}-{source['run_attempt']}-{source['sha']}")


def validate_receipt(run, artifact, *, expected, workflow_id, artifact_id, tip):
    validate_source(expected)
    require(positive(workflow_id) and positive(artifact_id))
    require(isinstance(run, dict) and isinstance(artifact, dict))
    for name in ['repository', 'head_repository']:
        repo = run.get(name)
        require(isinstance(repo, dict) and repo.get('id') == SOURCE_REPOSITORY_ID and repo.get('full_name') == SOURCE_REPOSITORY)
    exact = dict(id=expected['run_id'], run_attempt=expected['run_attempt'], workflow_id=workflow_id, path=WORKFLOW_PATH, head_sha=expected['sha'], head_branch=expected['ref'].removeprefix('refs/heads/'), status='completed', conclusion='success')
    require(all(run.get(name) == value for name, value in exact.items()))
    require(run.get('event') in {'push', 'workflow_dispatch'})
    require(tip == expected['sha'])
    require(artifact.get('id') == artifact_id and artifact.get('name') == expected['artifact_name'] and artifact.get('expired') is False)
    receipt = artifact.get('workflow_run')
    require(isinstance(receipt, dict) and receipt.get('id') == expected['run_id'] and receipt.get('head_sha') == expected['sha'])


def json_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result)
        result[key] = value
    return result


def allowed_files(version):
    return {'dc-exporter-rs', 'BUILD_INFO.txt', 'SHA256SUMS', 'manifest.json', 'dc-exporter-rs_amd64.deb', f'dc-exporter-rs_{version}_amd64.deb'}


def validate_archive(data, expected, *, max_total=MAX_TOTAL):
    validate_source(expected)
    require(isinstance(data, bytes) and 0 < len(data) <= max_total)
    files = {}
    try:
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            entries = archive.infolist()
            require(4 <= len(entries) <= 6)
            total = 0
            for entry in entries:
                name = entry.filename
                require(name not in files and name == os.path.basename(name) and '/' not in name and '\\' not in name)
                mode = entry.external_attr >> 16
                require(not entry.is_dir() and not entry.flag_bits & 1 and stat.S_IFMT(mode) in {0, stat.S_IFREG})
                require(entry.compress_type in {zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED})
                require(0 < entry.file_size <= MAX_FILE)
                total += entry.file_size
                require(total <= max_total)
                files[name] = archive.read(entry)
                require(len(files[name]) == entry.file_size)
        require('manifest.json' in files and len(files['manifest.json']) <= 64 * 1024)
        manifest = json.loads(files['manifest.json'], object_pairs_hook=json_object)
        require(isinstance(manifest, dict) and set(manifest) == {'schema', 'source', 'cargo_version', 'binary_version', 'files'})
        require(type(manifest['schema']) is int and manifest['schema'] == 1 and manifest['source'] == expected)
        version = manifest['cargo_version']
        require(isinstance(version, str) and VERSION.fullmatch(version) is not None and version == manifest['binary_version'])
        require(set(files).issubset(allowed_files(version)) and {'dc-exporter-rs', 'BUILD_INFO.txt', 'SHA256SUMS', 'manifest.json'}.issubset(files))
        package_names = allowed_files(version) - {'dc-exporter-rs', 'BUILD_INFO.txt', 'SHA256SUMS', 'manifest.json'}
        require(not package_names.intersection(files) or package_names.issubset(files))
        hashes = manifest['files']
        require(isinstance(hashes, dict) and set(hashes) == set(files) - {'manifest.json'})
        for name, metadata in hashes.items():
            require(isinstance(metadata, dict) and set(metadata) == {'size', 'sha256'})
            require(type(metadata['size']) is int and metadata['size'] == len(files[name]))
            require(metadata['sha256'] == hashlib.sha256(files[name]).hexdigest())
        if package_names.issubset(files):
            require(files['dc-exporter-rs_amd64.deb'] == files[f'dc-exporter-rs_{version}_amd64.deb'])
        return files, manifest
    except (zipfile.BadZipFile, KeyError, TypeError, UnicodeError, json.JSONDecodeError, RuntimeError) as error:
        raise ValueError('Publisher archive validation failed') from error


def publish_immutable(api, tag, files):
    existing = api.list_assets(tag)
    require(isinstance(existing, dict) and set(existing).issubset(files))
    # Check every existing byte before any mutation. A differing retry never
    # partially changes an immutable release or uses upload clobber behavior.
    require(all(files[name] == data for name, data in existing.items()))
    for name in sorted(files):
        if name not in existing:
            api.upload(tag, name, files[name])
    require(api.list_assets(tag) == files)


def promote_latest(api, source, files):
    validate_source(source)
    require(source['ref'] == 'refs/heads/dev' and api.tip('dev') == source['sha'])
    api.replace_latest(files)
    require(api.tip('dev') == source['sha'])


def validate_stable(source, tag, version, *, tag_sha, main_ancestor, hardware_accepted):
    validate_source(source)
    require(source['ref'] == 'refs/heads/main' and VERSION.fullmatch(version) is not None)
    require(tag == 'v' + version and tag_sha == source['sha'] and main_ancestor is True and hardware_accepted is True)


def public_failure_messages():
    return ['::error::Publisher validation or API operation failed']


def validate_runtime(*, uid, euid, groups, devices, sockets, sudo, capabilities, memory, pids, cpu):
    require(uid != 0 and euid != 0 and not {'root', 'sudo', 'wheel', 'docker', 'libvirt', 'libvirt-qemu', 'lxd', 'disk', 'kmem'}.intersection(groups))
    require(not devices and not sockets and not sudo and capabilities == 0)
    require(memory.isdecimal() and 0 < int(memory) <= 2 * 1024**3)
    require(pids.isdecimal() and 0 < int(pids) <= 128 and 0 < cpu <= 2)


def preflight():
    require(sys.platform == 'linux')
    status = Path('/proc/self/status').read_text()
    capabilities = 0
    for name in ['CapEff', 'CapPrm', 'CapAmb']:
        match = re.search(rf'^{name}:\s*([0-9a-f]+)$', status, re.M)
        require(match is not None)
        capabilities |= int(match[1], 16)
    groups = [grp.getgrgid(gid).gr_name for gid in set(os.getgroups() + [os.getgid()])]
    devices = list(Path('/dev').glob('nvidia*')) + [p for p in [Path('/dev/mem'), Path('/dev/kmem'), Path('/dev/dri')] if p.exists()]
    sockets = [p for p in [Path('/var/run/docker.sock'), Path('/run/containerd/containerd.sock'), Path('/var/run/libvirt/libvirt-sock'), Path('/var/snap/lxd/common/lxd/unix.socket')] if p.exists()]
    roots = [line[3:] for line in Path('/proc/self/cgroup').read_text().splitlines() if line.startswith('0::')]
    require(len(roots) == 1 and roots[0].startswith('/') and '..' not in roots[0].split('/'))
    root = Path('/sys/fs/cgroup')
    current = root / roots[0].lstrip('/')
    bounds = {'memory.max': [], 'pids.max': []}
    while True:
        for name, values in bounds.items():
            value = 'max' if current == root and not (current / name).exists() else (current / name).read_text().strip()
            require(value == 'max' or value.isdecimal() and int(value) > 0)
            if value != 'max':
                values.append(int(value))
        if current == root:
            break
        current = current.parent
    require(all(bounds.values()))
    validate_runtime(uid=os.getuid(), euid=os.geteuid(), groups=groups, devices=devices, sockets=sockets, sudo=shutil.which('sudo') is not None, capabilities=capabilities, memory=str(min(bounds['memory.max'])), pids=str(min(bounds['pids.max'])), cpu=os.cpu_count() or 999)


class PublicParser(argparse.ArgumentParser):
    def error(self, message):
        raise ValueError('Invalid publisher arguments')


class SafeRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, msg, headers, newurl):
        require(urllib.parse.urlsplit(newurl).scheme == 'https')
        redirected = super().redirect_request(request, fp, code, msg, headers, newurl)
        # Artifact/asset downloads use signed storage URLs. Never forward an
        # API credential to another origin, and never write signed URLs to logs.
        if redirected is not None and urllib.parse.urlsplit(newurl).netloc != urllib.parse.urlsplit(request.full_url).netloc:
            redirected.remove_header('Authorization')
        return redirected


class GithubAPI:
    def __init__(self, *, target_only=False):
        self.reader = None if target_only else os.environ['SOURCE_READER_TOKEN']
        self.writer = os.environ['GITHUB_TOKEN']
        require(bool(self.writer) and (target_only or bool(self.reader)))
        self.opener = urllib.request.build_opener(SafeRedirect())
        self.latest_expected_sha = None
        if not target_only:
            source = self.request('GET', f'/repos/{SOURCE_REPOSITORY}', source=True)
            require(source.get('id') == SOURCE_REPOSITORY_ID and source.get('full_name') == SOURCE_REPOSITORY)
        target = self.request('GET', f'/repos/{RELEASE_REPOSITORY}')
        require(target.get('id') == RELEASE_REPOSITORY_ID and target.get('full_name') == RELEASE_REPOSITORY)

    def request(self, method, path, *, source=False, payload=None, raw=False, upload=False):
        repository = SOURCE_REPOSITORY if source else RELEASE_REPOSITORY
        require(path.startswith(f'/repos/{repository}/') or path == f'/repos/{repository}')
        if method in {'POST', 'PATCH', 'DELETE'} and self.latest_expected_sha is not None:
            require(not source and self.tip('dev') == self.latest_expected_sha)
        token = self.reader if source else self.writer
        require(bool(token))
        headers = {'Authorization': 'Bearer ' + token, 'Accept': 'application/octet-stream' if raw else 'application/vnd.github+json', 'X-GitHub-Api-Version': '2022-11-28', 'User-Agent': 'dc-exporter-bytes-publisher'}
        data = payload if isinstance(payload, bytes) else json.dumps(payload).encode() if payload is not None else None
        if data is not None:
            headers['Content-Type'] = 'application/octet-stream' if upload else 'application/json'
        url = ('https://uploads.github.com' if upload else 'https://api.github.com') + path
        request = urllib.request.Request(url, data=data, headers=headers, method=method)
        with self.opener.open(request, timeout=60) as response:
            data = response.read(MAX_TOTAL + 1)
        require(len(data) <= MAX_TOTAL)
        return data if raw else json.loads(data, object_pairs_hook=json_object) if data else None

    def get_run(self, run_id, attempt):
        return self.request('GET', f'/repos/{SOURCE_REPOSITORY}/actions/runs/{run_id}/attempts/{attempt}', source=True)

    def get_artifact(self, artifact_id):
        return self.request('GET', f'/repos/{SOURCE_REPOSITORY}/actions/artifacts/{artifact_id}', source=True)

    def download_artifact(self, artifact_id):
        return self.request('GET', f'/repos/{SOURCE_REPOSITORY}/actions/artifacts/{artifact_id}/zip', source=True, raw=True)

    def tip(self, branch):
        require(branch in {'dev', 'main'})
        return self.request('GET', f'/repos/{SOURCE_REPOSITORY}/git/ref/heads/{branch}', source=True)['object']['sha']

    def tag_sha(self, tag):
        require(re.fullmatch(r'v\d+\.\d+\.\d+', tag) is not None)
        obj = self.request('GET', f'/repos/{SOURCE_REPOSITORY}/git/ref/tags/{tag}', source=True)['object']
        for _ in range(4):
            if obj['type'] == 'commit':
                require(SHA.fullmatch(obj['sha']) is not None)
                return obj['sha']
            require(obj['type'] == 'tag' and SHA.fullmatch(obj['sha']) is not None)
            obj = self.request('GET', f"/repos/{SOURCE_REPOSITORY}/git/tags/{obj['sha']}", source=True)['object']
        raise ValueError('Invalid tag object')

    def main_ancestor(self, sha):
        comparison = self.request('GET', f'/repos/{SOURCE_REPOSITORY}/compare/{sha}...main', source=True)
        return comparison.get('status') in {'ahead', 'identical'} and comparison.get('merge_base_commit', {}).get('sha') == sha

    def release(self, tag, create=False):
        require(re.fullmatch(r'dev-(?:latest|[0-9a-f]{40}|stage-[1-9][0-9]*-[1-9][0-9]*-[0-9a-f]{40}|previous-[1-9][0-9]*)|v\d+\.\d+\.\d+', tag) is not None)
        try:
            release = self.request('GET', f'/repos/{RELEASE_REPOSITORY}/releases/tags/{tag}')
        except urllib.error.HTTPError as error:
            if error.code != 404:
                raise
            if not create:
                return None
            release = self.request('POST', f'/repos/{RELEASE_REPOSITORY}/releases', payload=dict(tag_name=tag, target_commitish=os.environ['GITHUB_SHA'], name=tag, body='Exact source provenance is recorded in manifest.json and BUILD_INFO.txt.', draft=False, prerelease=tag.startswith('dev-')))
        require(positive(release['id']) and release['tag_name'] == tag and release['draft'] is False and release['prerelease'] == tag.startswith('dev-'))
        return release

    def asset_metadata(self, release):
        result = []
        for page in range(1, 11):
            batch = self.request('GET', f"/repos/{RELEASE_REPOSITORY}/releases/{release['id']}/assets?per_page=100&page={page}")
            require(isinstance(batch, list))
            result.extend(batch)
            if len(batch) < 100:
                return result
        raise ValueError('Too many assets')

    def release_files(self, release):
        files = {}
        total = 0
        for asset in self.asset_metadata(release):
            require(positive(asset['id']) and asset['name'] not in files and type(asset['size']) is int and 0 < asset['size'] <= MAX_FILE)
            total += asset['size']
            require(total <= MAX_TOTAL)
            data = self.request('GET', f"/repos/{RELEASE_REPOSITORY}/releases/assets/{asset['id']}", raw=True)
            require(len(data) == asset['size'])
            files[asset['name']] = data
        return files

    def list_assets(self, tag):
        release = self.release(tag)
        return self.release_files(release) if release is not None else {}

    def upload(self, tag, name, data):
        require(name == os.path.basename(name) and re.fullmatch(r'[A-Za-z0-9_.-]+', name) is not None)
        release = self.release(tag, create=True)
        self.upload_to_release(release, name, data)

    def upload_to_release(self, release, name, data):
        require(positive(release['id']) and name == os.path.basename(name) and re.fullmatch(r'[A-Za-z0-9_.-]+', name) is not None)
        self.request('POST', f"/repos/{RELEASE_REPOSITORY}/releases/{release['id']}/assets?name={urllib.parse.quote(name)}", payload=data, upload=True)

    @staticmethod
    def file_receipt(files):
        return {name: dict(size=len(data), sha256=hashlib.sha256(data).hexdigest()) for name, data in files.items()}

    def release_id(self, rid):
        require(positive(rid))
        release = self.request('GET', f'/repos/{RELEASE_REPOSITORY}/releases/{rid}')
        require(release['id'] == rid and release['prerelease'] is True and isinstance(release['draft'], bool))
        return release

    def find_stage(self, tag):
        require(re.fullmatch(r'dev-stage-[1-9][0-9]*-[1-9][0-9]*-[0-9a-f]{40}', tag) is not None)
        found = []
        for page in range(1, 11):
            batch = self.request('GET', f'/repos/{RELEASE_REPOSITORY}/releases?per_page=100&page={page}')
            require(isinstance(batch, list))
            found.extend(release for release in batch if release.get('tag_name') == tag)
            require(len(found) <= 1)
            if len(batch) < 100:
                return found[0] if found else None
        raise ValueError('Stage discovery bound exceeded')

    def verify_previous(self, files):
        base = {'dc-exporter-rs', 'BUILD_INFO.txt', 'SHA256SUMS'}
        require(base.issubset(files) and len(files) <= 6)
        if 'manifest.json' in files:
            source = json.loads(files['manifest.json'], object_pairs_hook=json_object)['source']
            stream = io.BytesIO()
            with zipfile.ZipFile(stream, 'w') as archive:
                for name, data in files.items():
                    archive.writestr(name, data)
            validate_archive(stream.getvalue(), source)
        else:
            # First migration preserves existing legacy bytes/metadata without
            # fabricating source identity. Only the known old latest names pass.
            require(set(files) == base)

    def stage_recipe(self, stage):
        require(positive(stage['id']) and stage['prerelease'] is True and isinstance(stage['draft'], bool))
        recipe = json.loads(stage['body'], object_pairs_hook=json_object)
        require(set(recipe) == {'schema', 'kind', 'stage_tag', 'source', 'publisher_sha', 'previous', 'files'})
        require(recipe['schema'] == 1 and recipe['kind'] == 'dc-exporter-latest-stage')
        validate_source(recipe['source'])
        require(recipe['source']['ref'] == 'refs/heads/dev' and SHA.fullmatch(recipe['publisher_sha']) is not None)
        require(re.fullmatch(r'dev-stage-[1-9][0-9]*-[1-9][0-9]*-' + recipe['source']['sha'], recipe['stage_tag']) is not None)
        require(stage['tag_name'] in {recipe['stage_tag'], 'dev-latest'})
        previous = recipe['previous']
        if previous is not None:
            require(set(previous) == {'id', 'tag', 'files'} and positive(previous['id']) and previous['id'] != stage['id'])
            require(previous['tag'] == f"dev-previous-{previous['id']}")
        return recipe

    def patch_alias(self, rid, expected_tag, new_tag, *, draft):
        current = self.release_id(rid)
        require(current['tag_name'] == expected_tag)
        return self.request('PATCH', f'/repos/{RELEASE_REPOSITORY}/releases/{rid}', payload=dict(tag_name=new_tag, draft=draft, prerelease=True))

    def recover_stage(self, stage_id):
        # Compensating recovery intentionally does not require a fresh source
        # tip: staleness must never prevent restoration of the verified old set.
        saved = self.latest_expected_sha
        self.latest_expected_sha = None
        try:
            stage = self.release_id(stage_id)
            recipe = self.stage_recipe(stage)
            latest = self.release('dev-latest')
            previous = recipe['previous']
            allowed = {stage_id} | ({previous['id']} if previous else set())
            require(latest is None or latest['id'] in allowed)
            if previous is None:
                # A first-ever promotion has no old alias to restore. A fully
                # published, verified new set remains complete if already live.
                if latest is not None:
                    require(latest['id'] == stage_id and self.file_receipt(self.release_files(stage)) == recipe['files'])
                return
            old = self.release_id(previous['id'])
            require(old['tag_name'] in {'dev-latest', previous['tag']} and old['draft'] is False)
            require(self.file_receipt(self.release_files(old)) == previous['files'])
            if latest is not None and latest['id'] == stage_id:
                require(self.file_receipt(self.release_files(stage)) == recipe['files'])
                require(self.find_stage(recipe['stage_tag']) is None)
                self.patch_alias(stage_id, 'dev-latest', recipe['stage_tag'], draft=True)
            latest = self.release('dev-latest')
            require(latest is None or latest['id'] == old['id'])
            if latest is None:
                old = self.release_id(old['id'])
                require(old['tag_name'] == previous['tag'] and self.file_receipt(self.release_files(old)) == previous['files'])
                self.patch_alias(old['id'], previous['tag'], 'dev-latest', draft=False)
            restored = self.release('dev-latest')
            require(restored is not None and restored['id'] == old['id'] and self.file_receipt(self.release_files(restored)) == previous['files'])
        finally:
            self.latest_expected_sha = saved

    def replace_latest(self, files):
        source = json.loads(files['manifest.json'], object_pairs_hook=json_object)['source']
        validate_source(source)
        require(source['ref'] == 'refs/heads/dev')
        publisher_sha = os.environ['GITHUB_SHA']
        run, attempt = int(os.environ['GITHUB_RUN_ID']), int(os.environ['GITHUB_RUN_ATTEMPT'])
        require(SHA.fullmatch(publisher_sha) is not None and positive(run) and positive(attempt))
        stage_tag = f"dev-stage-{run}-{attempt}-{source['sha']}"
        stage = None
        self.latest_expected_sha = source['sha']
        try:
            require(self.tip('dev') == source['sha'])
            stage = self.find_stage(stage_tag)
            if stage is not None:
                self.recover_stage(stage['id'])
                stage = self.release_id(stage['id'])
            old = self.release('dev-latest')
            old_files = self.release_files(old) if old is not None else None
            if old_files == files:
                return
            if old_files is not None:
                self.verify_previous(old_files)
            previous = dict(id=old['id'], tag=f"dev-previous-{old['id']}", files=self.file_receipt(old_files)) if old is not None else None
            recipe = dict(schema=1, kind='dc-exporter-latest-stage', stage_tag=stage_tag, source=source, publisher_sha=publisher_sha, previous=previous, files=self.file_receipt(files))
            if stage is None:
                stage = self.request('POST', f'/repos/{RELEASE_REPOSITORY}/releases', payload=dict(tag_name=stage_tag, target_commitish=publisher_sha, name=stage_tag, body=json.dumps(recipe, sort_keys=True), draft=True, prerelease=True))
            require(stage['tag_name'] == stage_tag and stage['draft'] is True and self.stage_recipe(stage) == recipe)
            staged_files = self.release_files(stage)
            require(set(staged_files).issubset(files) and all(files[name] == data for name, data in staged_files.items()))
            for name, data in sorted(files.items()):
                if name not in staged_files:
                    self.upload_to_release(stage, name, data)
            require(self.release_files(stage) == files and self.tip('dev') == source['sha'])
            latest = self.release('dev-latest')
            require((latest is None and old is None) or latest is not None and old is not None and latest['id'] == old['id'] and self.release_files(latest) == old_files)
            if old is not None:
                require(self.release(previous['tag']) is None)
                self.patch_alias(old['id'], 'dev-latest', previous['tag'], draft=False)
            require(self.tip('dev') == source['sha'] and self.release('dev-latest') is None)
            require(self.release_files(self.release_id(stage['id'])) == files)
            self.patch_alias(stage['id'], stage_tag, 'dev-latest', draft=False)
            latest = self.release('dev-latest')
            require(latest is not None and latest['id'] == stage['id'] and self.release_files(latest) == files and self.tip('dev') == source['sha'])
        except Exception:
            if stage is not None:
                try:
                    self.recover_stage(stage['id'])
                except Exception:
                    # Verified bytes and recovery metadata remain on reserved
                    # releases. Persistent API outages can leave an alias 404
                    # window; a protected manual recovery dispatch repairs it.
                    pass
            raise
        finally:
            self.latest_expected_sha = None


# TRACEWEAVER: entrypoint=main; req=REQ-PUB-006; trace=TRACE-PUB-006; ver=VER-PUB-006
def main(argv=None, *, api=None):
    parser = PublicParser()
    parser.add_argument('--preflight', action='store_true')
    parser.add_argument('--recover-stage-id', type=int)
    parser.add_argument('--run-id', type=int)
    parser.add_argument('--run-attempt', type=int)
    parser.add_argument('--artifact-id', type=int)
    parser.add_argument('--sha')
    parser.add_argument('--branch', choices=['dev', 'main'])
    parser.add_argument('--tag', default='')
    try:
        args = parser.parse_args(argv)
        if args.preflight:
            require(all(value is None for value in [args.run_id, args.run_attempt, args.artifact_id, args.sha, args.branch, args.recover_stage_id]) and not args.tag)
            preflight()
            return 0
        default = os.environ['GITHUB_DEFAULT_BRANCH']
        require(re.fullmatch(r'[A-Za-z0-9_.-]+', default) is not None)
        require(os.environ['GITHUB_REPOSITORY'] == RELEASE_REPOSITORY and os.environ['GITHUB_EVENT_NAME'] == 'workflow_dispatch')
        require(os.environ['GITHUB_REF'] == 'refs/heads/' + default and os.environ['GITHUB_WORKFLOW_REF'] == f'{RELEASE_REPOSITORY}/.github/workflows/publish.yml@refs/heads/{default}')
        if args.recover_stage_id is not None:
            require(positive(args.recover_stage_id) and all(value is None for value in [args.run_id, args.run_attempt, args.artifact_id, args.sha, args.branch]) and not args.tag)
            if api is None:
                preflight()
                api = GithubAPI(target_only=True)
            api.recover_stage(args.recover_stage_id)
            print('Publisher recovery verified a complete asset set')
            return 0
        workflow_id = int(os.environ['SOURCE_WORKFLOW_ID'])
        require(positive(workflow_id))
        require(positive(args.run_id) and positive(args.run_attempt) and positive(args.artifact_id) and args.branch in {'dev', 'main'})
        source = dict(repository=SOURCE_REPOSITORY, repository_id=SOURCE_REPOSITORY_ID, sha=args.sha, ref='refs/heads/' + args.branch, run_id=args.run_id, run_attempt=args.run_attempt, artifact_name=f'dc-exporter-{SOURCE_REPOSITORY_ID}-{args.run_id}-{args.run_attempt}-{args.sha}')
        validate_source(source)
        if api is None:
            preflight()
            api = GithubAPI()
        run = api.get_run(args.run_id, args.run_attempt)
        artifact = api.get_artifact(args.artifact_id)
        validate_receipt(run, artifact, expected=source, workflow_id=workflow_id, artifact_id=args.artifact_id, tip=api.tip(args.branch))
        files, manifest = validate_archive(api.download_artifact(args.artifact_id), source)
        if args.branch == 'main':
            # Fail before tag/history API work and publication until acceptance.
            require(HARDWARE_ACCEPTED)
            validate_stable(source, args.tag, manifest['cargo_version'], tag_sha=api.tag_sha(args.tag), main_ancestor=api.main_ancestor(args.sha), hardware_accepted=HARDWARE_ACCEPTED)
            require(api.tip('main') == args.sha)
            publish_immutable(api, args.tag, files)
        else:
            require(not args.tag and api.tip('dev') == args.sha)
            publish_immutable(api, 'dev-' + args.sha, files)
            promote_latest(api, source, files)
        print('Publisher verified exact artifact bytes')
        return 0
    except Exception:
        print(public_failure_messages()[0], file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
