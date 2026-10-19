#!/usr/bin/env python3
# TRACEWEAVER: file-role=public-commit-tree-export; req=REQ-PUB-002; trace=TRACE-PUB-002; ver=VER-PUB-002
"""Export approved Git blobs without history. Approval/receipts stay outside the tree."""

import argparse
import hashlib
import ipaddress
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import tempfile
import tomllib


# Literal selection: adding a public path requires review of this policy and approval.
PUBLIC_PATHS = frozenset("""
.github/workflows/build.yml .gitignore Cargo.toml Cargo.lock LICENSE Makefile README.md
RELEASING.md TESTING.md build.rs config.toml.example
docs/gpu-thermal-abi.md docs/gpu-thermal-support.md docs/public-publishing.md docs/gpu-health.md
docs/validation/rtx5090-acceptance.md docs/fixtures/rusd-abi-layout.c
docs/validation/vm-passthrough-acceptance.md
scripts/build-release.sh scripts/deploy.sh scripts/public_export.py
scripts/ci/build_contract.py scripts/ci/build.sh scripts/ci/publish.py
scripts/ci/rusd_abi.py scripts/ci/rusd-headers.json
src/config.rs src/main.rs src/safe_nvml.rs src/state.rs
src/metrics/collector.rs src/metrics/dmesg.rs src/metrics/gpu.rs src/metrics/mod.rs
src/metrics/nvidia_rm/abi.rs src/metrics/nvidia_rm/mod.rs src/metrics/nvidia_rm/rusd.rs
src/metrics/pci.rs src/metrics/prometheus.rs src/metrics/register.rs
src/metrics/thermal.rs src/metrics/thermal_worker.rs src/metrics/vm.rs
src/tui/app.rs src/tui/mod.rs src/tui/ui.rs systemd/dc-exporter.service
tests/nvidia_rm_abi.rs tests/nvidia_rm_rusd.rs tests/release_workflow.rs
tests/thermal_policy.rs tests/thermal_worker.rs tests/test_public_export.py
tests/test_ci_build.py tests/test_ci_publisher.py
tests/test_ci_rusd_abi.py
.github/workflows/ci-pool-build-recipe.yml scripts/ci/pool-build.sh scripts/ci/pool_manifest.py
scripts/ci/notice_inventory.py tests/test_pool_workflow.py tests/test_ci_notices.py
CONTRIBUTING.md SECURITY.md THIRD-PARTY-NOTICES.md
""".split())

REVIEWED_CHANGE_PATHS = frozenset({
    ".github/workflows/ci-pool-build-recipe.yml", "scripts/ci/pool-build.sh", "scripts/ci/pool_manifest.py",
    "scripts/ci/notice_inventory.py", "tests/test_pool_workflow.py", "tests/test_ci_notices.py",
    "CONTRIBUTING.md", "SECURITY.md", "THIRD-PARTY-NOTICES.md", "systemd/dc-exporter.service",
    ".github/workflows/build.yml", ".gitignore", "Cargo.lock", "LICENSE", "README.md", "RELEASING.md", "TESTING.md",
    "docs/gpu-thermal-abi.md", "docs/gpu-thermal-support.md", "docs/public-publishing.md", "docs/gpu-health.md",
    "docs/validation/rtx5090-acceptance.md", "docs/fixtures/rusd-abi-layout.c",
    "docs/validation/vm-passthrough-acceptance.md",
    "src/metrics/collector.rs", "src/metrics/vm.rs", "src/metrics/pci.rs",
    "scripts/public_export.py", "scripts/ci/build_contract.py", "scripts/ci/build.sh",
    "scripts/ci/publish.py", "tests/release_workflow.rs", "tests/test_public_export.py",
    "scripts/ci/rusd_abi.py", "scripts/ci/rusd-headers.json", "tests/test_ci_rusd_abi.py",
    "tests/test_ci_build.py", "tests/test_ci_publisher.py", "Cargo.toml", "scripts/deploy.sh",
})
SCHEMA = "public-snapshot/1"
SHA = re.compile(r"[0-9a-f]{40}\Z")
HASH = re.compile(r"[0-9a-f]{64}\Z")


class ExportError(ValueError):
    """A proposed tree violates the pinned public-export contract."""


def git(repo, *args):
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    result = subprocess.run(["git", "--no-replace-objects", "-C", str(repo), *args],
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
    if result.returncode:
        raise ExportError("Git commit/blob lookup failed")
    return result.stdout


def commit_id(repo, commit):
    if not isinstance(commit, str) or not SHA.fullmatch(commit):
        raise ExportError("A full immutable commit SHA is required")
    resolved = git(repo, "rev-parse", "--verify", commit + "^{commit}").decode().strip()
    if resolved != commit:
        raise ExportError("Commit must identify a commit object directly")
    return commit


def tree(repo, commit):
    commit_id(repo, commit)
    entries = {}
    for record in git(repo, "ls-tree", "-rz", "--full-tree", commit).split(b"\0"):
        if not record:
            continue
        metadata, raw_path = record.split(b"\t", 1)
        mode, kind, blob = metadata.decode("ascii").split()
        try:
            path = raw_path.decode("utf-8")
        except UnicodeDecodeError as error:
            raise ExportError("Non-UTF8 tracked path") from error
        entries[path] = (mode, kind, blob)
    return entries


# TRACEWEAVER: entrypoint=validate_path; req=REQ-PUB-007; trace=TRACE-PUB-007; ver=VER-PUB-007
def validate_path(path):
    if (not isinstance(path, str) or path not in PUBLIC_PATHS or "\\" in path
            or path.startswith("/") or str(PurePosixPath(path)) != path
            or any(part in ("", ".", "..") for part in path.split("/"))):
        raise ExportError("Path is outside the literal public allowlist")


def validate_approval(approval):
    expected = {"schema", "commit", "baseline_commit", "files", "changes"}
    if not isinstance(approval, dict) or set(approval) != expected or approval["schema"] != SCHEMA:
        raise ExportError("Approval schema or fields invalid; history/ref fields are forbidden")
    if not isinstance(approval["files"], dict) or not approval["files"]:
        raise ExportError("Approval must contain exact file hashes and modes")
    if not isinstance(approval["changes"], dict):
        raise ExportError("Changed paths need explicit reviewed reasons")
    for path, expected_file in approval["files"].items():
        validate_path(path)
        if (not isinstance(expected_file, dict) or set(expected_file) != {"sha256", "mode"}
                or not isinstance(expected_file["sha256"], str)
                or not HASH.fullmatch(expected_file["sha256"])
                or expected_file["mode"] not in ("100644", "100755")):
            raise ExportError("Invalid approved hash or mode")
    for path, reason in approval["changes"].items():
        if (path not in approval["files"] or path not in REVIEWED_CHANGE_PATHS
                or not isinstance(reason, str) or not reason.strip()):
            raise ExportError("Invalid changed-path approval")


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def public_content(path, data):
    # Marker checks complement credential scanning and owner disclosure review.
    text = data.decode("utf-8", errors="replace")
    if any(marker in text for marker in ("/Us" + "ers/", "/ho" + "me/",
                                        "-----BEGIN " + "OPENSSH PRIVATE KEY-----",
                                        "-----BEGIN " + "RSA PRIVATE KEY-----",
                                        "-----BEGIN " + "EC PRIVATE KEY-----",
                                        "-----BEGIN " + "PRIVATE KEY-----")):
        raise ExportError("Private content marker in selected file: " + path)
    for match in re.finditer(r"(?<![\w.])(?:[0-9]{1,3}\.){3}[0-9]{1,3}(?![\w.])", text):
        try:
            address = ipaddress.ip_address(match.group())
        except ValueError:
            continue
        if not address.is_loopback and not address.is_unspecified:
            raise ExportError("Operator address marker in selected file: " + path)


def repository_url_only(before, after):
    line = re.compile(rb'^repository\s*=\s*"[^"\r\n]+"\s*$', re.MULTILINE)
    return (len(line.findall(before)) == len(line.findall(after)) == 1
            and line.sub(b"repository = PUBLIC", before) == line.sub(b"repository = PUBLIC", after)
            and b'repository = "https://github.com/cryptolabsza/dc-exporter-rs-public"' in after)


def pinned_vm_dependency_only(before, after):
    # Closed VM-FIX-006 exception: only this reviewed dependency block may be
    # added. Every other byte still obeys the existing repository-URL rule.
    addition = (b'\n# Strict libvirt XML parsing; encoding/transcoding and entity resolvers disabled.\n'
                b'quick-xml = { version = "=0.42.0", default-features = false }\n')
    if after.count(addition) != 1:
        return False
    try:
        old = tomllib.loads(before.decode('utf-8'))
        new = tomllib.loads(after.decode('utf-8'))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError):
        return False
    return ('quick-xml' not in old.get('dependencies', {})
            and new.get('dependencies', {}).get('quick-xml') == {
                'version': '=0.42.0', 'default-features': False}
            and repository_url_only(before, after.replace(addition, b'', 1)))


def approved_blobs(repo, commit, approval):
    validate_approval(approval)
    commit_id(repo, commit)
    if approval["commit"] != commit:
        raise ExportError("Approval belongs to a different commit")
    current = tree(repo, commit)
    baseline = tree(repo, approval["baseline_commit"])
    if any(path.startswith(("src/", "tests/")) and path not in PUBLIC_PATHS
           for path in set(current) | set(baseline)):
        raise ExportError("Unknown production/test path requires public selection review")
    selected = set(current) & PUBLIC_PATHS
    if set(approval["files"]) != selected:
        raise ExportError("Approval omits required selected production/test/package files")
    if (set(baseline) & PUBLIC_PATHS) - selected:
        raise ExportError("Public baseline files were removed")
    blobs = {}
    changed = set()
    for path in sorted(selected):
        mode, kind, blob = current[path]
        if kind != "blob" or mode not in ("100644", "100755"):
            raise ExportError("Symlinks/submodules are forbidden in the public tree")
        data = git(repo, "cat-file", "blob", blob)
        if len(data) > 8 * 1024 * 1024:
            raise ExportError("Selected file exceeds export size limit")
        if approval["files"][path] != {"sha256": sha256(data), "mode": mode}:
            raise ExportError("Commit blob bytes/mode differ from approval")
        if baseline.get(path) != current[path]:
            changed.add(path)
            if path not in REVIEWED_CHANGE_PATHS or path not in approval["changes"]:
                raise ExportError("Unapproved changed bytes or mode: " + path)
            if path == "Cargo.toml":
                before = git(repo, "cat-file", "blob", baseline[path][2]) if path in baseline else b""
                if (path not in baseline or mode != baseline[path][0]
                        or not (repository_url_only(before, data)
                                or pinned_vm_dependency_only(before, data))):
                    raise ExportError("Cargo approval permits only the public repository URL and exact pinned VM parser addition")
            if path == "scripts/deploy.sh":
                before = git(repo, "cat-file", "blob", baseline[path][2]) if path in baseline else b""
                old_example = ("root@" + "192.168." + "1.100").encode()
                if mode != baseline[path][0] or before.replace(old_example, b"root@lab.example.invalid") != data:
                    raise ExportError("Deployment helper permits only example-identifier sanitization")
        public_content(path, data)
        blobs[path] = data
    if set(approval["changes"]) != changed:
        raise ExportError("Changed-path approval must exactly match the commit difference")
    return blobs, sorted(set(current) - selected)


def create_approval(repo, commit, baseline_commit, changes):
    """Create a review candidate, never an automatic owner approval."""
    entries = tree(repo, commit)
    files = {}
    for path in sorted(set(entries) & PUBLIC_PATHS):
        mode, kind, blob = entries[path]
        files[path] = {"sha256": sha256(git(repo, "cat-file", "blob", blob)), "mode": mode}
    approval = {"schema": SCHEMA, "commit": commit, "baseline_commit": baseline_commit,
                "files": files, "changes": changes}
    approved_blobs(repo, commit, approval)
    return approval


def safe_destination(repo, destination):
    destination = Path(destination).absolute()
    if destination.exists() or destination.is_symlink():
        raise ExportError("Destination must be a new directory")
    for ancestor in (destination, *destination.parents):
        if ancestor.is_symlink():
            raise ExportError("Symlink destination/ancestor is forbidden")
    resolved = destination.resolve()
    if resolved.is_relative_to(Path(repo).resolve()):
        raise ExportError("Destination cannot be inside the source repository")
    if not destination.parent.is_dir():
        raise ExportError("Destination parent must already exist")
    for ancestor in destination.parents:
        if (ancestor / ".git").exists():
            raise ExportError("Destination cannot inherit an ancestor Git repository")
    return destination


# TRACEWEAVER: entrypoint=export_snapshot; req=REQ-PUB-002; trace=TRACE-PUB-002; ver=VER-PUB-002
def export_snapshot(repo, commit, destination, approval):
    destination = safe_destination(repo, destination)
    blobs, excluded = approved_blobs(repo, commit, approval)
    manifest = {"schema": SCHEMA, "commit": commit, "baseline_commit": approval["baseline_commit"],
                "files": approval["files"], "changes": approval["changes"], "excluded": excluded}
    staging = Path(tempfile.mkdtemp(prefix=".public-export-", dir=destination.parent))
    try:
        for path, data in blobs.items():
            target = staging / path
            target.parent.mkdir(parents=True, exist_ok=True)
            with target.open("xb") as stream:
                stream.write(data)
            target.chmod(0o755 if approval["files"][path]["mode"] == "100755" else 0o644)
        verify_snapshot(staging, manifest)
        # Never replace an existing directory, even if it appeared while validating.
        if destination.exists() or destination.is_symlink():
            raise ExportError("Destination appeared during export")
        staging.rename(destination)
    finally:
        if staging.exists():
            shutil.rmtree(staging)
    return manifest


def verify_snapshot(destination, manifest):
    destination = Path(destination)
    if destination.is_symlink() or not destination.is_dir():
        raise ExportError("Snapshot must be a real directory")
    approval = {key: manifest.get(key) for key in ("schema", "commit", "baseline_commit", "files", "changes")}
    if set(manifest) != set(approval) | {"excluded"}:
        raise ExportError("Snapshot receipt has unexpected history/ref fields")
    validate_approval(approval)
    found = set()
    expected_dirs = {str(parent) for path in approval["files"] for parent in PurePosixPath(path).parents
                     if str(parent) != "."}
    for base, dirs, files in os.walk(destination, followlinks=False):
        for name in dirs:
            path = Path(base) / name
            relative = path.relative_to(destination).as_posix()
            if path.is_symlink() or relative not in expected_dirs:
                raise ExportError("Unexpected directory, symlink or Git metadata in snapshot")
        for name in files:
            path = Path(base) / name
            relative = path.relative_to(destination).as_posix()
            validate_path(relative)
            if path.is_symlink() or not path.is_file() or relative not in approval["files"]:
                raise ExportError("Unexpected snapshot file or symlink")
            data = path.read_bytes()
            expected = approval["files"][relative]
            mode = 0o755 if expected["mode"] == "100755" else 0o644
            if sha256(data) != expected["sha256"] or path.stat().st_mode & 0o7777 != mode:
                raise ExportError("Snapshot file bytes/mode differ from approval")
            public_content(relative, data)
            found.add(relative)
    if found != set(approval["files"]):
        raise ExportError("Snapshot is missing approved files")


def write_json(path, value):
    with Path(path).open("x", encoding="utf-8") as stream:
        json.dump(value, stream, indent=2, sort_keys=True)
        stream.write("\n")


# TRACEWEAVER: entrypoint=main; req=REQ-PUB-002; trace=TRACE-PUB-002; ver=VER-PUB-002
def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    propose = commands.add_parser("propose", help="Create an exact-tree candidate for owner review")
    propose.add_argument("--repo", required=True)
    propose.add_argument("--commit", required=True)
    propose.add_argument("--baseline", required=True)
    propose.add_argument("--changes", required=True, help="External JSON path -> reviewed reason map")
    propose.add_argument("--approval", required=True)
    export = commands.add_parser("export")
    export.add_argument("--repo", required=True)
    export.add_argument("--commit", required=True)
    export.add_argument("--approval", required=True)
    export.add_argument("--output", required=True)
    export.add_argument("--manifest", required=True)
    verify = commands.add_parser("verify")
    verify.add_argument("--output", required=True)
    verify.add_argument("--manifest", required=True)
    args = parser.parse_args()
    try:
        if args.command == "propose":
            changes = json.loads(Path(args.changes).read_text())
            write_json(args.approval, create_approval(args.repo, args.commit, args.baseline, changes))
        elif args.command == "export":
            if Path(args.manifest).absolute().is_relative_to(Path(args.output).absolute()):
                raise ExportError("Receipt must be outside the public tree")
            if Path(args.manifest).exists():
                raise ExportError("Receipt must be a new file")
            approval = json.loads(Path(args.approval).read_text())
            manifest = export_snapshot(args.repo, args.commit, args.output, approval)
            write_json(args.manifest, manifest)
        else:
            verify_snapshot(args.output, json.loads(Path(args.manifest).read_text()))
    except (ExportError, OSError, ValueError, KeyError, TypeError):
        parser.exit(1, "Public snapshot validation failed; inspect the private approval and candidate.\n")
    print("Public snapshot validation passed.")


if __name__ == "__main__":
    main()
