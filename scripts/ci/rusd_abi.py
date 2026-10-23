#!/usr/bin/env python3
# TRACEWEAVER: file-role=driver-free-abi-verification; req=REQ-THERMAL-012; trace=TRACE-THERMAL-012; ver=VER-THERMAL-010
"""Verify pinned RUSD layouts without a driver, GPU, linking or executable.

REQ-THERMAL-012: exact pinned source-layout verification.
"""
import argparse
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import resource
import shutil
import signal
import ssl
import stat
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parents[2]
LOCK = Path(__file__).with_name("rusd-headers.json")
PROFILES = (("590.48.01", 590, "2ccbad25e1af6a6ee6f38cf569f89f8b65d658ab"),
            ("595.99.02", 595, "e394a404108a9ce5bba988c1dddf694dabbd4a43"),
            ("615.71.09", 615, "61dcc93722ecb418bb5f2e00923f05b4b8051dd1"))
ORIGIN = "https://raw.githubusercontent.com/NVIDIA/open-gpu-kernel-modules/"
FILE_LIMIT = 256 * 1024
TOTAL_LIMIT = 2 * 1024 * 1024
REQUEST_TIMEOUT = 10


class AbiError(Exception):
    pass


def valid_path(path):
    return (isinstance(path, str) and bool(path) and "\\" not in path
            and ":" not in path and not path.startswith("/")
            and all(part not in ("", ".", "..") for part in path.split("/"))
            and str(PurePosixPath(path)) == path and path.endswith(".h")
            and re.fullmatch(r"[A-Za-z0-9_./-]+", path) is not None)


def validate_lock(data):
    if not isinstance(data, dict) or set(data) != {"schema_version", "profiles"} or type(data.get("schema_version")) is not int or data.get("schema_version") != 1:
        raise AbiError("invalid header lock schema")
    profiles = data.get("profiles")
    if not isinstance(profiles, list) or len(profiles) != len(PROFILES):
        raise AbiError("exactly three pinned profiles required")
    total = 0
    for profile, expected in zip(profiles, PROFILES):
        if not isinstance(profile, dict) or set(profile) != {"version", "profile", "commit", "headers"} or tuple(profile.get(k) for k in ("version", "profile", "commit")) != expected:
            raise AbiError("unreviewed header profile/commit")
        headers = profile.get("headers")
        if not isinstance(headers, list) or len(headers) != 34:
            raise AbiError("incomplete 34-header dependency closure")
        seen = set()
        for entry in headers:
            if not isinstance(entry, dict) or set(entry) != {"path", "size", "sha256"}:
                raise AbiError("invalid header record")
            path, size, digest = (entry.get(k) for k in ("path", "size", "sha256"))
            if not valid_path(path) or path in seen:
                raise AbiError("invalid or duplicate header path")
            if type(size) is not int or not 0 < size <= FILE_LIMIT:
                raise AbiError("header byte length exceeds policy")
            if not isinstance(digest, str) or re.fullmatch(r"[0-9a-f]{64}", digest) is None:
                raise AbiError("invalid header digest")
            seen.add(path)
            total += size
    if total > TOTAL_LIMIT:
        raise AbiError("header closure exceeds total byte limit")
    return data


def load_lock(path=LOCK):
    try:
        with open(path, "rb") as stream:
            raw = stream.read(128 * 1024 + 1)
        if len(raw) > 128 * 1024:
            raise AbiError("header lock too large")
        return validate_lock(json.loads(raw))
    except (OSError, ValueError, TypeError) as error:
        raise AbiError("cannot read valid header lock") from error


def header_url(profile, entry):
    if tuple(profile.get(k) for k in ("version", "profile", "commit")) not in PROFILES or not valid_path(entry.get("path")):
        raise AbiError("unreviewed source URL")
    return ORIGIN + profile["commit"] + "/" + entry["path"]


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, msg, headers, newurl):
        raise AbiError("pinned header redirect rejected")


def make_opener():
    # create_default_context honors SSLKEYLOGFILE; this explicit context does not.
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    context.load_default_certs()
    return urllib.request.build_opener(urllib.request.ProxyHandler({}),
        urllib.request.HTTPSHandler(context=context), NoRedirect())


def remaining(deadline):
    seconds = deadline - time.monotonic()
    if seconds <= 0:
        raise AbiError("header verification overall deadline expired")
    return seconds


def check_bytes(body, entry):
    if len(body) != entry["size"] or hashlib.sha256(body).hexdigest() != entry["sha256"]:
        raise AbiError("pinned header length/digest mismatch")


@contextmanager
def request_deadline(seconds):
    # Single-process Unix CLI: bound DNS and slow-drip reads, not just idle sockets.
    def expired(signum, frame):
        raise TimeoutError("public header request deadline")
    previous_handler = signal.signal(signal.SIGALRM, expired)
    previous_timer = signal.setitimer(signal.ITIMER_REAL, seconds)
    started = time.monotonic()
    try:
        yield
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, previous_handler)
        if previous_timer[0]:
            signal.setitimer(signal.ITIMER_REAL,
                max(0.000001, previous_timer[0] - (time.monotonic() - started)), previous_timer[1])


def download_header(url, entry, deadline):
    suffix = url[len(ORIGIN):] if url.startswith(ORIGIN) else ""
    commit, separator, path = suffix.partition("/")
    if not separator or commit not in {p[2] for p in PROFILES} or not valid_path(path) or path != entry["path"]:
        raise AbiError("unpinned header URL rejected")
    if type(entry["size"]) is not int or not 0 < entry["size"] <= FILE_LIMIT:
        raise AbiError("invalid download byte limit")
    opener = make_opener()
    for attempt in range(2):
        timeout = min(REQUEST_TIMEOUT, remaining(deadline))
        try:
            with request_deadline(timeout), opener.open(url, timeout=timeout) as response:
                if response.geturl() != url:
                    raise AbiError("pinned header redirect rejected")
                body = bytearray()
                digest = hashlib.sha256()
                while True:
                    remaining(deadline)
                    chunk = response.read(min(16384, entry["size"] + 1 - len(body)))
                    remaining(deadline)
                    if not chunk:
                        break
                    body.extend(chunk)
                    digest.update(chunk)
                    if len(body) > entry["size"]:
                        raise AbiError("pinned header oversized response")
                if len(body) != entry["size"] or digest.hexdigest() != entry["sha256"]:
                    raise AbiError("pinned header length/digest mismatch")
                return bytes(body)
        except urllib.error.HTTPError as error:
            if error.code not in (408, 429, 500, 502, 503, 504) or attempt:
                raise AbiError("public header HTTPS request failed") from error
        except (urllib.error.URLError, TimeoutError, OSError) as error:
            if attempt:
                raise AbiError("public header HTTPS request unavailable within bounds") from error
    raise AbiError("public header download failed")


def ensure_cache_root(root):
    root = Path(root).absolute()
    # Reject symlinks along the supplied path before opening any cache directory.
    for parent in reversed((root, *root.parents)):
        if parent.exists() or parent.is_symlink():
            if stat.S_ISLNK(parent.lstat().st_mode):
                raise AbiError("symlink cache parent rejected")
    try:
        root.mkdir(mode=0o700, parents=True, exist_ok=True)
        info = root.lstat()
        if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid():
            raise AbiError("cache root must be an owned directory")
    except OSError as error:
        raise AbiError("cache root must be an owned directory") from error
    return root


def materialize_header(cache, profile, entry, deadline):
    remaining(deadline)
    relative = profile["commit"] + "/" + entry["path"]
    header_url(profile, entry)
    root = ensure_cache_root(cache)
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
    directory = os.open(root, flags)
    leaf = relative.split("/")[-1]
    temporary = None
    try:
        for part in relative.split("/")[:-1]:
            try:
                os.mkdir(part, 0o700, dir_fd=directory)
            except FileExistsError:
                pass
            child = os.open(part, flags, dir_fd=directory)
            if os.fstat(child).st_uid != os.getuid():
                os.close(child)
                raise AbiError("cache parent must be owned")
            os.close(directory)
            directory = child
        try:
            fd = os.open(leaf, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory)
        except FileNotFoundError:
            fd = None
        if fd is not None:
            with os.fdopen(fd, "rb") as stream:
                info = os.fstat(stream.fileno())
                if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_size != entry["size"]:
                    raise AbiError("cache header must be an owned regular file of exact size")
                check_bytes(stream.read(entry["size"] + 1), entry)
        else:
            body = download_header(header_url(profile, entry), entry, deadline)
            check_bytes(body, entry)
            temporary = "." + leaf + "." + uuid.uuid4().hex + ".tmp"
            fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=directory)
            with os.fdopen(fd, "wb") as stream:
                stream.write(body)
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(temporary, leaf, src_dir_fd=directory, dst_dir_fd=directory)
            temporary = None
        return root / relative
    except OSError as error:
        raise AbiError("unsafe or unavailable header cache entry") from error
    finally:
        if temporary is not None:
            os.unlink(temporary, dir_fd=directory)
        os.close(directory)


def compiler_environment():
    env = dict(os.environ)
    for key in ("CPATH", "C_INCLUDE_PATH", "CPLUS_INCLUDE_PATH", "OBJC_INCLUDE_PATH", "GCC_EXEC_PREFIX", "COMPILER_PATH"):
        env.pop(key, None)
    for key in list(env):
        if key.startswith(("LD_", "DYLD_")):
            env.pop(key)
    return env


def require_native(system, machine):
    if (system, machine) != ("Linux", "x86_64"):
        raise AbiError("RUSD ABI check requires native Linux x86_64")


def compile_profiles(cache, lock, compiler=None):
    validate_lock(lock)
    compiler = shutil.which(compiler or "cc")
    if not compiler or not os.path.isabs(compiler):
        raise AbiError("absolute installed native cc required")
    env = compiler_environment()

    def output_limit():
        resource.setrlimit(resource.RLIMIT_FSIZE, (FILE_LIMIT, FILE_LIMIT))

    def run(args, expected=None, source=None):
        print("RUSD ABI command: " + " ".join(args), flush=True)
        try:
            with tempfile.TemporaryFile(dir=cache) as stdout, tempfile.TemporaryFile(dir=cache) as stderr:
                result = subprocess.run(args, input=source, text=True, stdout=stdout, stderr=stderr,
                                        timeout=30, env=env, check=False, preexec_fn=output_limit)
                for name, stream in (("stdout", stdout), ("stderr", stderr)):
                    stream.seek(0)
                    captured = stream.read(FILE_LIMIT + 1)
                    if len(captured) > FILE_LIMIT:
                        raise AbiError("compiler diagnostics exceeded byte limit")
                    setattr(result, name, captured.decode("utf-8", errors="replace"))
        except (OSError, subprocess.SubprocessError) as error:
            raise AbiError("native compiler verification failed") from error
        if expected is None and result.returncode != 0:
            raise AbiError("native compiler verification failed")
        if expected is not None and (result.returncode == 0 or expected not in result.stderr):
            raise AbiError("compiler did not reject expected ABI mismatch")
        return result

    macros = run([compiler, "-dM", "-E", "-x", "c", "-"], source="").stdout
    for definition in ("#define __linux__ 1", "#define __x86_64__ 1", "#define __SIZEOF_POINTER__ 8"):
        if definition not in macros.splitlines():
            raise AbiError("compiler target is not native Linux x86_64/64-bit pointers")
    run([compiler, "-ffreestanding", "-std=c11", "-fsyntax-only", "-x", "c", "-"],
        source='_Static_assert(sizeof(void*) == 8, "64-bit pointer ABI");\n')
    fixture = str(ROOT / "docs/fixtures/rusd-abi-layout.c")
    for profile in lock["profiles"]:
        base = Path(cache) / profile["commit"]
        include = ["-I" + str(base / path) for path in (
            "src/common/sdk/nvidia/inc", "kernel-open/common/inc", "src/nvidia/arch/nvalloc/unix/include")]
        command = [compiler, "-ffreestanding", "-std=c11", "-fsyntax-only", *include]
        run([*command, "-DRUSD_ABI_PROFILE=" + str(profile["profile"]), fixture])
        for _, wrong, _ in PROFILES:
            if wrong != profile["profile"]:
                run([*command, "-DRUSD_ABI_PROFILE=" + str(wrong), fixture], expected="NV00DE_SHARED_DATA")
        if profile["profile"] == 590:
            run([*command, fixture])
            run([*command, "-DRUSD_ABI_PROFILE=999", fixture], expected="Only exact pinned")


# TRACEWEAVER: entrypoint=main; req=REQ-THERMAL-012; trace=TRACE-THERMAL-012; ver=VER-THERMAL-010
def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache", required=True, type=Path)
    args = parser.parse_args(argv)
    try:
        require_native(platform.system(), platform.machine())
        lock = load_lock()
        print("RUSD header lock sha256=" + hashlib.sha256(LOCK.read_bytes()).hexdigest(), flush=True)
        deadline = time.monotonic() + 120
        for profile in lock["profiles"]:
            print("RUSD profile=" + profile["version"] + " commit=" + profile["commit"], flush=True)
            for entry in profile["headers"]:
                materialize_header(args.cache, profile, entry, deadline)
        compile_profiles(args.cache, lock)
        print("RUSD pinned ABI verification passed (driver-free; hardware acceptance remains separate)")
        return 0
    except AbiError as error:
        print("RUSD ABI verification failed: " + str(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
