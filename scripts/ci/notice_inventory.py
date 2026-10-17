#!/usr/bin/env python3
# TRACEWEAVER: file-role=locked-licence-inventory; req=REQ-PUB-007; trace=TRACE-PUB-007; ver=VER-PUB-007
"""Collect actual licence/notice files for the locked Linux non-dev closure."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import tomllib

TARGET = 'x86_64-unknown-linux-gnu'
METADATA_COMMAND = ['cargo', 'metadata', '--locked', '--format-version', '1',
                    '--filter-platform', TARGET]


def selected_packages(metadata, lock):
    """Follow normal and build edges from the actual workspace package root."""
    resolve = metadata.get('resolve')
    if metadata.get('version') != 1 or not isinstance(resolve, dict):
        raise ValueError('Cargo metadata v1 with resolved dependencies is required')
    packages = metadata.get('packages', [])
    nodes = resolve.get('nodes', [])
    by_id = {p['id']: p for p in packages}
    by_node = {p['id']: p for p in nodes}
    root = resolve.get('root')
    if (len(by_id) != len(packages) or len(by_node) != len(nodes)
            or root not in by_id or root not in by_node
            or metadata.get('workspace_members') != [root]):
        raise ValueError('Ambiguous or incomplete Cargo workspace graph')
    locked = {(p['name'], p['version'], p.get('source')) for p in lock['package']}
    selected, pending = {}, [root]
    while pending:
        identity = pending.pop()
        if identity in selected:
            continue
        if identity not in by_id or identity not in by_node:
            raise ValueError('Selected dependency package or resolve node is missing')
        package = by_id[identity]
        key = (package['name'], package['version'], package.get('source'))
        if key not in locked or not identity.endswith(f"#{key[0]}@{key[1]}"):
            raise ValueError('Selected Cargo metadata identity differs from Cargo.lock')
        if identity == root:
            if key[2] is not None or not identity.startswith('path+'):
                raise ValueError('Workspace root identity mismatch')
        elif not key[2] or not key[2].startswith('registry+') or not identity.startswith(key[2] + '#'):
            raise ValueError('Unresolved non-registry dependency source')
        selected[identity] = package
        node = by_node[identity]
        deps = node.get('deps')
        if not isinstance(deps, list) or set(node.get('dependencies', [])) != {d['pkg'] for d in deps}:
            raise ValueError('Incomplete resolved dependency edge metadata')
        for dependency in deps:
            kinds = dependency.get('dep_kinds')
            if not kinds or any(k.get('kind') not in {None, 'build', 'dev'} for k in kinds):
                raise ValueError('Unresolved dependency edge kind')
            if any(k.get('kind') != 'dev' for k in kinds):
                pending.append(dependency['pkg'])
    return root, [selected[key] for key in sorted(selected)]


def licence_files(package):
    expression = package.get('license')
    explicit = package.get('license_file')
    if not expression and not explicit:
        raise ValueError('Selected package has unresolved licensing')
    if expression and (not isinstance(expression, str) or not expression.strip()
                       or expression.strip().upper() in {'NOASSERTION', 'NONE', 'UNLICENSED'}
                       or 'LicenseRef-' in expression or not re.fullmatch(r'[A-Za-z0-9.()+ :/-]+', expression)):
        raise ValueError('Unresolved licence expression')
    root = Path(package['manifest_path']).parent.resolve(strict=True)
    selected = set()
    if explicit:
        path = (root / explicit).resolve(strict=True)
        if not path.is_relative_to(root):
            raise ValueError('Licence file escapes selected crate')
        selected.add(path)
    # Include multiple alternative licences and attribution files; keep exact text.
    # Some crates place attribution beneath a licences directory.
    for path in root.iterdir():
        if path.is_file() and re.match(r'^(LICENSE|LICENCE|COPYING|NOTICE|COPYRIGHT)(?:[._-]|$)', path.name, re.I):
            selected.add(path)
        elif path.is_dir() and path.name.lower() in {'licenses', 'licences'}:
            selected.update(p for p in path.rglob('*') if p.is_file())
    if not selected:
        raise ValueError('Selected dependency is missing actual licence files')
    result = []
    for path in sorted(selected, key=lambda p: str(p.relative_to(root))):
        if path.is_symlink() or not path.resolve().is_relative_to(root) or not 0 < path.stat().st_size <= 1024 * 1024:
            raise ValueError('Invalid dependency licence file')
        data = path.read_bytes()
        try:
            text = data.decode('utf-8')
        except UnicodeDecodeError as error:
            raise ValueError('Dependency licence must be UTF-8') from error
        if not text.strip() or '\x00' in text:
            raise ValueError('Empty or invalid dependency licence text')
        result.append({'path': str(path.relative_to(root)), 'sha256': hashlib.sha256(data).hexdigest(), 'text': text})
    return result


def collect(metadata, lock, project_notice):
    root, selected = selected_packages(metadata, lock)
    if not project_notice.strip() or any(token not in project_notice for token in ['Copyright (c) CryptoLabs ZA', 'NVIDIA CORPORATION', 'Copyright (c) 2026 Philip Langdale.']):
        raise ValueError('Project/adaptation notice is incomplete')
    records = []
    for package in sorted(selected, key=lambda p: (p['name'], p['version'], p.get('source') or '')):
        if package['id'] == root:
            continue
        records.append({'id': package['id'], 'name': package['name'], 'version': package['version'],
                        'source': package['source'], 'license': package.get('license'),
                        'files': licence_files(package)})
    text = project_notice.rstrip() + '\n\n# Locked Linux dependency notices\n\n'
    text += 'Target: `' + TARGET + '`. Default features; normal and build dependencies; dev dependencies excluded.\n'
    for package in records:
        text += f"\n## Runtime dependency: {package['name']} {package['version']}\n\n"
        text += f"Source: `{package['source']}`\n\nDeclared licence: `{package['license'] or 'licence-file'}`\n"
        for item in package['files']:
            text += '\n### ' + item['path'] + '\n\n' + item['text'].rstrip() + '\n'
    # File-system locations from metadata must never enter a public notice asset.
    if any(package['manifest_path'] in text for package in selected):
        raise ValueError('Operator path leaked into licence text')
    return text, records, root


# TRACEWEAVER: entrypoint=generate; req=REQ-PUB-007; trace=TRACE-PUB-007; ver=VER-PUB-007
def generate(metadata, lock, project_notice):
    return collect(metadata, lock, project_notice)[0]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--metadata', type=Path, required=True)
    parser.add_argument('--lock', type=Path, default=Path('Cargo.lock'))
    parser.add_argument('--project-notice', type=Path, default=Path('THIRD-PARTY-NOTICES.md'))
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--inventory', type=Path, required=True)
    args = parser.parse_args()
    raw = args.metadata.read_bytes()
    metadata = json.loads(raw)
    lock_bytes = args.lock.read_bytes()
    text, records, root = collect(metadata, tomllib.loads(lock_bytes.decode()), args.project_notice.read_text())
    # Preserve the actual filtered graph for an independent expected-set walk,
    # while removing machine-specific paths and Cargo filesystem directories.
    public_id = lambda value: 'workspace:dc-exporter' if value == root else value
    evidence = {'schema': 1, 'target': TARGET, 'features': 'default', 'profile': 'release',
                'metadata_command': METADATA_COMMAND, 'metadata_sha256': hashlib.sha256(raw).hexdigest(),
                'cargo_lock_sha256': hashlib.sha256(lock_bytes).hexdigest(),
                'root': public_id(root),
                'packages': [{'id': public_id(p['id']), 'name': p['name'], 'version': p['version'], 'source': p.get('source')} for p in metadata['packages']],
                'nodes': [{'id': public_id(n['id']), 'deps': [{'pkg': public_id(d['pkg']), 'dep_kinds': d['dep_kinds']} for d in n['deps']]} for n in metadata['resolve']['nodes']],
                'selected': [{key: value for key, value in record.items() if key != 'files'} | {'files': [{k: v for k, v in f.items() if k != 'text'} for f in record['files']]} for record in records],
                'notice_sha256': hashlib.sha256(text.encode()).hexdigest()}
    args.output.write_text(text)
    args.inventory.parent.mkdir(parents=True, exist_ok=True)
    args.inventory.write_text(json.dumps(evidence, sort_keys=True, indent=2) + '\n')


if __name__ == '__main__':
    main()
