# 0003. Protocol v0 is unencrypted and LAN-only
Status: Accepted (temporary)
Date: 2026-10-14

## Context
Encryption with device identity, pairing and key pinning is the biggest single piece of
work and would delay the first end-to-end test.

## Decision
v0 has no encryption or authentication. Mitigations: LAN only, explicit accept by default,
sanitized names, hash-verified files, auto-accept behind a warning.

## Consequences
No remote/relay transport and no public release before v1 encryption. The README states
"trusted networks only".
