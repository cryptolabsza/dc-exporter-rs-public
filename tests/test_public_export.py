# TRACEWEAVER: file-role=public-snapshot-abi-allowlist-verification; req=REQ-PUB-007; trace=TRACE-PUB-007; ver=VER-PUB-007
# TRACEWEAVER: verifies=VER-PUB-007; req=REQ-PUB-007; trace=TRACE-PUB-007
# TRACEWEAVER: file-role=public-snapshot-fixtures; req=REQ-PUB-002; trace=TRACE-PUB-002; ver=VER-PUB-002
# TRACEWEAVER: verifies=VER-PUB-002; req=REQ-PUB-002; trace=TRACE-PUB-002
"""Offline public-snapshot acceptance fixtures; all Git mutations use scratch repos."""

import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "public_export.py"


def digest(data):
    return hashlib.sha256(data).hexdigest()


class PublicExportTests(unittest.TestCase):
    def setUp(self):
        self.assertTrue(SCRIPT.is_file(), "commit-tree public exporter has not been implemented")
        spec = importlib.util.spec_from_file_location("public_export", SCRIPT)
        self.exporter = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.exporter)
        self.scratch = tempfile.TemporaryDirectory(prefix="public-export-test-")
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name).resolve()
        self.repo = self.root / "private"
        self.repo.mkdir()
        self.git("init", "-q")
        self.git("config", "user.name", "Snapshot Fixture")
        self.git("config", "user.email", "fixture@example.invalid")
        self.git("config", "commit.gpgsign", "false")
        self.git("config", "core.hooksPath", "/dev/null")
        self.put("Cargo.toml", '[package]\nname = "fixture"\nversion = "0.2.7"\nrepository = "https://github.com/cryptolabsza/dc-exporter-rs-public"\n')
        self.put("src/main.rs", "fn main() {}\n")
        self.put("tests/thermal_policy.rs", "// offline fixture\n")
        self.put("README.md", "# Exporter\nExperimental thermal support; hardware acceptance pending.\n")
        self.put("docs/gpu-thermal-abi.md", "# Public ABI provenance\n")
        self.put("scripts/build-release.sh", "#!/bin/sh\nexit 0\n", 0o755)
        self.put(".traceweaver/private.json", '{"private":true}\n')
        self.put("AGENTS.md", "private operator instructions\n")
        self.baseline = self.commit()
        self.output = self.root / "snapshot"
        self.approval = self.approve(self.baseline)

    def git(self, *args):
        return subprocess.check_output(["git", "-C", str(self.repo), *args], stderr=subprocess.PIPE)

    def put(self, name, text, mode=0o644):
        path = self.repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        path.chmod(mode)

    def commit(self):
        self.git("add", "--all")
        self.git("commit", "-qm", "scratch fixture")
        return self.git("rev-parse", "HEAD").decode().strip()

    def approve(self, commit, changes=None):
        files = {}
        for record in self.git("ls-tree", "-rz", "--full-tree", commit).split(b"\0"):
            if not record:
                continue
            entry, name = record.split(b"\t", 1)
            name = name.decode()
            if name.startswith(".traceweaver/") or name == "AGENTS.md":
                continue
            mode, kind, blob = entry.decode().split()
            files[name] = {"sha256": digest(self.git("cat-file", "blob", blob)), "mode": mode}
        return {"schema": "public-snapshot/1", "commit": commit,
                "baseline_commit": self.baseline, "files": files, "changes": changes or {}}

    def export(self, commit=None, approval=None, output=None):
        return self.exporter.export_snapshot(self.repo, commit or self.approval["commit"],
                                             output or self.output, approval or self.approval)

    def rejected(self, action):
        with self.assertRaises(self.exporter.ExportError):
            action()

    def test_gpu_health_document_is_the_only_new_health_public_path(self):
        self.assertIn("docs/gpu-health.md", self.exporter.PUBLIC_PATHS)
        self.assertIn("docs/gpu-health.md", self.exporter.REVIEWED_CHANGE_PATHS)
        for private in ("docs/validation/gpu-health-acceptance.md", ".traceweaver/gpu-health-2026-10-14/plan.md", "docs/gpu-health-private.md"):
            self.assertNotIn(private, self.exporter.PUBLIC_PATHS)
        self.put("docs/gpu-health.md", "# GPU health\nRecovery observations do not establish physical damage.\n")
        commit = self.commit()
        self.rejected(lambda: self.export(commit, self.approve(commit)))
        approval = self.approve(commit, {"docs/gpu-health.md": "Reviewed additive health metrics documentation"})
        self.export(commit, approval)
        self.assertEqual((self.output / "docs/gpu-health.md").read_text(), (self.repo / "docs/gpu-health.md").read_text())

    def test_driver_free_abi_adds_exact_public_paths(self):
        added = {"scripts/ci/rusd_abi.py", "scripts/ci/rusd-headers.json",
                 "tests/test_ci_rusd_abi.py"}
        baseline = frozenset("""
.github/workflows/build.yml .gitignore Cargo.toml Cargo.lock LICENSE Makefile README.md
RELEASING.md TESTING.md build.rs config.toml.example
docs/gpu-thermal-abi.md docs/gpu-thermal-support.md docs/public-publishing.md
docs/validation/rtx5090-acceptance.md docs/fixtures/rusd-abi-layout.c
scripts/build-release.sh scripts/deploy.sh scripts/public_export.py
scripts/ci/build_contract.py scripts/ci/build.sh scripts/ci/publish.py
src/config.rs src/main.rs src/safe_nvml.rs src/state.rs
src/metrics/collector.rs src/metrics/dmesg.rs src/metrics/gpu.rs src/metrics/mod.rs
src/metrics/nvidia_rm/abi.rs src/metrics/nvidia_rm/mod.rs src/metrics/nvidia_rm/rusd.rs
src/metrics/pci.rs src/metrics/prometheus.rs src/metrics/register.rs
src/metrics/thermal.rs src/metrics/thermal_worker.rs src/metrics/vm.rs
src/tui/app.rs src/tui/mod.rs src/tui/ui.rs systemd/dc-exporter.service
tests/nvidia_rm_abi.rs tests/nvidia_rm_rusd.rs tests/release_workflow.rs
tests/thermal_policy.rs tests/thermal_worker.rs tests/test_public_export.py
tests/test_ci_build.py tests/test_ci_publisher.py
""".split())
        pool_paths = {".github/workflows/ci-pool-build-recipe.yml", "scripts/ci/pool-build.sh",
                      "scripts/ci/pool_manifest.py", "scripts/ci/notice_inventory.py",
                      "tests/test_pool_workflow.py", "tests/test_ci_notices.py",
                      "CONTRIBUTING.md", "SECURITY.md", "THIRD-PARTY-NOTICES.md"}
        self.assertEqual(self.exporter.PUBLIC_PATHS, baseline | added | pool_paths | {
            "docs/gpu-health.md", "docs/validation/vm-passthrough-acceptance.md"})
        self.assertTrue(added.issubset(self.exporter.REVIEWED_CHANGE_PATHS))

    def test_vm_exact_document_and_three_runtime_paths_require_reviewed_hashes(self):
        paths = {"docs/validation/vm-passthrough-acceptance.md": "# VM acceptance definition\nOperational acceptance HELD; fixture only.\n",
                 "src/metrics/collector.rs": "// reviewed collector fixture\n",
                 "src/metrics/vm.rs": "// reviewed guest mapping fixture\n",
                 "src/metrics/pci.rs": "// reviewed assignment evidence fixture\n"}
        for path, content in paths.items():
            with self.subTest(policy_path=path):
                self.assertIn(path, self.exporter.PUBLIC_PATHS)
                self.assertIn(path, self.exporter.REVIEWED_CHANGE_PATHS)
            self.put(path, content)
        commit = self.commit()
        approval = self.approve(commit, {path: "Reviewed exact VM correction fixture" for path in paths})
        self.rejected(lambda: self.export(commit, self.approve(commit)))
        for path in paths:
            with self.subTest(path=path):
                missing_reason = copy.deepcopy(approval)
                del missing_reason["changes"][path]
                self.rejected(lambda: self.export(commit, missing_reason))
                empty_reason = copy.deepcopy(approval)
                empty_reason["changes"][path] = " "
                self.rejected(lambda: self.export(commit, empty_reason))
                missing_hash = copy.deepcopy(approval)
                del missing_hash["files"][path]["sha256"]
                self.rejected(lambda: self.export(commit, missing_hash))
                wrong_hash = copy.deepcopy(approval)
                wrong_hash["files"][path]["sha256"] = "0" * 64
                self.rejected(lambda: self.export(commit, wrong_hash))
        self.export(commit, approval)
        for path in paths:
            self.assertEqual((self.output / path).read_bytes(), self.git("show", commit + ":" + path))

    def test_vm_review_cannot_admit_unrelated_new_public_paths(self):
        for path in ("docs/validation/vm-private.md", "src/metrics/vm_extra.rs"):
            with self.subTest(path=path):
                self.assertNotIn(path, self.exporter.PUBLIC_PATHS)
                self.assertNotIn(path, self.exporter.REVIEWED_CHANGE_PATHS)
                self.put(path, "// unrelated fixture\n")
                commit = self.commit()
                self.rejected(lambda: self.export(commit, self.approve(commit, {path: "Reviewed reason cannot expand policy"})))
                self.git("reset", "--hard", self.baseline)

    def test_driver_free_abi_files_require_reviewed_hashes_and_export_exactly(self):
        added = {"scripts/ci/rusd_abi.py": "# pinned public ABI helper fixture\n",
                 "scripts/ci/rusd-headers.json": '{"fixture":"pinned headers"}\n',
                 "tests/test_ci_rusd_abi.py": "# pinned ABI test fixture\n"}
        for name, content in added.items():
            self.put(name, content)
        commit = self.commit()
        approval = self.approve(commit, {name: "Reviewed driver-free ABI check" for name in added})
        self.rejected(lambda: self.export(commit, self.approve(commit)))
        self.export(commit, approval)
        for name in added:
            self.assertEqual((self.output / name).read_bytes(), self.git("show", commit + ":" + name))

    def test_exports_only_approved_commit_blobs_and_modes(self):
        manifest = self.export()
        self.assertEqual(manifest["commit"], self.baseline)
        self.assertEqual(set(manifest["files"]), set(self.approval["files"]))
        for name, expected in self.approval["files"].items():
            self.assertEqual(digest((self.output / name).read_bytes()), expected["sha256"])
            self.assertEqual((self.output / name).stat().st_mode & 0o777,
                             0o755 if expected["mode"] == "100755" else 0o644)
        self.assertFalse((self.output / ".git").exists())
        self.assertFalse((self.output / ".traceweaver").exists())
        self.assertFalse((self.output / "AGENTS.md").exists())

    def test_ignores_working_tree_and_untracked_content(self):
        self.put("src/main.rs", "unapproved mutable source\n")
        self.put("README.md", "unapproved mutable documentation\n")
        self.put("secret.env", "untracked secret\n")
        self.export()
        self.assertEqual((self.output / "src/main.rs").read_text(), "fn main() {}\n")
        self.assertNotIn("mutable", (self.output / "README.md").read_text())
        self.assertFalse((self.output / "secret.env").exists())

    def test_rejects_moving_branch_and_short_commit_names(self):
        for ref in ("HEAD", self.baseline[:12], "--all"):
            with self.subTest(ref=ref):
                self.rejected(lambda: self.export(commit=ref))

    def test_rejects_approval_for_different_commit(self):
        self.put("README.md", "Reviewed new documentation\n")
        current = self.commit()
        self.rejected(lambda: self.export(commit=current))

    def test_rejects_bad_or_private_approval_paths(self):
        names = ("../escape", "/absolute", "src/../escape", "src//main.rs", "src\\main.rs",
                 ".git/config", ".traceweaver/private.json", "AGENTS.md", ".env",
                 ".memsearch/memory.md", "graphify-out/graph.json", "docs/plans/private.md",
                 "requirements.md", "traceability-matrix.md", "unexpected.txt")
        for name in names:
            with self.subTest(name=name):
                approval = copy.deepcopy(self.approval)
                approval["files"][name] = {"sha256": "0" * 64, "mode": "100644"}
                self.rejected(lambda: self.export(approval=approval))

    def test_rejects_existing_private_blobs_with_valid_hashes_and_modes(self):
        names = ("AGENTS.md", ".traceweaver/private.json", ".memsearch/memory.md",
                 "graphify-out/graph.json", "docs/plans/private.md", "requirements.md",
                 "traceability-matrix.md", ".env", "unexpected.txt")
        for name in names:
            self.put(name, "ordinary content with no secret marker\n")
        commit = self.commit()
        for name in names:
            with self.subTest(name=name):
                approval = copy.deepcopy(self.approval)
                approval["commit"] = commit
                approval["baseline_commit"] = commit
                approval["files"][name] = {
                    "sha256": digest(self.git("show", commit + ":" + name)), "mode": "100644"}
                self.rejected(lambda: self.export(commit, approval))

    def test_rejects_missing_production_test_or_package_asset(self):
        for name in ("src/main.rs", "tests/thermal_policy.rs", "Cargo.toml", "scripts/build-release.sh",
                     "docs/gpu-thermal-abi.md"):
            with self.subTest(name=name):
                approval = copy.deepcopy(self.approval)
                del approval["files"][name]
                self.rejected(lambda: self.export(approval=approval))

    def test_rejects_unapproved_source_test_and_package_changes(self):
        for name in ("src/main.rs", "tests/thermal_policy.rs", "scripts/build-release.sh"):
            with self.subTest(name=name):
                self.put(name, "changed production or fixture bytes\n")
                commit = self.commit()
                self.rejected(lambda: self.export(commit, self.approve(commit)))
                self.git("reset", "--hard", self.baseline)

    def test_reason_cannot_authorize_production_code_change(self):
        self.put("src/main.rs", "fn main() { panic!(); }\n")
        commit = self.commit()
        approval = self.approve(commit, {"src/main.rs": "operator approved"})
        self.rejected(lambda: self.export(commit, approval))

    def test_unknown_production_file_cannot_be_silently_excluded(self):
        self.put("src/new_module.rs", "// additional production code\n")
        commit = self.commit()
        approval = copy.deepcopy(self.approval)
        approval["commit"] = approval["baseline_commit"] = commit
        self.rejected(lambda: self.export(commit, approval))

    def test_new_lockfile_is_required_and_preserved_exactly(self):
        self.put("Cargo.lock", '# lockfile fixture\nversion = 4\n')
        commit = self.commit()
        approval = self.approve(commit, {"Cargo.lock": "Pinned reproducible CI dependencies"})
        missing = copy.deepcopy(approval)
        del missing["files"]["Cargo.lock"]
        del missing["changes"]["Cargo.lock"]
        self.rejected(lambda: self.export(commit, missing))
        self.export(commit, approval)
        self.assertEqual((self.output / "Cargo.lock").read_bytes(), self.git("show", commit + ":Cargo.lock"))

    def test_docs_and_ci_changes_require_specific_nonempty_reasons(self):
        self.put("README.md", "# Public reviewed documentation\n")
        self.put(".github/workflows/build.yml", "name: reviewed\n")
        self.put(".gitignore", "/target/\n__pycache__/\n")
        commit = self.commit()
        approval = self.approve(commit)
        self.rejected(lambda: self.export(commit, approval))
        approval["changes"] = {"README.md": "", ".github/workflows/build.yml": "CI isolation"}
        self.rejected(lambda: self.export(commit, approval))
        approval["changes"]["README.md"] = "Sanitized public instructions"
        approval["changes"][".gitignore"] = "Exclude private local state"
        self.export(commit, approval)

    def test_cargo_exception_allows_only_repository_url(self):
        self.put("Cargo.toml", '[package]\nname = "fixture"\nversion = "0.2.8"\nrepository = "https://github.com/cryptolabsza/dc-exporter-rs-public"\n')
        commit = self.commit()
        self.rejected(lambda: self.export(commit, self.approve(commit, {"Cargo.toml": "public repository URL"})))

    def vm_cargo_manifests(self):
        # Exercise the maintained real manifest's exact reviewed dependency block.
        # These scratch exports provide no Cargo build or dependency-licence proof.
        after = (SCRIPT.parent.parent / "Cargo.toml").read_text()
        addition = '\n# Strict libvirt XML parsing; encoding/transcoding and entity resolvers disabled.\nquick-xml = { version = "=0.42.0", default-features = false }\n'
        self.assertEqual(after.count(addition), 1)
        return after.replace(addition, "", 1), after

    def test_cargo_exact_vm_parser_addition_requires_reviewed_reason_and_hash(self):
        before, after = self.vm_cargo_manifests()
        self.put("Cargo.toml", before)
        self.baseline = self.commit()
        self.put("Cargo.toml", after)
        commit = self.commit()
        approval = self.approve(commit, {"Cargo.toml": "Reviewed pinned VM XML parser addition"})
        self.rejected(lambda: self.export(commit, self.approve(commit)))
        for field, value in [("sha256", "0" * 64), ("mode", "100755")]:
            wrong = copy.deepcopy(approval)
            wrong["files"]["Cargo.toml"][field] = value
            self.rejected(lambda: self.export(commit, wrong))
        self.export(commit, approval)
        self.assertEqual((self.output / "Cargo.toml").read_text(), after)

    def test_cargo_vm_addition_can_accompany_existing_repository_url_sanitization(self):
        before, after = self.vm_cargo_manifests()
        before = before.replace('repository = "https://github.com/cryptolabsza/dc-exporter-rs-public"',
                                'repository = "https://github.com/example/private-fixture"')
        self.put("Cargo.toml", before)
        self.baseline = self.commit()
        self.put("Cargo.toml", after)
        commit = self.commit()
        self.export(commit, self.approve(commit, {"Cargo.toml": "Reviewed exact parser and public repository URL"}))
        self.assertEqual((self.output / "Cargo.toml").read_text(), after)

    def test_cargo_vm_exception_rejects_every_other_manifest_change(self):
        before, after = self.vm_cargo_manifests()
        self.put("Cargo.toml", before)
        self.baseline = self.commit()
        cases = [after.replace('version = "0.2.7"', 'version = "0.2.8"', 1),
                 after.replace('"=0.42.0"', '"=0.42.1"'),
                 after.replace('default-features = false', 'default-features = true'),
                 after.replace('default-features = false', 'default-features = false, features = ["encoding"]'),
                 after.replace('quick-xml = ', 'arbitrary-parser = '),
                 after.replace('serde_json = "1"', 'serde_json = "2"'),
                 after.replace('lto = true', 'lto = false'),
                 after.replace('strip = true', 'strip = false'),
                 after + '\n[features]\ndefault = ["unexpected"]\n',
                 after.replace('version = "=0.42.0"', 'version = "=0.42.0", optional = true')]
        for index, changed in enumerate(cases):
            with self.subTest(case=index):
                self.put("Cargo.toml", changed)
                commit = self.commit()
                self.rejected(lambda: self.export(commit, self.approve(commit, {"Cargo.toml": "A reason cannot expand exact parser policy"})))
                self.git("reset", "--hard", self.baseline)

    def test_rejects_approved_hash_or_mode_mismatch(self):
        for field, value in (("sha256", "0" * 64), ("mode", "100755")):
            with self.subTest(field=field):
                approval = copy.deepcopy(self.approval)
                approval["files"]["src/main.rs"][field] = value
                self.rejected(lambda: self.export(approval=approval))

    def test_rejects_symlink_blobs_even_when_hash_approved(self):
        for target in ("../outside", "main.rs"):
            with self.subTest(target=target):
                link = self.repo / "src/main.rs"
                link.unlink()
                link.symlink_to(target)
                commit = self.commit()
                approval = self.approve(commit)
                approval["baseline_commit"] = commit
                self.rejected(lambda: self.export(commit, approval))
                self.git("reset", "--hard", self.baseline)

    def test_rejects_existing_or_in_repository_destination(self):
        self.output.mkdir()
        self.put("untouched.txt", "private\n")
        (self.output / "untouched").write_text("keep\n")
        self.rejected(lambda: self.export())
        self.assertEqual((self.output / "untouched").read_text(), "keep\n")
        self.rejected(lambda: self.export(output=self.repo / "snapshot"))

    def test_rejects_symlink_destination_parent(self):
        (self.root / "linked").symlink_to(self.root, target_is_directory=True)
        self.rejected(lambda: self.export(output=self.root / "linked/snapshot"))

    def test_private_content_markers_fail_before_any_output(self):
        markers = ("/Us" + "ers/operator/project", "/ho" + "me/operator/private",
                   "root@" + "192.168." + "1.101", "88.0." + "95.1",
                   "-----BEGIN " + "OPENSSH PRIVATE KEY-----")
        for marker in markers:
            with self.subTest(marker=marker):
                self.put("README.md", marker + "\n")
                commit = self.commit()
                approval = self.approve(commit, {"README.md": "public documentation"})
                self.rejected(lambda: self.export(commit, approval))
                self.assertFalse(self.output.exists())

    def test_loopback_and_standard_linux_install_paths_are_public(self):
        self.put("README.md", "curl http://127.0.0.1:9835/metrics\n/usr/local/bin/dc-exporter\n")
        commit = self.commit()
        self.export(commit, self.approve(commit, {"README.md": "public documentation"}))

    def test_rejects_legacy_ref_fields_in_approval(self):
        for field in ("refs", "tags", "parents", "history", "git_dir"):
            with self.subTest(field=field):
                approval = copy.deepcopy(self.approval)
                approval[field] = [self.baseline]
                self.rejected(lambda: self.export(approval=approval))

    def test_verifier_detects_changed_bytes_modes_extra_files_and_git_refs(self):
        manifest = self.export()
        self.exporter.verify_snapshot(self.output, manifest)
        target = self.output / "src/main.rs"
        original = target.read_bytes()
        target.write_bytes(b"tampered\n")
        self.rejected(lambda: self.exporter.verify_snapshot(self.output, manifest))
        target.write_bytes(original)
        target.chmod(0o755)
        self.rejected(lambda: self.exporter.verify_snapshot(self.output, manifest))
        target.chmod(0o644)
        (self.output / "unexpected.txt").write_text("unexpected\n")
        self.rejected(lambda: self.exporter.verify_snapshot(self.output, manifest))
        (self.output / "unexpected.txt").unlink()
        (self.output / ".git").mkdir()
        (self.output / ".git/refs").mkdir()
        self.rejected(lambda: self.exporter.verify_snapshot(self.output, manifest))

    def test_verifier_rejects_replaced_file_or_directory_symlinks(self):
        manifest = self.export()
        target = self.output / "src/main.rs"
        target.unlink()
        target.symlink_to(self.repo / "src/main.rs")
        self.rejected(lambda: self.exporter.verify_snapshot(self.output, manifest))

    def test_cli_export_and_verify_use_external_approval_manifest(self):
        approval_path = self.root / "approval.json"
        approval_path.write_text(json.dumps(self.approval))
        manifest_path = self.root / "manifest.json"
        result = subprocess.run(["python3", str(SCRIPT), "export", "--repo", str(self.repo),
                                 "--commit", self.baseline, "--approval", str(approval_path),
                                 "--output", str(self.output), "--manifest", str(manifest_path)],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        manifest = json.loads(manifest_path.read_text())
        self.assertEqual(manifest["files"], self.approval["files"])
        result = subprocess.run(["python3", str(SCRIPT), "verify", "--output", str(self.output),
                                 "--manifest", str(manifest_path)], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
