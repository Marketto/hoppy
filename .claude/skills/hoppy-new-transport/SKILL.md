---
name: hoppy-new-transport
description: Use when planning or adding a transport or platform beyond LAN TCP - Bluetooth LE discovery, Wi-Fi Direct, Wi-Fi Aware (Android/iOS 26), internet relay/P2P, macOS, Linux desktop, iPhone.
---

# New transports and platforms

Read first: `docs/architecture.md` (§ transport selection) and `docs/roadmap.md`.

## Principles

1. **Discovery and transport are separate.** A peer may be *discovered* over BLE and then
   *transfer* over Wi-Fi Direct/Aware or LAN. Model peers with a list of reachable
   endpoints, ranked: LAN > Wi-Fi Aware/Direct > relay > BLE (BLE only for tiny payloads).
2. **Same session protocol on every pipe.** The v0 framing (hello, offer, decision,
   content+hash, result) runs over any reliable byte stream; new transports provide a
   stream, not a new protocol.
3. **Encryption before internet.** No remote/relay transport ships before protocol v1
   (done: Noise XX with TOFU-pinned device keys, ADR 0010; a relay requires plaintext v0 to be switched off first).
4. **Platform limits are product facts**: iOS has no Wi-Fi Direct/BT classic for apps but
   has Wi-Fi Aware (iOS 26, iPhone 12+); background receiving on iOS is limited; without a
   paid account there is no multicast (unicast sweep, ADR 0009). Record
   them in `docs/platforms/<platform>.md`.

## Process

1. Write a "Proposed" ADR (`hoppy-docs-adr`): goal, options, chosen design, test plan.
2. Prototype behind a feature flag; keep LAN path untouched.
3. Extend interop tests or add a device test plan to the platform doc.
4. Update `docs/roadmap.md` and the router in `AGENTS.md` if new files/areas appear.
