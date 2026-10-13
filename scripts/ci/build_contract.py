#!/usr/bin/env python3
# TRACEWEAVER: file-role=native-ci-build; req=REQ-PUB-003; trace=TRACE-PUB-003; ver=VER-PUB-003
"""Fail-closed native build preflight and exact-byte artifact manifest."""
import argparse
import grp
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import sys
import tomllib

SOURCE_REPOSITORY = 'cryptolabsza/dc-exporter-rs-public'
SOURCE_REPOSITORY_ID = 1416116330
VERSION = re.compile(r'\d+\.\d+\.\d+\Z')


def validate_host(*, uid, euid, groups, devices, sockets, python, sudo, capabilities):
    forbidden = {'root', 'sudo', 'wheel', 'docker', 'libvirt', 'libvirt-qemu', 'lxd', 'disk', 'kmem'}
    if uid == 0 or euid == 0 or forbidden.intersection(groups) or devices or sockets or not python or sudo or capabilities:
        raise ValueError('Native build isolation preflight failed')


def validate_glibc(symbols):
    versions = re.findall(r'GLIBC_(\d+)\.(\d+)(?:\.(\d+))?', symbols)
    if not versions or 'GLIBC_PRIVATE' in symbols or any(tuple(int(n or 0) for n in version) > (2, 34, 0) for version in versions):
        raise ValueError('GLIBC compatibility gate failed')


def parse_binary_version(output):
    match = re.fullmatch(r'dc-exporter (\d+\.\d+\.\d+)\n?', output)
    if match is None:
        raise ValueError('Invalid binary version')
    return match[1]


def validate_lock(directory, *, tracked):
    path = Path(directory, 'Cargo.lock')
    if not tracked or not path.exists() or not stat.S_ISREG(path.lstat().st_mode):
        raise ValueError('Tracked dependency lock required')
    lock = tomllib.loads(path.read_text())
    if lock.get('version') not in {3, 4} or not lock.get('package'):
        raise ValueError('Invalid dependency lock')


def cgroup_directory(text):
    lines = [line[3:] for line in text.splitlines() if line.startswith('0::')]
    if len(lines) != 1 or not lines[0].startswith('/') or '..' in lines[0].split('/'):
        raise ValueError('Missing cgroup hierarchy')
    return Path('/sys/fs/cgroup', lines[0].lstrip('/'))


def validate_resource_bounds(memory, pids, *, memory_limit, pids_limit):
    for value, limit in [(memory, memory_limit), (pids, pids_limit)]:
        if not value.isdecimal() or not 0 < int(value) <= limit:
            raise ValueError('Native runner cgroup boundary failed')


def validate_cgroup_hierarchy(levels, *, memory_limit, pids_limit):
    effective = {}
    for name in ['memory.max', 'pids.max']:
        values = []
        for level in levels:
            value = level.get(name)
            if value != 'max' and (not isinstance(value, str) or not value.isdecimal() or int(value) <= 0):
                raise ValueError('Missing cgroup evidence')
            if value != 'max':
                values.append(int(value))
        if not values:
            raise ValueError('Unbounded cgroup hierarchy')
        effective[name] = str(min(values))
    validate_resource_bounds(effective['memory.max'], effective['pids.max'], memory_limit=memory_limit, pids_limit=pids_limit)


def resource_levels():
    root = Path('/sys/fs/cgroup')
    current = cgroup_directory(Path('/proc/self/cgroup').read_text())
    levels = []
    while True:
        # The kernel root cgroup can omit limit files because it is unbounded.
        # Missing evidence on a non-root service/parent remains a failure.
        levels.append({name: 'max' if current == root and not (current / name).exists() else (current / name).read_text().strip() for name in ['memory.max', 'pids.max']})
        if current == root:
            return levels
        current = current.parent


def asset_names(version):
    return {'dc-exporter-rs', 'BUILD_INFO.txt', 'SHA256SUMS', 'dc-exporter-rs_amd64.deb', f'dc-exporter-rs_{version}_amd64.deb'}


def make_manifest(directory, *, repository, repository_id, sha, ref, run_id, run_attempt, artifact_name, cargo_version, binary_version):
    if repository != SOURCE_REPOSITORY or repository_id != SOURCE_REPOSITORY_ID or not re.fullmatch('[0-9a-f]{40}', sha) or ref not in {'refs/heads/dev', 'refs/heads/main'}:
        raise ValueError('Source identity mismatch')
    if any(type(value) is not int or value <= 0 for value in [run_id, run_attempt]):
        raise ValueError('Invalid run identity')
    if artifact_name != f'dc-exporter-{repository_id}-{run_id}-{run_attempt}-{sha}' or not VERSION.fullmatch(cargo_version) or cargo_version != binary_version:
        raise ValueError('Build identity mismatch')
    files = {}
    for path in Path(directory).iterdir():
        if path.name not in asset_names(cargo_version) or not stat.S_ISREG(path.lstat().st_mode):
            raise ValueError('Unexpected artifact file')
        data = path.read_bytes()
        if not data or len(data) > 128 * 1024 * 1024:
            raise ValueError('Invalid artifact size')
        files[path.name] = {'size': len(data), 'sha256': hashlib.sha256(data).hexdigest()}
    if not {'dc-exporter-rs', 'BUILD_INFO.txt', 'SHA256SUMS'}.issubset(files):
        raise ValueError('Missing artifact file')
    packages = asset_names(cargo_version) - {'dc-exporter-rs', 'BUILD_INFO.txt', 'SHA256SUMS'}
    if packages.intersection(files) and not packages.issubset(files):
        raise ValueError('Incomplete package assets')
    return dict(schema=1, source=dict(repository=repository, repository_id=repository_id, sha=sha, ref=ref, run_id=run_id, run_attempt=run_attempt, artifact_name=artifact_name), cargo_version=cargo_version, binary_version=binary_version, files=files)


def preflight():
    if sys.platform != 'linux':
        raise ValueError('Native build requires Linux')
    status = Path('/proc/self/status').read_text()
    capabilities = 0
    for name in ['CapEff', 'CapPrm', 'CapAmb']:
        match = re.search(rf'^{name}:\s*([0-9a-f]+)$', status, re.M)
        if match is None:
            raise ValueError('Missing capability evidence')
        capabilities |= int(match[1], 16)
    devices = list(Path('/dev').glob('nvidia*')) + [path for path in [Path('/dev/mem'), Path('/dev/kmem'), Path('/dev/dri')] if path.exists()]
    sockets = [path for path in [Path('/var/run/docker.sock'), Path('/run/containerd/containerd.sock'), Path('/var/run/libvirt/libvirt-sock'), Path('/var/snap/lxd/common/lxd/unix.socket')] if path.exists()]
    validate_host(uid=os.getuid(), euid=os.geteuid(), groups=[grp.getgrgid(gid).gr_name for gid in set(os.getgroups() + [os.getgid()])], devices=devices, sockets=sockets, python=os.access('/usr/bin/python3', os.X_OK), sudo=shutil.which('sudo') is not None, capabilities=capabilities)
    for tool in ['cargo', 'rustc', 'cc', 'strip', 'objdump', 'dpkg-deb', 'git']:
        if shutil.which(tool) is None:
            raise ValueError('Preinstalled native toolchain missing')
    temporary = Path(os.environ['RUNNER_TEMP'])
    if temporary.is_symlink() or not temporary.is_dir() or temporary.stat().st_uid != os.getuid():
        raise ValueError('Invalid owned temporary directory')
    if shutil.disk_usage(temporary).free < 2 * 1024**3 or (os.cpu_count() or 999) > 4:
        raise ValueError('Native runner resource boundary failed')
    # VM CPU/disk/network bounds remain operator acceptance evidence. A hard
    # memory and process cgroup bound is also required at workflow execution.
    validate_cgroup_hierarchy(resource_levels(), memory_limit=8 * 1024**3, pids_limit=256)


# TRACEWEAVER: entrypoint=main; req=REQ-PUB-003; trace=TRACE-PUB-003; ver=VER-PUB-003
def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('command', choices=['preflight', 'glibc', 'manifest', 'version', 'lock'])
    parser.add_argument('--dist')
    parser.add_argument('--symbols')
    parser.add_argument('--cargo-toml')
    parser.add_argument('--binary-version')
    args = parser.parse_args()
    try:
        if args.command == 'preflight':
            preflight()
        elif args.command == 'glibc':
            validate_glibc(Path(args.symbols).read_text())
        elif args.command == 'version':
            print(parse_binary_version(args.binary_version))
        elif args.command == 'lock':
            validate_lock(Path.cwd(), tracked=True)
        else:
            version = tomllib.loads(Path(args.cargo_toml).read_text())['package']['version']
            manifest = make_manifest(Path(args.dist), repository=os.environ['GITHUB_REPOSITORY'], repository_id=int(os.environ['GITHUB_REPOSITORY_ID']), sha=os.environ['GITHUB_SHA'], ref=os.environ['GITHUB_REF'], run_id=int(os.environ['GITHUB_RUN_ID']), run_attempt=int(os.environ['GITHUB_RUN_ATTEMPT']), artifact_name=os.environ['ARTIFACT_NAME'], cargo_version=version, binary_version=args.binary_version)
            Path(args.dist, 'manifest.json').write_text(json.dumps(manifest, sort_keys=True, indent=2) + '\n')
        return 0
    except Exception:
        print('::error::Native build contract failed', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
