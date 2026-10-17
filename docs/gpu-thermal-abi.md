<!-- TRACEWEAVER: file-role=abi-provenance; req=REQ-THERMAL-012; trace=TRACE-THERMAL-012; ver=VER-THERMAL-010 -->
# Experimental NVIDIA RUSD ABI

The memory-only backend accepts Linux x86-64, NVIDIA vendor `10de`, device
`2b85` (RTX 5090), and exact drivers `590.48.01`, `595.99.02` or `615.71.09`. These are
experimental profiles, **not hardware-validated support claims**. Other versions,
including unknown 6xx drivers, platforms and devices fail closed. Default policy
remains `legacy` and does not enable RUSD.

A separately authorized idle lab smoke on `590.48.01` returned 14 valid readings
across seven cards in two serial passes, 34–36°C, with clean helper exits and
unchanged observed GPU error/idle state. That limited result does not establish
independent accuracy, raw-value equivalence, workload noninterference, full
integration or soak acceptance. Separately authorized `595.99.02` postboot
functional smoke returned 14 valid idle probes in two serial passes, 32–36°C;
this is limited functional evidence. `615.71.09` is a source candidate awaiting
its own loaded-driver evidence. Permissions, controlled unbind/recovery,
Windows comparison and the full acceptance procedure remain pending.

## Public sources

NVIDIA release tag `590.48.01` resolves through annotated tag
`499451151a6819af1fa009d2625ed70ae4060f08` to commit
[`2ccbad25e1af6a6ee6f38cf569f89f8b65d658ab`](https://github.com/NVIDIA/open-gpu-kernel-modules/tree/2ccbad25e1af6a6ee6f38cf569f89f8b65d658ab).
The following definitions are adapted from that revision. All listed NVIDIA
headers carry MIT notices; the full permission text appears below.

| Source path relative to NVIDIA repository | Used definition | SHA-256 |
| --- | --- | --- |
| `src/common/sdk/nvidia/inc/class/cl00de.h` | shared layout, sensor index, sequence rules, thermal mask | `0f2ef94405b6646673ec651e41c268258ea7ec7ae347b3146d3f6e9297162340` |
| `src/common/sdk/nvidia/inc/ctrl/ctrl00de.h` | thermal poll request | `65eeec9d620d36d34b1be3dca2ea73732a3b466a9b5a481f52831cbc22354a35` |
| `src/common/sdk/nvidia/inc/nvos.h` | allocation, control, free, map and unmap records | `e15be0320fbf6c22a948806cdf092528e75aa9a4545afd4f3ce77fd6131dd7b3` |
| `src/common/sdk/nvidia/inc/class/cl0000.h` | client allocation parameters | `2f8c158b1e0d55fb425497054539e9dcb24002ec5e4949dd56b9b2b8e17d0208` |
| `src/common/sdk/nvidia/inc/class/cl0080.h` | device allocation parameters | `8829ae3793aaad80dc8e9b560882d4e900565de7ce7832c0ea0ceffe7fe12b08` |
| `src/common/sdk/nvidia/inc/ctrl/ctrl0000/ctrl0000gpu.h` | UUID/ID lookup and targeted attach/detach | `4d44378454f20394c9c5771a1b7b5c2cd9383b0b762c2827758a4ccdcef50fbf` |
| `kernel-open/common/inc/nv-ioctl.h` | CARD_INFO, PCI fields, minor and REGISTER_FD | `c2aa6dad513c8dd1ae9bd412b649ccf1928c45dccf32adbbce8a3bed28420f48` |
| `kernel-open/common/inc/nv-ioctl-numbers.h` | Linux escape numbers | `7f449a019521f2f858a2d485eed1ee0fa4a8ffc82c9aecd484be5d3f6821f87d` |
| `src/nvidia/arch/nvalloc/unix/include/nv_escape.h` | RM escape numbers | `12850ee7368dbfeba1e2392f9ff7fb44563d88ed12cbf478d0c0f61a03763570` |

NVIDIA release tag `595.99.02` resolves through annotated tag
`19f3b344ccbae68f19bdd1d628f54a08eaf5a712` to commit
[`e394a404108a9ce5bba988c1dddf694dabbd4a43`](https://github.com/NVIDIA/open-gpu-kernel-modules/tree/e394a404108a9ce5bba988c1dddf694dabbd4a43).
The following separately pinned MIT-licensed definitions support the 595
candidate. Definitions are manually adapted into Rust and checked against the
matching public headers; they are not generated bindings or a promise of ABI
stability across a driver family. `595.99.02` is an exact candidate, not a claim
about the latest release.

| Source path relative to NVIDIA repository | 595.99.02 SHA-256 |
| --- | --- |
| `src/common/sdk/nvidia/inc/class/cl00de.h` | `35670d98dc93e7ceb70d58ded910328cf9b275d40ed53b014fcd9cab323dd091` |
| `src/common/sdk/nvidia/inc/ctrl/ctrl00de.h` | `65eeec9d620d36d34b1be3dca2ea73732a3b466a9b5a481f52831cbc22354a35` |
| `src/common/sdk/nvidia/inc/nvos.h` | `5786e4c046f08dd627f13760474599624d578325938fe269cb4321c2e3502c02` |
| `src/common/sdk/nvidia/inc/class/cl0000.h` | `2f8c158b1e0d55fb425497054539e9dcb24002ec5e4949dd56b9b2b8e17d0208` |
| `src/common/sdk/nvidia/inc/class/cl0080.h` | `8829ae3793aaad80dc8e9b560882d4e900565de7ce7832c0ea0ceffe7fe12b08` |
| `src/common/sdk/nvidia/inc/ctrl/ctrl0000/ctrl0000gpu.h` | `68f4a7e9cc28e1ac97709315c89fc5e76954d7abcb96c14c51e740c9cedb897d` |
| `kernel-open/common/inc/nv-ioctl.h` | `c2aa6dad513c8dd1ae9bd412b649ccf1928c45dccf32adbbce8a3bed28420f48` |
| `kernel-open/common/inc/nv-ioctl-numbers.h` | `7a9d2670092496aca00b5109b61e41c90c9f66ace404965a728d692b77191352` |
| `src/nvidia/arch/nvalloc/unix/include/nv_escape.h` | `51da358aac86eba22c9ca9db137cd2b41e36f433b9848f99dbc1ce744f5115e7` |
| `src/common/sdk/nvidia/inc/nvfixedtypes.h` | `4682ac614a8e3c598fb481139de0e5ed1923314f361db9aad0c0749da6dbd5b9` |
| `src/common/sdk/nvidia/inc/class/cl0041.h` | `e7d4baca191689182d9dd3930b98a572f662f1d3c7e5a86248a89d8f2535a3e1` |
| `src/common/sdk/nvidia/inc/class/cl2080.h` | `6f0c32970e6ba7e56dec28c44594b41570237bf74b1ea33ab5e8bcd139805351` |
| `src/nvidia/arch/nvalloc/unix/include/nv-unix-nvos-params-wrappers.h` | `d3ff263781baae28ade28d5fd7a9f69d2019e8f4ca7bfb5675fa280be77792e5` |

NVIDIA exact `615.71.09` is pinned to commit
[`61dcc93722ecb418bb5f2e00923f05b4b8051dd1`](https://github.com/NVIDIA/open-gpu-kernel-modules/tree/61dcc93722ecb418bb5f2e00923f05b4b8051dd1).
The separate complete shared layout is3,184 bytes; common thermal fields and
RM object/identity records remain independently asserted. This exact pin does
not enable other615 or6xx versions. Its native Linux dependency closure contains
34headers/474,626bytes: `nvintrin.h` replaces `alloc/alloc_channel.h` in the595
closure. All102 profile header records total1,420,051bytes, below the unchanged
2MiB bound. The complete path/length/hash record is
[`scripts/ci/rusd-headers.json`](../scripts/ci/rusd-headers.json).

| Source path relative to NVIDIA repository | 615.71.09 SHA-256 |
| --- | --- |
| `src/common/sdk/nvidia/inc/class/cl00de.h` | `e4f11a480ab931a115564f5659f909e524717b1e6850fce0adea750a5e7850e7` |
| `src/common/sdk/nvidia/inc/ctrl/ctrl00de.h` | `65eeec9d620d36d34b1be3dca2ea73732a3b466a9b5a481f52831cbc22354a35` |
| `src/common/sdk/nvidia/inc/nvos.h` | `27adc076fe1a265d41374f7c865a6afb6a2f1025fc4e136d03431a638570f35c` |
| `src/common/sdk/nvidia/inc/ctrl/ctrl0000/ctrl0000gpu.h` | `af306168bc3f575c02b499e66a7631d8881d2cee897c680e15ea54cddf2ad6db` |
| `kernel-open/common/inc/nv-ioctl-numbers.h` | `ac3392867047e528019e573a3ab905096c4158f783cf3d2a2a93f8394b8d883f` |
| `src/common/sdk/nvidia/inc/nvintrin.h` | `6b296650a81b44967c75c8405ae89548f4df5b5cc22ed9ad4573eed1af490320` |

Additional exact-revision headers checked by the C harness are
`nvfixedtypes.h` (`NvTemp = NvSFXP24_8`), `class/cl0041.h` (root client class),
`class/cl2080.h` (subdevice parameters), and
`nv-unix-nvos-params-wrappers.h` (map-with-FD wrapper).

The public sequence was also inspected in Philip Langdale's
[`nvidia-gpu-sensors.c` at `2ea3c98754a50e73491aae618ba11526dc0b6029`](https://github.com/philipl/nvidia-gpu-sensors/blob/2ea3c98754a50e73491aae618ba11526dc0b6029/nvidia-gpu-sensors.c),
SHA-256 `382d38b1dc891dd5c41256742443497186f2332c06eff0ed05486c017ce452c0`.
Copyright (c) 2026 Philip Langdale, MIT. The Rust implementation follows the
public RM object/control sequence but excludes the upstream voltage setup,
register scanning, attach-all behavior and hard-coded 3,152-byte layout.
No historical confidential register information is included in this RUSD backend.
The unchanged legacy BAR implementation has a separate owner disclosure/provenance
review before public visibility.

## Layout and semantics

| Linux x86-64 field/type | Bytes (590.48.01 / 595.99.02 / 615.71.09) |
| --- | ---: |
| `NV00DE_SHARED_DATA` | 3144 / 3152 / 3184 |
| `temperatures` base | 2312 |
| temperature entry stride | 16 |
| memory sensor index | 1 |
| memory update marker offset | 2328 |
| memory signed value offset | 2336 |
| `NVOS21` / `NVOS54` / `NVOS00` | 32 / 32 / 16 |
| `NVOS33` / map-with-FD wrapper / `NVOS34` | 48 / 56 / 32 |
| client / device allocation parameters | 120 / 56 |
| CARD_INFO record / PCI record | 72 / 12 |
| attach IDs / UUID lookup parameters | 132 / 272 |

The common thermal offsets, units, sequence rules and controls are unchanged.
595 adds two 32-bit memory row-remap counters after the temperature array; the
complete shared layout therefore grows by eight bytes.615 adds the32-byte
`RUSD_MEM_BANK_REMAP` structure after those counters. RM mapping length is the
exact selected profile's shared structure size. The OS `mmap` length is
rounded up to the observed power-of-two page size (4,096 bytes on the tested ABI
fixture). Mapping uses `PROT_READ` and RM's read-only access flag. Value loads
remain bounded by the selected 3,144-,3,152- or3,184-byte structure and use aligned
volatile reads.
The ioctl request encoding is Linux x86 `_IOC(READ|WRITE, 'F', escape, size)`,
independent of the build host's libc encoding.

The memory entry is signed 24.8 Celsius, decoded by division by 256. Fixtures
cover zero and fractions. Negative values, overflow/sentinels and values above
the experimental 150°C profile bound are unavailable, never clamped.

The reader makes at most ten attempts per snapshot. Marker zero and all-ones
are invalid. At or above `0xff00000000000000`, odd sequence markers indicate
an in-progress update; below that boundary, odd timestamps are permitted by
NVIDIA's rules. Compiler and CPU load fences surround the temperature load,
and the marker must match before and after. Exhaustion never returns a last
plausible value.

Driver marker clock units are not assumed interchangeable with host time.
Each new per-batch mapping first reports stale; it becomes current only after
observing a strictly advancing marker using local monotonic time. Regressing
markers are invalid. Identical markers expire after `max_sample_age`, and a
completed reading is checked again before batch publication so waiting on
other GPUs cannot return an expired value. This policy may withhold a first
sample when the driver polling interval exceeds the configured deadline.

## Identity and control boundary

Requests are limited to 32 and require unique normalized BDFs and UUIDs.
Normalization keeps PCI function and domain; malformed addresses cannot become
filesystem paths. Before opening a targeted device, the child verifies current
sysfs NVIDIA binding, vendor/device, installed proc driver version, and exact
proc BDF/UUID/minor. It cross-checks CARD_INFO and UUID-to-RM-ID lookup with
ID_INFO_V2. RM device/subdevice instances select objects; the separately
verified driver minor selects `/dev/nvidiaN`.

Both pinned Linux `nvidia_read_card_info` sources zero CARD_INFO and do not set
its PCI function field. Consequently CARD_INFO alone is insufficient evidence:
the request must also match the complete sysfs/proc BDF. This profile rejects a
nonzero function when CARD_INFO cannot corroborate it. Binding rechecks use
`readlink` of the driver symlink, never `driver_override`. A concurrent unbind
still races these observations; parent process isolation and per-batch cleanup
are required, and real unbind/workload behavior remains unvalidated.

The allowed native transcript is:

1. Open control FD; allocate root client (`0041`).
2. CARD_INFO (`200`), UUID_INFO (`0274`) and ID_INFO_V2 (`0205`), then attach
   exactly the verified GPU ID (`0215`), with remaining IDs set to invalid.
3. Open its verified device minor and REGISTER_FD (`201`); allocate device
   (`0080`), subdevice (`2080`) and shared data (`00de`).
4. Open mapping FD, RM_MAP_MEMORY (`4e`), read-only `mmap`, then request
   `NV00DE_CTRL_CMD_REQUEST_DATA_POLL` (`00de0001`) with mask **`0x10` only**.
5. Read bounded memory snapshots. Multiple requested sessions wait for update
   progress in one round-robin loop.
6. `munmap`, RM_UNMAP_MEMORY (`4f`), mapping FD close, shared/subdevice/device
   free (`29`), device FD close, targeted detach (`0216`), client free and
   control FD close.

Successful acquisitions alone enter the cleanup ledger. Initialization errors,
sample errors and callback unwinding use the same generic `Session<T>` Drop
path tested by the mock transport and used by the native Linux transport.
Cleanup continues even when a release reports failure; process exit remains
the final owner of kernel resources. Sessions never persist across batches.
No `/dev/mem`, register operations, voltage controls, power writes, reset,
unbind, attach-all or polling-interval changes are exposed by this backend.

The child timeout bounds polling, not blocked ioctl initialization/cleanup.
Only the parent's process deadline can bound collector latency; a helper stuck
in a kernel call can remain alive, and the parent must not launch replacements
while it remains alive.

## Software verification and remaining claims

Fixtures are `tests/nvidia_rm_abi.rs` and `tests/nvidia_rm_rusd.rs`.
Private RED/GREEN logs are retained separately. The public freestanding C
layout harness is `docs/fixtures/rusd-abi-layout.c`. Select `RUSD_ABI_PROFILE=590`
(default), `595` or `615` with the matching pinned source tree. Its compile
targets `x86_64-unknown-linux-gnu` and asserts matching-header sizes and offsets;
the Rust Linux cross-check compiles the native code without running GPU calls.

To reproduce the C assertions, obtain each pinned NVIDIA source tree, set
`NVIDIA_SRC` to that tree and `RUSD_ABI_PROFILE` to its matching `590`, `595` or `615`,
then run the command for each tree:

```sh
clang -target x86_64-unknown-linux-gnu -ffreestanding -std=c11 -fsyntax-only \
  -DRUSD_ABI_PROFILE="$RUSD_ABI_PROFILE" \
  -I "$NVIDIA_SRC/src/common/sdk/nvidia/inc" \
  -I "$NVIDIA_SRC/kernel-open/common/inc" \
  -I "$NVIDIA_SRC/src/nvidia/arch/nvalloc/unix/include" \
  docs/fixtures/rusd-abi-layout.c
cargo test --test nvidia_rm_abi --test nvidia_rm_rusd
cargo check --tests --target x86_64-unknown-linux-gnu
```

The CI command `python3 scripts/ci/rusd_abi.py --cache "$work/rusd-abi"`
verifies all three matching source/fixture pairs, all six off-diagonal size
mismatches, the default590 fixture and an unsupported999 fixture. It also proves
native Linux x86-64/64-bit pointers before syntax-only C compilation; no driver,
GPU, linker or executable is used. Downloads retain256KiB/file,2MiB total and
120s overall limits, exact immutable HTTPS pins/hash checks, no redirects/proxy
auth/keylogging, owned no-symlink cache/atomic writes and bounded sanitized
compiler execution. Unknown runtime profiles are never generated from this check.

Passing fixtures establish decoder, identity, cleanup and ABI behavior. They do
not establish full real-card sensor accuracy, that permissions suffice,
that driver calls do not disturb workloads, or that Linux/Windows readings
agree. Hardware validation procedures V1/V8/V10 remain open. Stable RTX 5090 support
promotion requires the owner's actual-card acceptance; publishing experimental
source also requires the separate exact-tree disclosure and CI gates.

## MIT permission notice for adapted public definitions

Copyright (c) 1999-2025 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
Copyright (c) 1993-2022 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
Copyright (c) 1993-2025 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
Copyright (c) 2001-2005, NVIDIA CORPORATION. All rights reserved.
Copyright (c) 2001-2022 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
Copyright (c) 2001-2024 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
Copyright (c) 2002-2022 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
Copyright (c) 2005-2025 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
Copyright (c) 2017 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
Copyright (c) 2020-2025 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
Copyright (c) 2022-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
Copyright (c) 2023 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
Copyright (c) 2026 Philip Langdale.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
