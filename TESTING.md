# Testing CryptoLabs DC Exporter

Official build candidates use CryptoLabs' shared runner pool. Reviewed protected
`dev` and `main` pushes and manual dispatches are the only trusted inputs. Forks,
PR heads and unreviewed refs do not execute on this pool. Maintainers admit a
reviewed protected commit after reviewing contributor changes.

## Hardware-free checks

The pinned recipe uses Rust 1.88.0 in a Debian Bookworm image pinned by digest,
Linux x86-64, an ordinary container UID, two CPUs, 3 GiB memory and 256 PIDs. It
drops all capabilities, disables privilege gains and mounts only the disposable
guest source directory. The Docker daemon belongs to that ephemeral guest.
No host socket, keys or GPU devices enter the build container.

The job runs the Python boundary/export/notice tests, then the pinned driver-free
C layout assertions for exact NVIDIA `590.48.01`, `595.99.02` and `615.71.09` before
any Cargo command. It resolves the locked Linux default-feature dependency graph,
collects actual licence files, runs locked all-target release tests, and builds.
Only `test_collector_creation` and `test_timeout_behavior` are filtered, including
their integration-suite copies: these two existing tests require live NVML.
All other tests remain enabled. The build never uses `--ignored` to hide failures.

`--version` and `--help` execute without GPU discovery. HELP must retain `--tui`
and `--vram-backend`. The GLIBC import ceiling is 2.34. Neither a GPU collection
smoke nor package installation occurs in CI.

The Python checks can be run independently without compiling Rust or C:

```sh
python3 -m unittest discover -s tests -p 'test_*.py' -v
```

The thermal policy/worker and NVIDIA RM suites exercise deadlines, malformed
frames, identity, concurrency, fallback, sequence decoding and cleanup without
GPU calls. The two hardware-test filters do not establish any hardware result.

## Notice and distribution verification

The source tree retains the project and adapted NVIDIA/Philip Langdale notices.
The shared job uses `cargo metadata --locked --format-version 1 --filter-platform
x86_64-unknown-linux-gnu` with default features and follows the non-dev resolved
graph. Actual licence/attribution files are included for each selected dependency;
missing package identities, graph nodes or unresolved licensing fail the build.

The separately uploaded notice evidence retains the exact Cargo metadata, target,
feature selection, sanitized graph, selected inventory and file hashes. A fresh
verifier must independently walk the actual filtered graph and compare the exact
package-ID/name/version/source set to the emitted inventory. Inspect the resulting
notice asset and confirm exact equality with both DEBs' copyright files.
Synthetic licence fixtures are behavioral tests, never upstream licensing claims.

The distribution ZIP contains ten entries: the executable, both DEBs, BUILD_INFO,
VERSION, HELP, SYMBOLS, notices, SHA256SUMS and manifest. The manifest hashes the
other nine entries; SHA256SUMS has eight rows and excludes itself and the manifest.
Each file is limited to 128 MiB; the complete archive contents to 200 MiB.

Snapshot tests use isolated Git fixtures with signing and hooks disabled. They
verify immutable blob/mode selection, explicit per-file change approvals, complete
source/test selection, secret markers, path/symlink rejection and no Git history.
These tests do not prove effective GitHub admission or actual guest isolation.

## Hardware acceptance remains separate

RUSD remains opt-in for RTX 5090 PCI device `0x2b85`, Linux x86-64 and only the
three exact drivers above. Unknown, adjacent and suffixed versions fail closed.
Limited functional reads exist for 590 and deployed 615; the publication record
has not verified loaded-driver evidence for 595. Independent sensor accuracy,
complete GPU Burn/NCCL qualification and full workload noninterference remain
unaccepted. See [thermal methods](docs/gpu-thermal-support.md) and the separately
authorized [lab procedure](docs/validation/rtx5090-acceptance.md).

Do not deploy candidates, reset/reboot GPUs, alter drivers or workloads, or run
new probes on production/client systems as part of software verification. A
successful scrape or one-shot collection does not establish hardware health,
collector freshness or permission for a stress test.
