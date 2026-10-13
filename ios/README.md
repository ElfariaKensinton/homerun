# HomeRun for iOS

A native SwiftUI companion for the HomeRun daemon. It covers runner status and lifecycle, single and batch creation, group scaling/actions, repository discovery, GitHub authentication, metrics, live logs, job steps/history/re-runs, preferences, service controls, Docker status, and update checks.

## Open and build

Open HomeRun.xcodeproj in Xcode 15 or newer. Select the HomeRun scheme and an iOS 17+ simulator or device, then build and run. No third-party Swift packages are required.

## Connect to the Mac over your LAN

The existing HomeRun **desktop GUI** starts the bundled `homerund` daemon and configures its authenticated HTTP bridge on `0.0.0.0:47831`. No separate host app or terminal command is required.

1. Open HomeRun on the Apple Silicon Mac.
2. Open **Settings → LAN Remote Access** and copy the LAN URL and bearer token.
3. Keep the iPhone on the same trusted Wi-Fi/LAN.
4. In HomeRun for iOS, enter the LAN URL (for example, `http://192.168.1.50:47831`) and token.

The bridge requires `Authorization: Bearer <token>` for every endpoint, including `/health`. The token is stored in `~/.homerun/remote-api-token` and should be treated as a password. If the daemon was already running before upgrading, restart it once from the Network Access section to apply the listener settings.

**LAN HTTP is not encrypted.** Use it only on a network you trust. Do not forward port 47831 to the public Internet. For access outside the LAN, use a VPN or an HTTPS tunnel and connect through its HTTPS endpoint instead.

## Feature notes

- The app manages runners on the Mac/Windows host that runs homerund; it does not execute GitHub Actions jobs on the iPhone.
- Remote repository scans, runner operations, metrics, daemon logs, history, and preferences use the daemon API.
- Local workspace scans accept a path on the **host machine**, not a path on the iPhone.
- Starting a daemon that is not running, changing operating-system login settings, and some service-manager operations remain host/OS-level tasks. Service install/uninstall endpoints are exposed for compatibility and may report host-permission or platform errors.
- GitHub Device Flow is preferred; a PAT entry is also available. The PAT is sent directly to the daemon and is not saved by the iOS app.

## Security

The remote token grants access to powerful operations, including stopping/deleting runners and shutting down the daemon. Use a high-entropy token, restrict access at the HTTPS tunnel/proxy, keep the phone protected, and rotate the token if the phone or token is lost. HTTPS is required for public/remote endpoints; plain HTTP is accepted only for loopback or private-LAN hosts.
