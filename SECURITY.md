# Security policy

## Report a vulnerability privately

Use [GitHub private vulnerability reporting](https://github.com/cryptolabsza/dc-exporter-rs-public/security/advisories/new)
for security findings affecting DC Exporter. Include the affected source commit
or artifact SHA-256, exact driver/platform, impact and a minimal reproduction.
Remove credentials, client information and private infrastructure identifiers.
Do not post exploit details or secrets in public issues.

Ordinary defects and unsupported hardware reports belong in the source issue
tracker. A native GPU recovery action or Xid is diagnostic evidence and does not,
by itself, establish an exporter vulnerability or permanent hardware damage.

## Deployment boundary

The HTTP metrics listener binds all interfaces by default. Metrics can expose
host/GPU identity and infrastructure state; restrict access to trusted monitoring
systems through the host's network policy. Review device and service permissions
for the selected thermal backend. RUSD does not require `/dev/mem`; legacy BAR
access has separate privileged-access requirements.

The exporter reports GPU state and performs no automatic GPU reset, reboot or
workload remediation. Native driver interaction and collector freshness still
need operator review; a successful HTTP response is not a hardware health guarantee.

Official artifacts must come from reviewed protected refs through CryptoLabs'
shared runner pool and retain verified source/artifact provenance. Untrusted fork
and PR code must not execute on trusted runners or receive organization secrets.

---

**Publication prerequisite:** GitHub private vulnerability reporting is available
for public repositories. Keep this repository private until the owner explicitly
approves the reviewed visibility cutover. During that cutover, hold Actions and
artifact publication, change visibility, enable private vulnerability reporting,
and verify both its enabled status and the reporting form linked above before
public launch or announcement. If enablement or verification fails, stop the
launch and follow the approved visibility rollback; do not announce a working
reporting channel. No reporting email address has been invented. Maintainers must
also decide and document supported release lines; this policy promises no response
time or historical-version security coverage.
