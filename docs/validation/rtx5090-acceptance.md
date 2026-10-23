# RTX 5090 acceptance definition

Status: full acceptance pending execution and review. A limited 590 idle smoke is recorded below; this procedure does not claim full acceptance. The project owner and authorized lab operator rate the results and record any implementation participation. One independent reviewer checks the evidence before a release-level claim. Experimental profiles: exact NVIDIA drivers `590.48.01`, `595.99.02` and `615.71.09`, PCI device `0x2b85`, Linux x86-64; other versions, including unknown 6xx profiles, remain unsupported. Each profile requires separate actual-card evidence.

**VAL-THERMAL-001 (N1; R1–R10 and R13):** does the same-card measured memory temperature update correctly, remain distinguishable from unsupported/stale/placeholder data, preserve established dashboards and avoid unbounded optional collection?

Use V1 and V8 on a separately approved safe test card/environment. Pin identity, board/firmware/driver/kernel, exporter commit/artifact, public source hashes and exact configuration. Record raw signed24.8 values, update markers and decoded values. Compare same update markers where possible; otherwise matched steady-state Linux probe/exporter windows use the provisional 2°C band. Unit rounding error is at most 0.5°C on one captured unrounded sample, not a bound between asynchronous samples. Repeat idle, memory-heavy, compute-heavy and cooldown three times only in an authorized lab. Windows matched steady states use the provisional 3°C band; unmatched conditions are inconclusive. Per-chip comparison is conditional on a separately validated path and is not required to implement per-chip acquisition now. Run the 60-minute normal-cadence soak, preserve new errors, helper/FD counts and one-shot/HTTP/TUI output. The owner assesses representative Prometheus/Grafana output. Failures or inconclusive required evidence keep acceptance held.

**VAL-THERMAL-002 (N2; R2, R3, R5, R10–R12):** can another maintainer identify exact supported profiles, distinguish candidates from validated support, run hardware-free checks, and reproduce the evidence-based process without confidential or guessed definitions?

Inspect V2/V3/V5/V7/V9/V10 evidence and the compatibility record for pinned sources, ABI layout, units/freshness limitations, exact validated scope, rollback and rejected unknown profiles. A software fixture pass cannot establish Linux ioctl execution or sensor identity. Every enabled/promoted profile must show the actual outcome, including unsupported or pending.

Expected private evidence root: `validation/rtx5090/<run-id>/` outside Git unless sharing is explicitly authorized. Each run records requirement/procedure/validation IDs, tested candidate, environment/configuration, expected/actual result, pass/fail/inconclusive, deviations and artifact locations. Never put secrets in records. Do not write an empty placeholder file and label it RED or hardware proof.

All client/production hosts are excluded from this execution plan. No SSH, new probes, workload changes, driver controls or reboot are authorized by this definition. In particular do not repeat the blocked sysfs `driver_override` read. No promotion of RTX 5090 support to hardware-qualified status until actual evidence is accepted; the owner controls that decision and publication. The later owner-approved v0.3.0 software release may include opt-in experimental RTX 5090 support with full hardware acceptance still pending. Require an accepted protected-main build, a tag on that exact commit and verified publisher readback for the versioned software release; this definition supplies no release-completion evidence. Experimental source visibility requires the separate public-tree/CI disclosure gates.

## Bounded evidence as of 2026-10-13

| Exact profile | Evidence observed | Acceptance still held |
| --- | --- | --- |
| `590.48.01` | Authorized idle lab smoke: seven cards, two serial passes, 14 valid RUSD readings at 34–36°C; clean helper exits; unchanged observed GPU error/idle state. | Independent temperature accuracy, matched raw signed24.8 values/update markers, workload comparison, full one-shot/HTTP/TUI integration, permission/unbind/recovery and 60-minute soak. |
| `595.99.02` | Separate postboot functional smoke: seven cards, two serial passes, 14 valid idle RUSD readings at32–36°C; pinned source/layout verification. | Independent accuracy/raw comparison, workload qualification, recovery and full acceptance. |
| `615.71.09` | Exact pinned source/layout candidate, complete size3184; third driver-free ABI check. | Its own loaded-driver functional and final-stack workload evidence; no hardware acceptance claimed. |

The idle smoke does not close VAL-THERMAL-001/002. Retain private identity,
configuration, exact artifact hash and operator receipts with the run; this public
procedure omits private host addresses, GPU UUIDs and operator details. Source
compatibility does not establish that an installed package is the loaded driver.
Driver installation, activation and any reboot require separate operational
approval and do not follow from the approved software profile.

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
inconclusive required results keep hardware acceptance and promotion of qualified RTX 5090 support held.

## Exact615 candidate validation definition — 14 October 2026

The owner-approved combined source/host plan now permits experimental exact615.71.09 alongside590.48.01/595.99.02; source pin `61dcc93722ecb418bb5f2e00923f05b4b8051dd1`, complete shared size3184, offsets2328/2336 unchanged. This later authority supersedes only the old exclusion of615, without accepting unknown nearby/suffixed/future versions or changing defaultlegacy/version0.2.7.

For VAL-THERMAL-001, the authorized lab operator and owner assess whether seven admitted final-stack GPUs produce fresh valid memory temperatures with stable BDF/UUID binding and bounded helper lifetime; retain exact loaded driver/kernel/build/command and raw update evidence. Hardware stress and final615/7.0 operational admission remain separate gated work under the combined authority. For VAL-THERMAL-002, an independent maintainer/reviewer checks the three exact source pins,3144/3152/3184 layouts,102-record lock, three matching and six mismatched native driver-free C pairs, Python security regressions and unknown-profile rejection. Native syntax-only C/Rust tests cannot establish ioctl execution, sensor accuracy or final hardware qualification.

Expected evidence: `.traceweaver/rtx5090-615/evidence/` for source RED/GREEN/C/dependency/build receipts and `.traceweaver/rtx5090-615/software-verification.json` for the tested candidate hash; private final-stack operational receipts for real-card validation. Owner: Hannes Zietsman; implementation participation is disclosed and independent Astra source review is required. Missing/failed evidence is HOLD; no source-test pass closes full accuracy/reference/extended-soak or acceptance of qualified RTX 5090 support. The separately authorized versioned software release retains these qualification limits.
