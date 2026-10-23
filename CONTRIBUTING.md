# Contributing to CryptoLabs DC Exporter

Open an issue describing the observed behavior, expected result and the smallest
reproduction. For GPU-specific reports, include the GPU model, exact NVIDIA driver,
backend, exporter commit/artifact hash and sanitized metrics. Remove credentials,
client data, private hostnames/addresses and identifying workload details.

## Changes

1. Start from `dev` and describe the intended change before implementation.
2. Write a focused failing test that reproduces the issue or new behavior.
3. Implement the smallest change that makes it pass, then refactor with tests green.
4. Run the relevant hardware-free checks in [TESTING.md](TESTING.md).
5. Open a pull request targeting `dev` with the behavior change, test evidence and
   remaining limitations. Keep the development version unchanged.

Version bumps occur only with an accepted merge from `dev` to the release branch
`main`. Preserve CryptoLabs branding and applicable third-party notices.

## Hardware and permissions

Software tests do not establish hardware compatibility. New RUSD profiles require
exact pinned public definitions, layout checks, unknown-version rejection and
separately authorized real-card evidence. Do not generalize one exact driver result
to an entire driver family. Keep unavailable telemetry explicit and retain verified
GPU identity when reporting failures.

Do not run probes, resets, reboots or stress tests on someone else's GPU system.
Changes that affect native controls, permissions, GPU state or workloads require a
reviewed lab procedure and the operator's authorization. Driver recovery codes do
not independently establish physical damage.

## Trusted builds

Official builds use CryptoLabs' shared runner pool with reviewed protected refs.
Public forks, unreviewed PR heads and contributor-supplied workflow changes must
not execute on that trusted pool or receive organization secrets. Maintainers
review a contribution before admitting its resulting protected commit to an
official build. A runner label or workflow condition alone is not access control.

Never add private keys, tokens, `.env` files, internal fleet inventories or raw
client logs to a contribution. Use placeholders in examples. Report security
vulnerabilities through [SECURITY.md](SECURITY.md), not a public issue.

## License

Contributions must be compatible with the project's MIT license. Identify copied
or adapted material, retain its copyright/license notices and provide immutable
source references. Do not submit material that you cannot authorize for public
redistribution. Dependencies retain their own licenses.
