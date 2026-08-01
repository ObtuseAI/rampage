# Rampage security policy

## Supported release

The source and Windows x64 packages for the current `0.1.x` line receive security fixes. Other
platform builds are development targets until their installers and lifecycle tests are published.

## Report a vulnerability

Do not disclose secrets, working exploits, device identities, signed invitations, or private mesh
addresses in a public issue. Send a private report to the repository owner through GitHub's private
vulnerability reporting channel. Include the affected version, attack preconditions, impact,
reproduction steps, and the smallest safe proof of concept. Maintainers should acknowledge a report
within seven days; no response-time or bounty guarantee is implied.

## Security boundaries

- The HTTP control API must bind to loopback and all non-health routes require the random local
  controller token.
- Remote control traffic uses authenticated QUIC. Signed endpoint records bind the Governor key to
  the controller endpoint and addresses. Pre-enrollment transport identity must equal the invited
  key; enrolled calls must equal the node's recorded key.
- Remote workers can enroll, advertise an offer, claim their own work, and return their own receipt.
  They cannot mint invites, submit jobs, list other nodes, change policy, or operate the kill latch.
- Only compiled, allowlisted adapters execute. No job contract grants an arbitrary shell, path, URL,
  dynamic library, or code-evaluation capability.
- Artifact QUIC is distinct from control traffic and capped at 64 MiB. Every operation requires a
  Governor-signed storage lease binding node, SHA-256 digest, direction, size, storage class, expiry,
  and fencing epoch. Workers accept artifact connections only from their pinned controller identity.
- The Governor, signing keys, lease/fencing rules, ledger verification, stop latch, promotion gates,
  secret handling, and updater are outside AI authority.
- DumbMoney is an external trust domain. Rampage receives exported, read-only telemetry and returns
  proposals through typed contracts; it never opens that product's live database or credentials.

## Expected deployment

Version 0.1 is for devices controlled by one owner or a deliberately trusted circle. Protect the
Windows account and the Rampage application-data directory: possession of an unexpired invite can
enroll one machine, and possession of local key files represents that device. Invitations expire
after ten minutes and are consumed once.

Public relays, anonymous peers, untrusted arbitrary workloads, and multi-tenant isolation are out of
scope. A private relay may improve reachability but does not become an authority service.

The llama.cpp RPC backend is also out of scope for execution in the current release. Rampage does
not launch or expose its listener: upstream documents it as insecure and CVE-2026-34159 currently
has no patched version. Authentication around an unsafe parser is not treated as remediation. See
`docs/BACKEND_GATES.md` for the evidence required before any distributed model engine is admitted.

## Supply-chain practice

Release candidates must pass Rust, JavaScript, and Python dependency audits; source tests; signed
mesh and real workload E2E tests; packaged-sidecar smoke tests; and a post-exit process-leak check.
Warnings that do not affect the release target must still be recorded in release evidence.
