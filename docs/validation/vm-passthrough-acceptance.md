# VM passthrough acceptance definition

Status: **operational acceptance HELD**. This is an acceptance definition; it supplies no semantic RED/GREEN, build, review or live pass result and authorizes no rental, command execution or deployment.

Scope: `VM-FIX-001`–`VM-FIX-006`, `TRACE-VM-001`–`TRACE-VM-006`, `VER-VM-001`–`VER-VM-006`; validation `VAL-VM-001` / `VM-VAL-001`; procedure `ATP-VM-001`; reserved result `RESULT-VM-001`. Acceptance owner: Hannes Zietsman. Implementation and software review cannot substitute for explicit operator acceptance.

The goal is to distinguish a reserved or assigned GPU from an unavailable host GPU and attach guest telemetry to the correct physical GPU while preserving current faults. An observed VM configuration cannot qualify every hypervisor, physical hardware condition or guest-memory sensor.

## Preconditions before a live run

1. Retain semantic RED against the old behavior and GREEN using the same production-used tests, health/thermal/parser/formatter regressions, pinned ABI checks, a locked shared-pool build, dependency notices and exact export checks. Bind raw evidence to source and artifact SHA-256 identities. A compile failure alone is not semantic RED. Fixtures are software evidence, not live proof.
2. Complete independent review and fresh verification of the exact patch and resolve blocking findings before an endpoint run.
3. Identify and authorize one reserved guest and its parent host: host identity, domain name/UUID, exact assigned host-source and guest-target BDFs, GPU UUID/device ID, host/guest drivers, artifact and permitted access. Guest SSH alone cannot prove hypervisor assignment detection. Obtain rental-cost approval separately where needed.
4. Use a dedicated host with no unrelated client guests, or a separately reviewed diagnostic harness that constrains every guest command to the reserved domain. The ordinary full collector probes all running guests. A temporary HTTP port or guest SSH restriction alone cannot bound those probes.
5. Define the temporary endpoint, maximum collection count and duration, expected physical/usable GPU counts, permitted command outcomes, evidence paths and cleanup before execution. Keep register access disabled. The validated guest BAR profile set is empty: guest BAR reads stay disabled regardless of flag setting. Preserve the production exporter process, binary/configuration, boot state and unrelated workload identities.

Stop on the first identity mismatch, wrong-row telemetry, unexpected register access or guest command, unexplained count change, new current-fault evidence, unexpected command error, production-state change, or collection/time limit. Preserve the evidence and mark the observation failed or deferred. An expected unavailable-agent outcome may demonstrate assignment-only behavior; it cannot be treated as valid telemetry or used to suppress other errors.

No stress, manufactured failures, GPU reset/rebind, driver changes, reboot, production service restart or unrelated guest execution belongs to this procedure. Fault cases remain deterministic fixtures. Installing or enabling a guest agent requires its own concrete authorization.

## ATP-VM-001 observations

| Case | Required outcome | Evidence | Result |
|---|---|---|---|
| Assignment without an agent; VM-FIX-002 | Exact positively assigned GPU remains identifiable as state `3` without invented guest readings; physical/usable counts match the reviewed contract | Parent running-domain source XML or verified QEMU evidence, current PCI binding, host/guest identities and exact scrape | PENDING |
| Mapped telemetry with an already permitted agent; VM-FIX-003 | Valid available guest values attach only to the uniquely assigned host BDF/UUID; other rows keep their identity, state and errors | Guest PCI/UUID/driver evidence, host inventory, mapping, agent output and same-collection formatted output | PENDING |
| Unavailable host evidence; VM-FIX-001 | Missing-proc/unbound/nouveau fixture remains Error `2` and unusable across collections, independent of sibling evidence; stale assignment is removed | Production-used row/formatter fixtures, usable counts and raw semantic RED/GREEN | PENDING; fixture only |
| Current faults and history; VM-FIX-004 | Current established faults retain precedence; historical Xid alone cannot create an active failure or transfer by guest index | Stateful merge/formatter fixtures and health regressions | PENDING; fixture only |
| Register policy; VM-FIX-005 | Disabled/unsupported guest reads and non-owned host reads are never invoked; valid independently supplied mapped readings are preserved | Recording-reader fixtures, no-register setting, empty guest BAR profile set and exact source | PENDING |
| Preserved contracts and cleanup; VM-FIX-006 | Physical/usable counts, metric/status/version/default/TUI/branding contracts remain intact; only temporary resources are removed | Regression/export/notices evidence, before/after production/workload state and temporary process/file/endpoint cleanup | PENDING |

Missing identity or mapping evidence must retain assignment-only information and unavailable telemetry. Do not manufacture a valid measurement, infer healthy hardware from a usable count, or infer a physical defect from Error `2`. Supplied temperature fixtures do not establish new RTX5090/Ada guest VRAM support.

## Acceptance record

Retain a structured result bound to `RESULT-VM-001`, `ATP-VM-001`, all six work/trace/verification IDs and `VAL-VM-001` / `VM-VAL-001`. Include source/artifact hashes, authorized run identity, UTC start/end, parent/guest/BDF/UUID/driver evidence, collection and duration limits, expected/actual counts and command outcomes, every observation's result, raw evidence hashes, before/after state, cleanup, limitations and the owner's explicit pass/fail/deferred decision and date.

Cleanup must confirm that only the approved temporary processes, endpoint and files were removed and production/workload identities remain unchanged. A failed, partial or unavailable observation remains pending. Successful fixtures, definition review or a build cannot close operational acceptance. Rental, live execution, production promotion and public release remain separate decisions.
