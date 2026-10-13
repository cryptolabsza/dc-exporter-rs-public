# RTX 5090 acceptance definition

Status: pending execution and review; this document records a procedure, not an outcome. The project owner and authorized lab operator rate the results and record any implementation participation. One independent reviewer checks the evidence before a release-level claim. Experimental profile: NVIDIA driver `590.48.01`, PCI device `0x2b85`, Linux x86-64; other profiles remain unsupported.

**VAL-THERMAL-001 (N1; R1–R10 and R13):** does the same-card measured memory temperature update correctly, remain distinguishable from unsupported/stale/placeholder data, preserve established dashboards and avoid unbounded optional collection?

Use V1 and V8 on a separately approved safe test card/environment. Pin identity, board/firmware/driver/kernel, exporter commit/artifact, public source hashes and exact configuration. Record raw signed24.8 values, update markers and decoded values. Compare same update markers where possible; otherwise matched steady-state Linux probe/exporter windows use the provisional 2°C band. Unit rounding error is at most 0.5°C on one captured unrounded sample, not a bound between asynchronous samples. Repeat idle, memory-heavy, compute-heavy and cooldown three times only in an authorized lab. Windows matched steady states use the provisional 3°C band; unmatched conditions are inconclusive. Per-chip comparison is conditional on a separately validated path and is not required to implement per-chip acquisition now. Run the 60-minute normal-cadence soak, preserve new errors, helper/FD counts and one-shot/HTTP/TUI output. The owner assesses representative Prometheus/Grafana output. Failures or inconclusive required evidence keep acceptance held.

**VAL-THERMAL-002 (N2; R2, R3, R5, R10–R12):** can another maintainer identify exact supported profiles, distinguish candidates from validated support, run hardware-free checks, and reproduce the evidence-based process without confidential or guessed definitions?

Inspect V2/V3/V5/V7/V9/V10 evidence and the compatibility record for pinned sources, ABI layout, units/freshness limitations, exact validated scope, rollback and rejected unknown profiles. A software fixture pass cannot establish Linux ioctl execution or sensor identity. Every enabled/promoted profile must show the actual outcome, including unsupported or pending.

Expected private evidence root: `validation/rtx5090/<run-id>/` outside Git unless sharing is explicitly authorized. Each run records requirement/procedure/validation IDs, tested candidate, environment/configuration, expected/actual result, pass/fail/inconclusive, deviations and artifact locations. Never put secrets in records. Do not write an empty placeholder file and label it RED or hardware proof.

All client/production hosts are excluded from this execution plan. No SSH, new probes, workload changes, driver controls or reboot are authorized by this definition. In particular do not repeat the blocked sysfs `driver_override` read. No stable RTX 5090 main/tag/release promotion until actual evidence is accepted; the owner controls that decision and publication. Experimental source visibility requires the separate public-tree/CI disclosure gates.

## Lab readiness, driver interaction and recovery

Use only a separately authorized idle lab card. Verify the exact reviewed artifact
hash and software results, pinned public source hashes/ABI, full BDF/UUID, driver,
board/firmware/kernel and configuration before any sensor execution. Unknown or
mismatched profiles must fail closed. Begin with legacy and BAR access disabled;
opt into RUSD/auto only after that lab baseline is reviewed. Keep an external
operator deadline, rollback and a stop condition for new errors or workload impact.
The optional helper deadline does not bound existing global collectors.

V10 requires permission-denial and controlled driver-unbind/quiescence evidence
only in the authorized lab: verify per-batch RM mappings/objects/FDs detach and
close, no replacement is launched while a kernel-blocked helper remains alive,
and supported collection resumes only after the exact identity/profile is again
valid. A kill request does not prove a blocked kernel call exited. Record helper
counts, resource/error differences and actual rollback/recovery results. Driver
controls, workload tests, reboot or OS switching require explicit lab authorization.

Retain the V1/V8/V10 identifiers in result records along with this procedure's
VAL-THERMAL-001/002 questions. Passing software fixtures, a plausible scalar or
a zero scanner result cannot replace actual-card outcomes. Unsupported or
inconclusive required results keep hardware acceptance and stable promotion held.
