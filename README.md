# DC Exporter (Rust)

Linux GPU metrics exporter for Prometheus, with an HTTP endpoint, one-shot output
and a terminal UI. It collects NVML metrics, PCIe errors, VM passthrough state,
and optional memory/hotspot temperatures.

The development tree is version **0.2.7**. `legacy` is the default thermal policy.
RTX 5090 RUSD memory telemetry is **opt-in and experimental**; real-card accuracy,
permissions, cleanup, workload noninterference and Linux/Windows comparison remain
pending. Software tests do not establish hardware support.

## Downloads and source

- [Stable binaries and DEB packages](https://github.com/cryptolabsza/dc-exporter-releases/releases)
- [Development artifacts](https://github.com/cryptolabsza/dc-exporter-releases/releases/tag/dev-latest)
- [Source](https://github.com/cryptolabsza/dc-exporter-rs-public)

Existing stable assets have their original provenance. They are not evidence of
RTX 5090 acceptance or builds from the fresh source history.

## Build and verify

Linux x86-64 is the runtime target. CI pins Rust 1.88.0 and native Debian 12;
release artifacts must not import GLIBC symbols newer than 2.34.

```sh
git clone https://github.com/cryptolabsza/dc-exporter-rs-public.git
cd dc-exporter-rs-public
cargo +1.88.0 build --release
./target/release/dc-exporter --version
./target/release/dc-exporter --help
```

See [TESTING.md](TESTING.md) for hardware-free checks and the separately authorized
[RTX 5090 lab acceptance procedure](docs/validation/rtx5090-acceptance.md).

## Run on an authorized GPU system

```sh
# HTTP metrics server; default port 9835 and default legacy thermal policy
./target/release/dc-exporter --port 9835
curl http://localhost:9835/metrics

# Collect once, or use the terminal UI
./target/release/dc-exporter --one-shot
./target/release/dc-exporter --tui

# Disable BAR register access while retaining NVML telemetry
./target/release/dc-exporter --no-register-access
```

The HTTP listener binds all interfaces. Restrict exposure through the host's
network policy. TUI keys include arrows/Tab to select GPUs, Enter for details,
Space to pause and `q` to quit.

The optional thermal controls are `--vram-backend legacy|auto|nvml|rusd|off`,
`--thermal-timeout-ms` and `--thermal-max-sample-age-ms`. Explicit thermal CLI
options override `[thermal]` in the configuration file in every mode. Other
existing TOML sections retain their CLI-controlled behavior.

```toml
[thermal]
vram_backend = "legacy"
timeout_ms = 2000
# max_sample_age_ms = 30000
```

`auto` tries the exact NVML memory field, then the supported RUSD candidate only
on explicit unsupported, then a permitted legacy reading. Permission, device-loss,
invalid and stale results stop fallback. `rusd` uses RUSD only; `nvml` uses the
exact NVML memory field only; `off` suppresses VRAM telemetry, including guests.
RUSD is limited to NVIDIA driver **590.48.01**, PCI device **0x2b85** and Linux
x86-64. Unknown profiles fail closed. Neither optional method adds hotspot
support or needs `/dev/mem` or a kernel-parameter change.

Legacy BAR temperature reads require permitted `/dev/mem` mapping on supported
GPUs. Changing kernel permissions or `iomem` settings is an administrator's
separate decision. RTX 50-series legacy register access remains disabled.

## Metrics and limitations

The endpoint retains established metric names and labels, including:

- `DCXP_FI_DEV_VRAM_TEMP` and `DCXP_FI_DEV_HOT_SPOT_TEMP` in Celsius.
- `DCXP_FI_DEV_VRAM_TEMP_VALID`: one for a valid current memory measurement.
- `DCXP_FI_DEV_VRAM_TEMP_BACKEND_INFO`: selected backend and status labels.
- `DCXP_GPU_SUPPORTED`: one for current measured, non-approximated memory data.
- `dc_exporter_build_info`: version, build timestamp and source hash.

An unavailable scalar temperature remains zero. Use validity/status alongside
it to distinguish failure from measured zero. The optional helper has a bounded
parent deadline and one active child; a kernel-blocked child can survive a kill
request. Existing global NVML, VM and system collection is not made bounded by
this feature. Passthrough and noninterference claims need actual environment
acceptance. See [thermal methods](docs/gpu-thermal-support.md) and
[pinned ABI provenance](docs/gpu-thermal-abi.md).

## Install and operate

Download the binary or DEB from the release repository, verify its published
SHA-256 and provenance, and install it under your own change-control policy.
The binary package name is `dc-exporter-rs`; the built Cargo binary is
`dc-exporter`. The packaged service is `dc-exporter.service`.

```sh
sudo dpkg -i dc-exporter-rs_VERSION_amd64.deb
sudo systemctl status dc-exporter
journalctl -u dc-exporter -n 50
curl http://localhost:9835/metrics
```

The supplied service runs with the permissions needed by legacy register access.
Review that service before installing it; NVML/RUSD device permissions are a
separate requirement. Do not deploy test candidates to client workloads.

## Contributing and releases

Write a focused failing test before implementation, keep changes on `dev`, and
leave the dev version unchanged. Version bumps occur only when merging an
accepted release to `main`. See [RELEASING.md](RELEASING.md).

Public PR/fork code is not admitted to persistent self-hosted runners. Trusted
builds and bytes-only publication require reviewed protected refs and verified
runner/secret policy. See [public snapshot and publication](docs/public-publishing.md).

## License

The project declares MIT; see [LICENSE](LICENSE). Adapted NVIDIA and Philip
Langdale definitions retain their notices in [ABI provenance](docs/gpu-thermal-abi.md).
The legacy register implementation is preserved; its disclosure/provenance
review is a separate owner gate before public visibility.
