<!-- TRACEWEAVER: file-role=abi-provenance; req=REQ-THERMAL-012; trace=TRACE-THERMAL-012; ver=VER-THERMAL-010 -->
# Experimental NVIDIA RUSD ABI

The memory-only backend accepts Linux x86-64, NVIDIA vendor `10de`, device
`2b85` (RTX 5090), and driver `590.48.01` only. This is a source-reviewed
experimental profile, **not a hardware-validated support claim**. Other driver
versions, platforms and devices fail closed. Default policy does not enable it.
No sensor executable or new driver control was run on the production host during
development. Real-card sensor identity, permissions, workload noninterference,
unbind behavior, Windows comparison and soak remain pending.

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

| 590.48.01 Linux x86-64 field/type | Bytes |
| --- | ---: |
| `NV00DE_SHARED_DATA` | 3144 |
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

RM mapping length is the exact shared structure size. The OS `mmap` length is
rounded up to the observed power-of-two page size (4,096 bytes on the tested ABI
fixture). Mapping uses `PROT_READ` and RM's read-only access flag. Value loads
remain bounded by the 3,144-byte structure and use aligned volatile reads.
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

The 590 Linux `nvidia_read_card_info` source zeroes CARD_INFO and does not set
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
layout harness is `docs/fixtures/rusd-abi-layout.c`. Its compile
targets `x86_64-unknown-linux-gnu` and asserts matching-header sizes and offsets;
the Rust Linux cross-check compiles the native code without running GPU calls.

To reproduce the C assertions, obtain the pinned NVIDIA source tree and run:

```sh
cc -target x86_64-unknown-linux-gnu -ffreestanding -std=c11 -fsyntax-only \
  -I "$NVIDIA_SRC/src/common/sdk/nvidia/inc" \
  -I "$NVIDIA_SRC/kernel-open/common/inc" \
  -I "$NVIDIA_SRC/src/nvidia/arch/nvalloc/unix/include" \
  docs/fixtures/rusd-abi-layout.c
cargo test --test nvidia_rm_abi --test nvidia_rm_rusd
cargo check --tests --target x86_64-unknown-linux-gnu
```

Passing fixtures establish decoder, identity, cleanup and ABI behavior. They do
not establish that a real 5090 provides this sensor, that permissions suffice,
that driver calls do not disturb workloads, or that Linux/Windows readings
agree. Hardware validation procedures V1/V8/V10 remain open. Stable RTX 5090 support
promotion requires the owner's actual-card acceptance; publishing experimental
source also requires the separate exact-tree disclosure and CI gates.

## MIT permission notice for adapted public definitions

Copyright (c) 1999-2025 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
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
