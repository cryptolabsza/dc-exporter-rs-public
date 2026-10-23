# TRACEWEAVER: file-role=driver-free-abi-tests; req=REQ-THERMAL-012; trace=TRACE-THERMAL-012; ver=VER-THERMAL-010
# TRACEWEAVER: verifies=VER-THERMAL-010; req=REQ-THERMAL-012; trace=TRACE-THERMAL-012
"""REQ-THERMAL-012/REQ-PUB-003/007: pinned, driver-free ABI verification."""
import copy
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import ssl
import subprocess
import tempfile
import time
import unittest
import urllib.error
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("rusd_abi", ROOT / "scripts/ci/rusd_abi.py")
abi = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(abi)


class LockedSourceTests(unittest.TestCase):
    def setUp(self):
        self.lock = abi.load_lock()

    def test_exact_three_profile_dependency_closures(self):
        self.assertEqual([p["version"] for p in self.lock["profiles"]], ["590.48.01", "595.99.02", "615.71.09"])
        self.assertEqual([p["commit"] for p in self.lock["profiles"]], [
            "2ccbad25e1af6a6ee6f38cf569f89f8b65d658ab", "e394a404108a9ce5bba988c1dddf694dabbd4a43", "61dcc93722ecb418bb5f2e00923f05b4b8051dd1"])
        self.assertEqual([len(p["headers"]) for p in self.lock["profiles"]], [34, 34, 34])
        self.assertEqual([sum(h["size"] for h in p["headers"]) for p in self.lock["profiles"]], [469235, 476190, 474626])
        for profile, expected in zip(self.lock["profiles"], [
            "0f2ef94405b6646673ec651e41c268258ea7ec7ae347b3146d3f6e9297162340",
            "35670d98dc93e7ceb70d58ded910328cf9b275d40ed53b014fcd9cab323dd091",
            "e4f11a480ab931a115564f5659f909e524717b1e6850fce0adea750a5e7850e7"]):
            header = next(h for h in profile["headers"] if h["path"].endswith("/cl00de.h"))
            self.assertEqual(header["sha256"], expected)
            self.assertEqual(abi.header_url(profile, header),
                "https://raw.githubusercontent.com/NVIDIA/open-gpu-kernel-modules/" + profile["commit"] + "/" + header["path"])

    def test_615_uses_its_authoritative_dependency_closure_and_preserves_caps(self):
        self.assertEqual(len(self.lock["profiles"]), 3)
        previous, candidate = self.lock["profiles"][1:]
        old = {h["path"] for h in previous["headers"]}
        new = {h["path"] for h in candidate["headers"]}
        self.assertEqual(new - old, {"src/common/sdk/nvidia/inc/nvintrin.h"})
        self.assertEqual(old - new, {"src/common/sdk/nvidia/inc/alloc/alloc_channel.h"})
        entry = next(h for h in candidate["headers"] if h["path"].endswith("/nvintrin.h"))
        self.assertEqual(entry["size"], 2168)
        self.assertEqual(entry["sha256"], "6b296650a81b44967c75c8405ae89548f4df5b5cc22ed9ad4573eed1af490320")
        self.assertEqual(sum(h["size"] for p in self.lock["profiles"] for h in p["headers"]), 1420051)
        self.assertEqual((abi.FILE_LIMIT, abi.TOTAL_LIMIT, abi.REQUEST_TIMEOUT), (262144, 2097152, 10))
        for version in ["615.71.08", "615.71.10", "615.71.09-open", "615.78.08"]:
            altered = copy.deepcopy(self.lock)
            altered["profiles"][2]["version"] = version
            with self.subTest(version=version), self.assertRaises(abi.AbiError):
                abi.validate_lock(altered)

    def test_manifest_rejects_unreviewed_versions_commits_and_duplicate_paths(self):
        for field, value in [("version", "615.71.10"), ("commit", "main"), ("commit", "0" * 40), ("profile", 615)]:
            data = copy.deepcopy(self.lock)
            data["profiles"][0][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(abi.AbiError):
                abi.validate_lock(data)
        data = copy.deepcopy(self.lock)
        data["profiles"][0]["headers"][1] = data["profiles"][0]["headers"][0]
        with self.assertRaises(abi.AbiError):
            abi.validate_lock(data)

    def test_manifest_rejects_paths_lengths_hashes_and_incomplete_closure(self):
        for field, value in [("path", "../escape.h"), ("path", "/tmp/x.h"), ("path", "src\\x.h"),
                             ("path", "https://evil.example/x.h"), ("size", 0), ("size", 262145),
                             ("size", True), ("sha256", "f" * 63)]:
            data = copy.deepcopy(self.lock)
            data["profiles"][0]["headers"][0][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(abi.AbiError):
                abi.validate_lock(data)
        data = copy.deepcopy(self.lock)
        data["profiles"][0]["headers"].pop()
        with self.assertRaises(abi.AbiError):
            abi.validate_lock(data)

    def test_manifest_rejects_unreviewed_extra_keys(self):
        for level in ["root", "profile", "header"]:
            data = copy.deepcopy(self.lock)
            target = data if level == "root" else data["profiles"][0] if level == "profile" else data["profiles"][0]["headers"][0]
            target["unreviewed"] = "ignored input must fail closed"
            with self.subTest(level=level), self.assertRaises(abi.AbiError):
                abi.validate_lock(data)
        data = copy.deepcopy(self.lock)
        for profile in data["profiles"]:
            for header in profile["headers"]:
                header["size"] = 262144
        with self.assertRaises(abi.AbiError):
            abi.validate_lock(data)


class CacheAndDownloadTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve() / "cache"
        self.profile = {"version": "590.48.01", "profile": 590, "commit": "2ccbad25e1af6a6ee6f38cf569f89f8b65d658ab"}
        self.body = b"/* public MIT header fixture */\n"
        self.entry = {"path": "src/common/sdk/nvidia/inc/nvtypes.h", "size": len(self.body), "sha256": hashlib.sha256(self.body).hexdigest()}

    def cached(self):
        path = self.root / self.profile["commit"] / self.entry["path"]
        path.parent.mkdir(parents=True)
        path.write_bytes(self.body)
        return path

    def test_verified_cache_needs_no_network(self):
        path = self.cached()
        with mock.patch.object(abi, "download_header", side_effect=AssertionError("unit network forbidden")):
            self.assertEqual(abi.materialize_header(self.root, self.profile, self.entry, time.monotonic()+10), path)

    def test_corrupt_cache_fails_without_network_or_repair(self):
        path = self.cached()
        path.write_bytes(b"x" * len(self.body))
        with mock.patch.object(abi, "download_header", side_effect=AssertionError("unit network forbidden")):
            with self.assertRaises(abi.AbiError):
                abi.materialize_header(self.root, self.profile, self.entry, time.monotonic()+10)

    def test_cache_symlink_file_parent_and_root_rejected(self):
        for position in ["file", "parent", "root"]:
            with self.subTest(position=position), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp).resolve() / "cache"
                outside = Path(tmp).resolve() / "outside"
                outside.mkdir()
                if position == "root":
                    root.symlink_to(outside, target_is_directory=True)
                elif position == "parent":
                    root.mkdir()
                    (root / self.profile["commit"]).symlink_to(outside, target_is_directory=True)
                else:
                    path = root / self.profile["commit"] / self.entry["path"]
                    path.parent.mkdir(parents=True)
                    target = outside / "header.h"
                    target.write_bytes(self.body)
                    path.symlink_to(target)
                with self.assertRaises(abi.AbiError):
                    abi.materialize_header(root, self.profile, self.entry, time.monotonic()+10)

    def test_nonregular_cache_entry_rejected(self):
        path = self.cached()
        path.unlink()
        path.mkdir()
        with self.assertRaises(abi.AbiError):
            abi.materialize_header(self.root, self.profile, self.entry, time.monotonic()+10)

    def test_missing_header_downloads_verified_bytes_atomically(self):
        with mock.patch.object(abi, "download_header", return_value=self.body):
            path = abi.materialize_header(self.root, self.profile, self.entry, time.monotonic()+10)
        self.assertEqual(path.read_bytes(), self.body)
        self.assertEqual(list(path.parent.glob("*.tmp")), [])

    def test_truncated_oversized_bad_digest_and_expired_download_fail(self):
        url = abi.header_url(self.profile, self.entry)
        for body in [self.body[:-1], self.body+b"x", b"x"*len(self.body)]:
            response = io.BytesIO(body)
            response.geturl = lambda: url
            with self.subTest(body=body), mock.patch.object(abi, "make_opener") as factory:
                factory.return_value.open.return_value = response
                with self.assertRaises(abi.AbiError):
                    abi.download_header(url, self.entry, time.monotonic()+10)
                self.assertEqual(factory.return_value.open.call_count, 1)
        with self.assertRaises(abi.AbiError):
            abi.download_header(url, self.entry, time.monotonic()-1)

    def test_redirect_and_unpinned_url_rejected(self):
        url = abi.header_url(self.profile, self.entry)
        response = io.BytesIO(self.body)
        response.geturl = lambda: "https://evil.example/header.h"
        with mock.patch.object(abi, "make_opener") as factory:
            factory.return_value.open.return_value = response
            with self.assertRaises(abi.AbiError):
                abi.download_header(url, self.entry, time.monotonic()+10)
        for bad in ["http://raw.githubusercontent.com/x", "https://evil.example/x", url.replace(self.profile["commit"], "main")]:
            with self.subTest(url=bad), self.assertRaises(abi.AbiError):
                abi.download_header(bad, self.entry, time.monotonic()+10)

    def test_transient_retry_is_bounded_and_request_timeout_at_most_ten_seconds(self):
        url = abi.header_url(self.profile, self.entry)
        with mock.patch.object(abi, "make_opener") as factory:
            factory.return_value.open.side_effect = urllib.error.URLError("temporary offline")
            with self.assertRaises(abi.AbiError):
                abi.download_header(url, self.entry, time.monotonic()+120)
            self.assertEqual(factory.return_value.open.call_count, 2)
            for call in factory.return_value.open.call_args_list:
                self.assertGreater(call.kwargs["timeout"], 0)
                self.assertLessEqual(call.kwargs["timeout"], 10)

    def test_request_wall_timeout_bounds_blocking_transport(self):
        url = abi.header_url(self.profile, self.entry)
        def blocked_open(*args, **kwargs):
            time.sleep(0.05)
            response = io.BytesIO(self.body)
            response.geturl = lambda: url
            return response
        with mock.patch.object(abi, "REQUEST_TIMEOUT", 0.01, create=True), mock.patch.object(abi, "make_opener") as factory:
            factory.return_value.open.side_effect = blocked_open
            with self.assertRaises(abi.AbiError):
                abi.download_header(url, self.entry, time.monotonic()+10)
            self.assertEqual(factory.return_value.open.call_count, 2)

    def test_tls_context_does_not_enable_keylogging_or_proxy_auth(self):
        with mock.patch.dict(os.environ, {"SSLKEYLOGFILE": str(Path(self.tmp.name)/"keylog"), "HTTPS_PROXY": "http://credential.invalid"}), \
             mock.patch.object(ssl, "create_default_context", side_effect=AssertionError("keylog-sensitive helper forbidden")):
            opener = abi.make_opener()
        https = next(handler for handler in opener.handlers if hasattr(handler, "_context"))
        self.assertTrue(https._context.check_hostname)
        self.assertEqual(https._context.verify_mode, ssl.CERT_REQUIRED)
        self.assertIsNone(https._context.keylog_filename)
        self.assertFalse((Path(self.tmp.name)/"keylog").exists())
        self.assertFalse(any(getattr(handler, "proxies", {}) for handler in opener.handlers))


class CompilerTests(unittest.TestCase):
    def test_native_architecture_and_os_only(self):
        abi.require_native("Linux", "x86_64")
        for system, machine in [("Darwin", "x86_64"), ("Linux", "aarch64"), ("Linux", "i686")]:
            with self.assertRaises(abi.AbiError):
                abi.require_native(system, machine)

    def test_compiler_environment_removes_injection_and_loader_variables(self):
        forbidden = ["CPATH", "C_INCLUDE_PATH", "CPLUS_INCLUDE_PATH", "OBJC_INCLUDE_PATH", "GCC_EXEC_PREFIX", "COMPILER_PATH", "LD_PRELOAD", "LD_LIBRARY_PATH", "DYLD_INSERT_LIBRARIES", "DYLD_LIBRARY_PATH"]
        with mock.patch.dict(os.environ, {key: "unsafe" for key in forbidden}):
            env = abi.compiler_environment()
        self.assertFalse(set(forbidden) & set(env))

    def test_compiler_failure_cannot_be_reported_as_success(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(abi.subprocess, "run", side_effect=subprocess.CalledProcessError(1, ["/usr/bin/cc"])):
            with self.assertRaises(abi.AbiError):
                abi.compile_profiles(Path(tmp), abi.load_lock(), compiler="/usr/bin/cc")

    def test_compiler_pointer_target_probe_rejects_wrong_target(self):
        def wrong_target(args, **kwargs):
            kwargs["stdout"].write(b"#define __SIZEOF_POINTER__ 4\n")
            return subprocess.CompletedProcess(args, 0)
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(abi.subprocess, "run", side_effect=wrong_target):
            with self.assertRaises(abi.AbiError):
                abi.compile_profiles(Path(tmp), abi.load_lock(), compiler="/usr/bin/cc")

    def test_negative_fixture_must_fail_and_commands_never_link_or_run(self):
        macros = "#define __linux__ 1\n#define __x86_64__ 1\n#define __SIZEOF_POINTER__ 8\n"
        def accepts_everything(args, **kwargs):
            if "-dM" in args:
                kwargs["stdout"].write(macros.encode())
            return subprocess.CompletedProcess(args, 0)
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(abi.subprocess, "run", side_effect=accepts_everything) as run:
            with self.assertRaises(abi.AbiError):
                abi.compile_profiles(Path(tmp), abi.load_lock(), compiler="/usr/bin/cc")
        self.assertEqual(run.call_count, 4)
        for call in run.call_args_list[1:]:
            args = call.args[0]
            self.assertIn("-fsyntax-only", args)
            self.assertIn("-ffreestanding", args)
            self.assertNotIn("-o", args)
            self.assertNotIn("-target", args)

    def test_all_three_matches_six_mismatches_default_and_unknown_are_checked(self):
        commits = {"2ccbad25e1af6a6ee6f38cf569f89f8b65d658ab": 590,
                   "e394a404108a9ce5bba988c1dddf694dabbd4a43": 595,
                   "61dcc93722ecb418bb5f2e00923f05b4b8051dd1": 615}
        observed = []
        def compiler(args, **kwargs):
            if "-dM" in args:
                kwargs["stdout"].write(b"#define __linux__ 1\n#define __x86_64__ 1\n#define __SIZEOF_POINTER__ 8\n")
                return subprocess.CompletedProcess(args, 0)
            includes = [arg for arg in args if arg.startswith("-I")]
            if not includes:
                return subprocess.CompletedProcess(args, 0)
            actual = next(value for commit, value in commits.items() if commit in includes[0])
            defines = [arg for arg in args if arg.startswith("-DRUSD_ABI_PROFILE=")]
            selected = int(defines[0].split("=")[1]) if defines else 590
            observed.append((actual, selected, bool(defines)))
            self.assertIn("-fsyntax-only", args)
            self.assertIn("-ffreestanding", args)
            self.assertEqual(kwargs["timeout"], 30)
            if selected not in commits.values():
                kwargs["stderr"].write(b"Only exact pinned ABI fixtures are defined")
                return subprocess.CompletedProcess(args, 1)
            if actual != selected:
                kwargs["stderr"].write(b"NV00DE_SHARED_DATA size mismatch")
                return subprocess.CompletedProcess(args, 1)
            return subprocess.CompletedProcess(args, 0)
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(abi.subprocess, "run", side_effect=compiler):
            abi.compile_profiles(Path(tmp), abi.load_lock(), compiler="/usr/bin/cc")
        self.assertEqual(len(abi.load_lock()["profiles"]), 3)
        self.assertCountEqual([(a, b) for a, b, explicit in observed if explicit and b in commits.values()],
                              [(a, b) for a in (590, 595, 615) for b in (590, 595, 615)])
        self.assertIn((590, 590, False), observed)
        self.assertIn((590, 999, True), observed)
        self.assertEqual(len(observed), 11)

    def test_compiler_diagnostics_have_an_explicit_bounded_sink(self):
        def noisy_compiler(args, **kwargs):
            self.assertNotIn("capture_output", kwargs)
            kwargs["stdout"].write(b"x" * (262144 + 1))
            return subprocess.CompletedProcess(args, 0)
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(abi.subprocess, "run", side_effect=noisy_compiler):
            with self.assertRaises(abi.AbiError):
                abi.compile_profiles(Path(tmp), abi.load_lock(), compiler="/usr/bin/cc")


if __name__ == "__main__":
    unittest.main()
