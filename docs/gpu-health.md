# GPU health and numeric error diagnostics

The exporter collects the current NVIDIA GPU recovery action and typed NVML query errors independently of operational measurements. A GPU can return its name, temperature and memory capacity while still requiring recovery. Successful later queries do not restore OK within that collection.

Existing dashboard state numbers remain unchanged:

| GPU_ERROR_STATE | Meaning |
|---|---|
| 0 | Current supported evidence is unavailable/invalid, or an unexpected query error prevents a reliable health decision |
| 1 | Existing OK/Warning mapping |
| 2 | Essential query failure, native GPU lost/reset required, or a valid nonzero recovery action |
| 3 | Existing VM passthrough classification |
| 5 | Existing independently established hardware/PCI/proc failure or active terminal-Xid latch |

State `5` takes precedence over `2`, then `0`, then nominal `1`. Faulted or unknown host records retain available verified identity and diagnostics, omit invalid operational measurements, and are excluded from usable GPU count and thermal probing. Recovery actions and Xid codes do not establish physical damage or an RMA requirement. The exporter does not reset, reboot, drain, restart services, or change workloads.

## Additive metrics

All new metrics are snapshot gauges. Per-GPU samples reuse the existing DCXP identity labels: `gpu`, `UUID`, `pci_bus_id`, `device`, `modelName`, `Hostname`, `DCXP_FI_DRIVER_VERSION`.

| Metric | Additional labels | Value |
|---|---|---|
| `DCXP_GPU_RECOVERY_ACTION` | `action` | Exact native recovery code, emitted only when valid |
| `DCXP_GPU_RECOVERY_ACTION_STATUS` | `status` | One sample of `1`; `valid`, `unsupported`, `unavailable`, or `invalid` |
| `DCXP_GPU_NVML_ERROR` | `query`, `error`, `code` | One sample of `1` per distinct fixed typed query/error/code in the current collection |
| `DCXP_GPU_XID_CODE` | `code` | Occurrences of a numeric Xid in the currently read kernel-log snapshot |
| `DCXP_GPU_XID_SOURCE_STATUS` | `source`, `status` | One sample of `1` describing selected-source read coverage |
| `DCXP_GPU_XID_CODES_TRUNCATED` | None | `1` if more than 64 distinct Xid codes were observed for this GPU; otherwise `0` |

Recovery action interpretations are fixed: `0` `none`, `1` `gpu_reset`, `2` `node_reboot`, `3` `drain_p2p`, `4` `drain_and_reset`, `5` `recover_imex_domain`, `6` `bus_reset`, `7` `system_reboot`; a future valid integer uses `unknown` and remains state `2`. Unsupported recovery coverage never becomes a valid zero. Field `230` is read from the same loaded NVML provider as its device handle. Numeric conversion checks the native tag and full value before narrowing, including native unsigned long overflow.

The fixed query vocabulary is `device_by_index`, `name`, `uuid`, `pci_info`, `temperature`, `memory`, `ecc_mode`, `ecc_corrected`, `ecc_uncorrected`, `power_usage`, `power_limit`, `power_constraints`, `sm_clock`, `memory_clock`, `utilization`, `encoder_utilization`, `decoder_utilization`, `fan_speed`, `throttle_reasons`, `pcie_tx`, `pcie_rx`, `pcie_link_speed`, `nvlink_state`, `compute_processes`, `graphics_processes`, `compute_capability`, `recovery_action`.

Error labels are fixed classes such as `gpu_lost`, `reset_required`, `permission_denied`, `not_supported`, `conversion_error`, `invalid_data` and `symbol_unavailable`; library messages and paths are never labels. A known unambiguous native return uses its decimal code, including GPU lost `15` and reset required `16`. Wrapper conversion payloads and missing-symbol diagnostics use `code="unknown"`. Repeated NvLink slot outcomes collapse into one diagnostic tuple.

Optional unsupported coverage does not establish a defect. Unexpected errors downgrade health. NvLink InvalidArgument is exempt only for a context-proven absent slot: slots 12–17 on a known A100, or a model known to have no NvLink such as L40/L4. An unexpected InvalidArgument in an in-range slot or an undocumented NotFound is unknown health; native `15`/`16` always wins.

A device-handle acquisition failure has no verified UUID/BDF. It appears only as an anonymous `DCXP_GPU_NVML_ERROR{query="device_by_index",...}` with empty UUID/BDF. Its `gpu`/`device` labels identify enumeration context, not a verified physical card. It adds no legacy health row, GPU count, operational telemetry or thermal eligibility. PCI fallback records are independent; errors are never joined by enumeration index alone. A missing native BDF may join inventory through a unique verified UUID in PCI `/proc` evidence. Conflicting verified UUIDs or duplicated UUIDs cannot join a physical record, even if one BDF appears unique. Otherwise available UUID/BDF, current recovery action/status and query diagnostics remain in the separate diagnostic collection, with no duplicate legacy health row or thermal eligibility. A queried partial record emits one recovery status even when it has multiple query errors.

## Snapshot coverage and state transitions

Kernel-log source labels are `flagged_dmesg`, `plain_dmesg`, `kern_log`, or `none`. Status is `valid`, `unavailable`, or `truncated`. An empty successful read is valid; no readable source is unavailable. Valid means the selected source was read, not that its retained history is complete. The lowest 64 distinct numeric codes are emitted in stable order; omission beyond that limit is explicit. Repeated scrapes of an unchanged snapshot do not accumulate counts.

Full and functionless NVIDIA Xid PCI tokens are accepted. A functionless token is assigned only when inventory identifies exactly one PCI function; missing or ambiguous identities cannot assign an error to another card. Terminal changes are compared by original source token before diagnostics are mapped to unambiguous PCI inventory.

Current recovery/query evidence applies to the current snapshot. Healthy→Reset→valid `0` can report `1`→`2`→`1`; Reset→supported unavailable reports `2`→`0`. Unsupported recovery after a current fault keeps that unresolved nonhealthy status until supported valid `0` and nominal queries, a definite new outcome, or collector restart. This cache uses verified UUID and PCI identity, not enumeration index; a different card at the same BDF cannot inherit another card's recovery fault. An earlier fault seen with a unique verified UUID and missing BDF remains unresolved when that same UUID later joins a unique BDF with unsupported recovery coverage. Duplicated or ambiguous UUIDs cannot provide this bridge.

The preexisting terminal-Xid latch remains separate. A new terminal `79` stays state `5` despite later recovery `0`, unchanged history, or rotation/unavailable logs. A startup historical `79` follows the existing baseline policy while its numeric observation remains visible. Historical `154` alone creates no active-fault latch. Existing aggregate Xid metric semantics are retained; neither aggregate nor per-code absence proves a clean hardware history.

Software fixtures validate these decisions without a GPU. Real driver coverage, deployed build identity, Grafana behavior and operator acceptance require separate read-only evidence after an authorized deployment.

## VM passthrough correction candidate

The source candidate requires semantic tests, a locked shared build and independent review before operational validation. State `3` requires positive reservation or assignment evidence: a current `vfio-pci` or `pci-stub` binding, a verified QEMU executable with a matching vfio-pci device option, or a running libvirt domain's PCI hostdev **source** address. A missing NVIDIA proc entry, missing driver binding or healthy sibling cannot establish VM assignment. An unexplained unavailable GPU remains Error `2` with unavailable measurements and no usable-GPU contribution; that status alone does not establish a physical defect.

Guest PCI target addresses belong to the guest namespace. Guest readings are eligible only when the full canonical guest BDF maps unambiguously to the assigned host source BDF in current inventory. XML order and guest enumeration index cannot identify a physical host card. Missing, malformed, duplicate or conflicting mappings retain assignment-only information with unavailable telemetry; they cannot move a sample or clear another host GPU's fault. Detection remains possible without a guest agent. Current fault precedence and terminal-Xid semantics remain in force; historical Xid observations alone do not create an active fault.

The [VM acceptance definition](validation/vm-passthrough-acceptance.md) requires parent-host and reserved-guest evidence. A normal host collection probes all running guests, so a live test requires a dedicated host without unrelated client VMs or a separately reviewed diagnostic harness restricted to the reserved domain. Operational acceptance remains **HELD**; software fixtures provide no live acceptance evidence.
