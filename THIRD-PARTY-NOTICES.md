# Source and adapted third-party notices

These are the project and adaptation notices retained in the reviewed source.
Binary distributions append the exact locked Linux dependency notices generated
from actual licence files on the authorized shared build runner.

## CryptoLabs project license

MIT License

Copyright (c) CryptoLabs ZA

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

## Historical project and register research credits

DC Exporter began as a Rust refactor of the original CryptoLabs C exporter.
The original project documentation also credits
[olealgoritme/gddr6](https://github.com/olealgoritme/gddr6) for register-offset
research. This research acknowledgment does not establish that upstream code
was copied or that a licence grant applies to it.

## Adapted NVIDIA and Philip Langdale definitions

The RUSD ABI definitions and driver-control sequence use public sources pinned in
`docs/gpu-thermal-abi.md` and `scripts/ci/rusd-headers.json`:

- NVIDIA `open-gpu-kernel-modules`: exact commits
  `2ccbad25e1af6a6ee6f38cf569f89f8b65d658ab` (590.48.01),
  `e394a404108a9ce5bba988c1dddf694dabbd4a43` (595.99.02), and
  `61dcc93722ecb418bb5f2e00923f05b4b8051dd1` (615.71.09).
- Philip Langdale `nvidia-gpu-sensors`: exact commit
  `2ea3c98754a50e73491aae618ba11526dc0b6029`.

The Rust RUSD implementation excludes upstream voltage initialization, register
scanning and attach-all behavior. Header downloads used for layout verification
are pinned by path, length and SHA-256; they are not automatically enabled runtime
profiles. Preserve the following existing adaptation notice:

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

## Binary dependency notices

Each official build resolves `Cargo.lock` for Linux x86-64 with default features,
walks the non-dev dependency closure and retains the actual licence/attribution
files. Missing or unresolved selected licensing fails the build. The generated
`THIRD-PARTY-NOTICES.md` is distributed beside the executable and copied exactly to
`/usr/share/doc/dc-exporter-rs/copyright` in both DEBs. Source notices alone are not
the complete dependency inventory of a binary. The separate build evidence records
the target, command, resolved graph, selected package identities and file hashes.

## Pending legacy provenance clearance

The historical legacy BAR implementation has a separate owner provenance and
disclosure review. The public RUSD sources above do not establish ownership or
redistribution clearance of every earlier contribution. Preserve the legacy code;
record clearance of the exact proposed public bytes and add any applicable
upstream notices before public visibility. No ownership or disclosure clearance is asserted here.
