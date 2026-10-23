#!/usr/bin/env python3
# TRACEWEAVER: file-role=public-pool-artifact-manifest; req=REQ-PUB-003; trace=TRACE-PUB-003; ver=VER-PUB-003
"""Bind the exact distribution bytes to the trusted public-source build run."""
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import tomllib

REPOSITORY = 'cryptolabsza/dc-exporter-rs-public'
REPOSITORY_ID = 1416116330
MAX_FILE = 128 * 1024 * 1024
MAX_TOTAL = 200 * 1024 * 1024


def runner_group(repository):
    private = repository.get('private')
    visibility = repository.get('visibility')
    if (type(repository.get('id')) is not int or repository['id'] != REPOSITORY_ID
            or repository.get('full_name') != REPOSITORY or type(private) is not bool
            or visibility not in {None, 'private' if private else 'public'}):
        raise ValueError('Unknown or inconsistent authoritative repository visibility')
    return 'ci-build-ephemeral' if private else 'ci-build-public-ephemeral'


def assets(version):
    return {'dc-exporter-rs', 'BUILD_INFO.txt', 'SHA256SUMS', 'VERSION.txt',
            'HELP.txt', 'SYMBOLS.txt', 'THIRD-PARTY-NOTICES.md',
            'dc-exporter-rs_amd64.deb', f'dc-exporter-rs_{version}_amd64.deb'}


# TRACEWEAVER: entrypoint=manifest; req=REQ-PUB-003; trace=TRACE-PUB-003; ver=VER-PUB-003
def manifest(dist, cargo, env):
    if (env.get('GITHUB_REPOSITORY') != REPOSITORY
            or env.get('GITHUB_REPOSITORY_ID') != str(REPOSITORY_ID)
            or not re.fullmatch(r'[0-9a-f]{40}', env.get('GITHUB_SHA', ''))):
        raise ValueError('Public source identity mismatch')
    if (env.get('GITHUB_REF') not in {'refs/heads/dev', 'refs/heads/main'}
            or env.get('GITHUB_EVENT_NAME') not in {'push', 'workflow_dispatch'}
            or env.get('GITHUB_REF_PROTECTED') != 'true'):
        raise ValueError('Untrusted source event or ref')
    for key in ['GITHUB_RUN_ID', 'GITHUB_RUN_ATTEMPT']:
        if not re.fullmatch(r'[1-9][0-9]*', env.get(key, '')):
            raise ValueError('Invalid run identity')
    version = tomllib.loads(cargo.read_text())['package']['version']
    if not re.fullmatch(r'\d+\.\d+\.\d+', version):
        raise ValueError('Invalid Cargo version')
    if {p.name for p in dist.iterdir()} != assets(version):
        raise ValueError('Unexpected artifact files')
    files = {}
    total = 0
    for path in sorted(dist.iterdir()):
        info = path.lstat()
        if (not stat.S_ISREG(info.st_mode) or info.st_nlink != 1
                or not 0 < info.st_size <= MAX_FILE):
            raise ValueError('Invalid artifact bytes')
        total += info.st_size
        if total > MAX_TOTAL:
            raise ValueError('Artifact total exceeds limit')
        data = path.read_bytes()
        files[path.name] = {'size': len(data), 'sha256': hashlib.sha256(data).hexdigest()}
    if (dist / 'VERSION.txt').read_text() != f'dc-exporter {version}\n':
        raise ValueError('Cargo and binary version mismatch')
    rows = (dist / 'SHA256SUMS').read_text().splitlines()
    expected = assets(version) - {'SHA256SUMS'}
    seen = set()
    for row in rows:
        match = re.fullmatch(r'([0-9a-f]{64})  ([A-Za-z0-9_.-]+)', row)
        if not match or match[2] not in expected or match[2] in seen:
            raise ValueError('Invalid checksum rows')
        seen.add(match[2])
        if files[match[2]]['sha256'] != match[1]:
            raise ValueError('Checksum mismatch')
    if seen != expected:
        raise ValueError('Incomplete checksum rows')
    source = {'repository': REPOSITORY, 'repository_id': REPOSITORY_ID,
              'sha': env['GITHUB_SHA'], 'ref': env['GITHUB_REF'],
              'run_id': int(env['GITHUB_RUN_ID']), 'run_attempt': int(env['GITHUB_RUN_ATTEMPT']),
              'artifact_name': f"dc-exporter-{REPOSITORY_ID}-{env['GITHUB_RUN_ID']}-{env['GITHUB_RUN_ATTEMPT']}-{env['GITHUB_SHA']}"}
    result = {'schema': 1, 'source': source, 'cargo_version': version,
              'binary_version': version, 'files': files}
    if total + len((json.dumps(result, sort_keys=True, indent=2) + '\n').encode()) > MAX_TOTAL:
        raise ValueError('Artifact total including manifest exceeds limit')
    return result


if __name__ == '__main__':
    import sys
    if sys.argv[1:] == ['runner-group']:
        event = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())
        print(runner_group(event['repository']))
    elif sys.argv[1:]:
        raise SystemExit('Unsupported command')
    else:
        result = manifest(Path('dist'), Path('Cargo.toml'), os.environ)
        Path('dist/manifest.json').write_text(json.dumps(result, sort_keys=True, indent=2) + '\n')
