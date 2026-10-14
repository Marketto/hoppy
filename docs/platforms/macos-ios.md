# macOS and iPhone/iPad (planned)

## macOS
- Target: macOS 12 Monterey and later. That is the newest macOS for 2015 MacBook Pro/Air,
  so it covers Intel Macs from 2015 and every Apple silicon Mac (M1 and later).
- Ship one universal binary (x86_64 + arm64, `lipo`), as `release.yml` already does for the CLI.
- Planned UX: Share extension ("Share → Hoppy"), menu-bar app for receiving, drag onto the
  Dock icon. Needs Developer ID signing and notarization (Apple Developer Program).

## iPhone / iPad
- Target: devices from 2021 onwards (e.g. iPhone 13 family, iPad 9th gen, iPad mini 6).
- Wi-Fi Aware (iOS 26) gives direct peer-to-peer without a shared network on iPhone 12 and later.
- Distribution only through TestFlight / App Store (no side-loading from the portal).
- Background receiving is limited: the app must be open, or woken by a push notification.
