# Testing DC Exporter

## Hardware-free verification

Use Linux x86-64 and Rust 1.88.0 for the release target. Run the following checks
without GPU devices, `/dev/mem`, privileged sockets, sudo or unrelated credentials:

```sh
python3 -m unittest discover -s tests -p 'test_*.py' -v
cargo +1.88.0 test --release --all-targets -- \
  --skip test_collector_creation --skip test_timeout_behavior
cargo +1.88.0 build --release
./target/release/dc-exporter --version
./target/release/dc-exporter --help
```

Only two existing live-NVML tests are skipped, including their copies compiled
into integration suites: `test_collector_creation` and `test_timeout_behavior`.
They call live collector/availability paths and require separate lab authorization.
All pure tests and child fixtures remain enabled. `--version`/`--help` avoid GPU
inventory and driver calls. CI requires `/usr/bin/python3` and native Debian 12;
its artifact gate rejects GLIBC imports newer than 2.34.

The thermal policy/worker and NVIDIA RM suites check deadlines, malformed frames,
identity, concurrency, fallback, sequence decoding and cleanup without GPU calls.
A real helper smoke test uses an unsupported profile and must exit before normal
exporter startup. Reproduce pinned C layout assertions using
[the public ABI fixture](docs/gpu-thermal-abi.md#software-verification-and-remaining-claims).

The snapshot tests use isolated scratch Git repositories with signing and hooks
disabled. They check commit-only selection, exact hashes/modes, complete code/test
selection, explicit documentation/CI changes, private content, path/symlink
rejection and absence of Git history. They do not prove live GitHub or VM policy.

## When an RTX 5090 candidate can be tested

A candidate is ready for a **separately authorized idle lab card** only after its
exact Linux artifact passes software checks, independent review and profile/ABI
checks, with a recorded rollback and operator permission. Pin NVIDIA driver
**590.48.01**, PCI device **0x2b85**, Linux x86-64, full BDF/UUID, exporter hash and
configuration. Other profiles must return unsupported.

Start with existing default `legacy` behavior and BAR access disabled, then opt
into `rusd` or `auto` only on that lab card. Use an external operator timeout as
well as the optional helper deadline. The helper deadline does not bound the
existing global NVML/VM/system collectors or prove noninterference.

```sh
# On the authorized lab system only; use the exact reviewed candidate path.
timeout 15s ./dc-exporter --one-shot --no-register-access --vram-backend legacy
# Only after the baseline is accepted for that lab session:
timeout 15s ./dc-exporter --one-shot --no-register-access \
  --vram-backend rusd --thermal-timeout-ms 2000
```

Collect expected/actual results, current validity/status and same-card comparison.
Follow the full [RTX 5090 acceptance procedure](docs/validation/rtx5090-acceptance.md)
for repeated steady states, soak, permissions, cleanup, controlled unbind and
Linux/Windows evidence. Inconclusive evidence keeps acceptance held. One-shot
success alone does not authorize service installation, stable release or client
workload testing.

## Production boundary

Client/production hosts are excluded. Do not reboot, deploy candidates, change
workloads/services/drivers, run new probes or issue RM controls there. Do not
repeat the blocked sysfs `driver_override` read. Only separately approved lab
work may exercise driver interaction, load, unbind or OS comparison; preserve
resource/quiescence and rollback evidence. No actual-card acceptance is claimed.
