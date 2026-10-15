# 0006. Callback (reverse connect) for receivers behind an inbound firewall
Status: Accepted (PoC)
Date: 2026-10-15

## Context
A Windows PC on a managed network was discovered by an Android phone (its multicast
announces go out) but the phone's TCP connect to port 47471 timed out: inbound connections
are blocked by Windows Firewall and the user cannot add rules (no admin, or Group Policy
ignores local rules). Outbound connections from the PC work.

## Decision
Add a `callback` discovery datagram and a `HOPC` callback hello (spec §6, additive to v0).
When a direct connect fails within 3 s, the sender opens an ephemeral listener and asks the
receiver, by UDP from its discovery socket, to connect back; it re-sends every 2 s and
immediately after each announce of the receiver (Windows accepts unicast replies to its
own multicast for ~3 s). The receiver connects only to the datagram's source IP, only on
private/link-local/loopback ranges, with dedupe and rate limits, then the normal session
runs with roles unchanged (sender offers, receiver's user decides). The 5 s announce
interval is kept: it gives ≥ 3 chances in the 20 s window without raising LAN traffic.

## Consequences
Works for the common "PC can't accept inbound" case without admin rights or extra
infrastructure; worst-case latency ≈ one announce interval (5 s). Does not help when both
sides block inbound, across subnets, or when the sender is a send-only node behind a strict
firewall: those need a relay (v1 roadmap). The token is not authentication (ADR 0003 still
applies). All three implementations and the interop test cover it.

Hardening after protocol review (same date, spec §6): while a callback is pending the
sender trusts announces/queries carrying the receiver's id, and callback connections, only
from the receiver's known (local) IP, so another LAN host cannot collect the token just by
claiming the id; the callback hello has one 3 s deadline and a 1 KiB frame cap (no
slow-loris). Receivers cap attempts at 10/min per source IP and 30/min in total, and
record a token as used only after a successful connect-back so a re-send can retry. A
forged UDP source can still make a receiver connect to a local IP:port and write the fixed
`HOPC` hello; accepted as low impact for v0.
