# CryptoLabs DC Exporter

GPU metrics for Prometheus, built by [CryptoLabs](https://www.cryptolabs.co.za/).
DC Exporter is a Linux exporter with an HTTP endpoint, one-shot collection and a
terminal UI. It combines NVIDIA telemetry, GPU recovery diagnostics, PCIe error
observations, VM passthrough state and optional memory/hotspot temperatures.

For help investigating GPU server failures, explore [CryptoLabs AI SRE](https://www.cryptolabs.co.za/services/).
Our SRE agent collects diagnostic evidence and proposes a fix plan for operator
review. The exporter itself does not reset GPUs, reboot hosts or change workloads.

## Source and downloads

- [Source and issues](https://github.com/cryptolabsza/dc-exporter-rs-public)
- [Binaries and DEB packages](https://github.com/cryptolabsza/dc-exporter-releases/releases)
- [Development channel](https://github.com/cryptolabsza/dc-exporter-releases/releases/tag/dev-latest)

The development tree remains version **0.2.7**. Existing stable **0.2.8** assets
retain their historical provenance; they are not builds of this fresh source
history. Use the source commit, artifact hash and build provenance to identify a
candidate, rather than comparing these version strings alone.

## GPU temperatures

Core telemetry comes from NVML. Memory and hotspot availability depends on the
GPU, driver, backend and device permissions. Unsupported readings are explicit;
a zero temperature value alone does not mean that a measurement succeeded.

| Memory backend | Behavior |
| --- | --- |
| `legacy` | Default. Existing permitted BAR/guest paths; RTX 50-series BAR access is disabled. |
| `nvml` | Read the exact NVML memory-temperature field; no silent fallback. |
| `rusd` | Experimental NVIDIA shared-data memory sensor; no BAR fallback. |
| `auto` | Try NVML, then an eligible RUSD profile and permitted legacy reading only on explicit unsupported results. |
| `off` | Suppress memory temperature collection, including guests. |

The experimental RUSD profiles are **Linux x86-64, RTX 5090 PCI device `0x2b85`,
and exact NVIDIA drivers `590.48.01`, `595.99.02` or `615.71.09`**. Unknown,
adjacent or suffixed versions fail closed. This does not mean “590 and newer.”

| Exact driver | Evidence available for this candidate |
| --- | --- |
| `590.48.01` | Limited authorized idle functional reads were observed. |
| `595.99.02` | Source/layout checks exist; loaded-driver functional evidence has not been verified for this publication record. |
| `615.71.09` | Fresh deployed memory readings were observed on operational GPUs. |

These observations do not establish independent sensor accuracy, completed
GPU Burn/NCCL qualification, every board/firmware combination, or full workload
noninterference. RUSD remains opt-in. See [thermal methods](docs/gpu-thermal-support.md),
[ABI provenance](docs/gpu-thermal-abi.md) and the [acceptance procedure](docs/validation/rtx5090-acceptance.md).

## Run

The default HTTP port is **9835**. The listener binds all interfaces; use the
host's network policy to restrict access to trusted monitoring systems.

```sh
# Keep NVML telemetry while disabling direct BAR register access.
dc-exporter-rs --no-register-access --port 9835
curl http://localhost:9835/metrics

# One-shot collection or terminal UI on an authorized GPU system.
dc-exporter-rs --no-register-access --one-shot
dc-exporter-rs --no-register-access --tui

# Explicit experimental RUSD selection on an eligible authorized system.
dc-exporter-rs --no-register-access --vram-backend rusd
```

The release executable is `dc-exporter-rs`; Cargo's executable is `dc-exporter`.
The DEB package and systemd service are `dc-exporter-rs` and
`dc-exporter.service`. Review the packaged service and required device permissions
before installation. RUSD does not require `/dev/mem`; legacy BAR access has its
own privileged-access requirements.

Only `[thermal]` values are currently applied from the TOML configuration file;
other existing sections retain CLI-controlled behavior. Explicit thermal CLI
arguments override TOML in HTTP, one-shot and TUI modes.

```toml
[thermal]
vram_backend = "legacy"
timeout_ms = 2000
# max_sample_age_ms = 30000
```

## Interpret GPU health

A GPU can return readable temperatures while requiring recovery. The exporter
reports current native recovery actions and fixed numeric NVML/Xid diagnostics
alongside the established `GPU_ERROR_STATE` values:

| Value | Meaning |
| --- | --- |
| `0` | Unknown or insufficient current evidence. |
| `1` | Existing nominal/warning mapping. |
| `2` | Essential query failure, GPU lost, reset required or valid nonzero recovery action. |
| `3` | VM passthrough classification. |
| `5` | Independently established hardware/PCI/proc failure or an active terminal-Xid latch. |

Recovery-required or unknown host GPUs are excluded from usable count and thermal
probing. A recovery action is evidence of a driver condition, not proof of
permanent damage or an RMA requirement. Numeric Xids describe the currently read
kernel-log snapshot, not a complete lifetime error history. See [GPU health diagnostics](docs/gpu-health.md).

Useful memory metrics include `DCXP_FI_DEV_VRAM_TEMP`,
`DCXP_FI_DEV_VRAM_TEMP_VALID` and `DCXP_FI_DEV_VRAM_TEMP_BACKEND_INFO`.
Use validity/status together with the scalar. Build identity is exposed through
`dc_exporter_build_info`; artifact SHA-256 remains the exact executable identity.

Optional thermal collection uses a bounded helper. Existing global NVML, VM and
system collectors are not all bounded by that helper deadline; a successful HTTP
scrape alone does not prove that the underlying collection is current.

## Build and contribute

Official candidates must be built from reviewed protected refs through CryptoLabs'
shared runner pool. Public fork and unreviewed PR code must not execute on the
trusted pool or receive organization secrets. Release verification targets Linux
x86-64, the pinned Rust toolchain and the GLIBC 2.34 import ceiling.

See [TESTING.md](TESTING.md), [CONTRIBUTING.md](CONTRIBUTING.md) and
[RELEASING.md](RELEASING.md). Development changes go to `dev`, tests precede
implementation, and the dev version stays unchanged. Version bumps accompany an
accepted merge to `main`.

## License and security

CryptoLabs' project code is MIT licensed; see [LICENSE](LICENSE). Adapted NVIDIA
and Philip Langdale definitions retain their notices and exact source pins in
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md) and [ABI provenance](docs/gpu-thermal-abi.md).
Dependencies retain their own licenses. See [SECURITY.md](SECURITY.md) for private
vulnerability reporting.
