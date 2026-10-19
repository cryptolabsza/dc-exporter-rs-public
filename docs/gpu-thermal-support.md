<!-- TRACEWEAVER: file-role=thermal-support-guide; req=REQ-THERMAL-012; trace=TRACE-THERMAL-012; ver=VER-THERMAL-010 -->
# GPU thermal methods

The development build adds opt-in aggregate memory temperature. `legacy` remains the default and never starts the new helper. RTX 5090 RUSD support is an experimental candidate; software tests do not establish hardware accuracy or production noninterference.

| Method | Behavior |
| --- | --- |
| `legacy` | Existing BAR and guest paths; RTX 50-series BAR offsets stay disabled. |
| `auto` | Try NVML memory field 82; on explicit unsupported, try the candidate RUSD profile; on unsupported, retain a permitted legacy reading. |
| `nvml` | Exact NVML memory-temperature field only; no silent fallback. |
| `rusd` | NVIDIA RM shared-data memory entry only; no NVML or BAR fallback. |
| `off` | Explicitly suppress VRAM temperature on hosts and guests; keep core and hotspot collection. |

NVML per-field permission, invalidity, device loss and stale results stop fallback. RUSD is limited to Linux x86-64, NVIDIA exact `590.48.01`, `595.99.02` or `615.71.09`, PCI device `0x2b85`. Adjacent versions, suffixed versions and unknown 6xx profiles are unsupported. The candidate remains subject to the [real-card acceptance procedure](validation/rtx5090-acceptance.md) and the [pinned public ABI provenance](gpu-thermal-abi.md). The separately authorized 590 idle smoke returned 14 valid readings across seven cards in two serial passes (34–36°C), with clean helper exits and unchanged observed GPU error/idle state. Independent accuracy, raw comparison, full integration, GPU Burn/NCCL qualification and soak remain pending. Loaded-driver evidence for595 has not been verified for this publication record. Fresh deployed functional memory readings exist for615 on operational GPUs; these observations do not establish full accuracy or workload noninterference. No full hardware acceptance result is claimed.

```toml
[thermal]
vram_backend = "legacy"
timeout_ms = 2000
# max_sample_age_ms = 30000
```

Explicit `--vram-backend`, `--thermal-timeout-ms` and `--thermal-max-sample-age-ms` override TOML in HTTP, one-shot and TUI modes. The default maximum age is `max(3 * collection interval, deadline)`; the deadline must be 1–60000 ms and an explicit age must be positive. Other existing TOML sections remain dormant. `--no-register-access` disables direct BAR access in every mode and does not disable NVML/RUSD. RUSD requires appropriate NVIDIA device permissions, not `/dev/mem` or `iomem=relaxed`.

The persistent parent manager owns at most one helper process. Each batch opens and releases its RM resources, sends one bounded framed result and exits. Requests and replies join by normalized full PCI domain/bus/device/function and current UUID. Only reconciled healthy NVIDIA-bound host GPUs are eligible; failed, unavailable, guest, passthrough and unbound records are excluded. Replies must contain exactly one current identity per request.

Auto/NVML/RUSD leave existing guest-agent temperatures, source and provenance unchanged. `off` is the explicit global VRAM opt-out. Unsupported NVML per-field results preserve their error status even when NVIDIA leaves the sample timestamp unset; permission and device-loss errors never trigger fallback.

Freshness is checked again after other devices, NVML shutdown and RUSD cleanup. NVML retains the real field timestamp in the wrapper's documented Unix microseconds. RUSD conservatively charges the entire RM collection duration as an upper bound on age after observed first update progress; its driver marker is never treated as an absolute clock. Valid wire results must carry a finite remaining validity budget no larger than the requested maximum age. The parent subtracts its entire elapsed round trip, including successful child exit, before accepting a measurement. This deliberately charges overlapping child time twice and can reject fresh samples with very short age limits. Expired results become unavailable/Stale; absent or invalid budgets are rejected.

The optional batch deadline includes parent pipe I/O and waiting for successful helper exit. Timeout requests termination and returns unavailable telemetry; replacement waits for observed child exit. A process stuck in a kernel driver call may remain alive despite a kill request. These controls do not bound the exporter's existing global NVML, VM or system collectors, and do not establish production noninterference.

Existing scalar names and labels remain intact, including the established zero failure sentinel. `DCXP_FI_DEV_VRAM_TEMP_VALID` distinguishes unavailable zero from a measured zero. `DCXP_FI_DEV_VRAM_TEMP_BACKEND_INFO` exposes finite `backend` and `status` labels. `DCXP_GPU_SUPPORTED` is one only for a current valid, non-approximated memory measurement; the old datacenter placeholder zero is unsupported. TUI detail shows the selected method and status.

Development publication retains version `0.2.7` and default `legacy`. Stable main/tag/release promotion waits for accepted hardware evidence and independent review. No production probe, deployment, reboot or workload test is part of software verification.

## VM register-policy correction candidate

The source candidate's direct-register policy includes guest-agent commands: `--no-register-access` must suppress guest BAR reads as well as host BAR reads. Host legacy BAR requests require reconciled identity, current NVIDIA binding and no passthrough reservation or assignment, including inconsistent snapshots where NVML returns nominal data. Existing RUSD identity/binding gates remain in force. The flag preserves independently supplied valid mapped guest telemetry; host NVML/RUSD eligibility excludes guest GPUs. `off` remains the explicit global VRAM suppression.

Guest register eligibility is separate from host thermal compatibility. The validated guest BAR profile set is empty, so guest BAR reads remain disabled with either flag setting. A future profile requires the exact guest PCI device, architecture, driver/environment, permitted register mechanism and offsets, and validation evidence. A model-name substring or host RUSD profile cannot establish guest eligibility. No new guest RUSD backend or RTX5090/Ada guest-memory support is claimed. Supplied valid guest temperature fixtures preserve their values and provenance only after unambiguous guest-to-host BDF mapping; unsupported or missing sensors remain unavailable.

Operational acceptance remains **HELD** under the [bounded VM procedure](validation/vm-passthrough-acceptance.md). This document supplies no software/build or live pass result. Package `0.2.7`, default `legacy`, metric/status contracts, TUI and CryptoLabs branding remain the preservation contract.

Official software candidates use the protected-ref shared pool described in [TESTING.md](../TESTING.md). CI runs exact pinned driver-free ABI checks before Cargo and never probes GPUs. The terminal UI remains available through `--tui`; devversion0.2.7 and defaultlegacy are preserved.
