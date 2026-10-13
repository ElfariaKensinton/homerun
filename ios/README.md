# HomeRun for iOS

A native SwiftUI companion for the HomeRun daemon. It covers runner status and lifecycle, single and batch creation, group scaling/actions, repository discovery, GitHub authentication, metrics, live logs, job steps/history/re-runs, preferences, service controls, Docker status, and update checks.

## Open and build

Open HomeRun.xcodeproj in Xcode 15 or newer. Select the HomeRun scheme and an iOS 17+ simulator or device, then build and run. No third-party Swift packages are required.

## Secure remote access

The existing daemon IPC listener remains the default and is unchanged. To allow the iOS app to reach it, start homerund with both environment variables below. The optional HTTP bridge **only binds to loopback** and rejects requests without a bearer token.

    export HOMERUN_REMOTE_BIND=127.0.0.1:47831
    export HOMERUN_REMOTE_TOKEN="$(openssl rand -hex 32)"
    ./target/release/homerund

Keep the generated token somewhere secure; enter it in the app's connection screen. Configure a trusted HTTPS reverse proxy or private-network tunnel (for example, a Tailscale HTTPS endpoint) to forward traffic to http://127.0.0.1:47831 and preserve the Authorization header. Enter that HTTPS endpoint in HomeRun for iOS. Do **not** bind the bridge to a LAN/WAN address or forward its raw HTTP port to the Internet.

The bridge is disabled unless both environment variables are set. The daemon refuses non-loopback bind addresses, missing/short tokens, or configuring only one variable. Requests to the bridge—including /health—require Authorization: Bearer <token>. The connection token is stored in the iOS Keychain; it is separate from the GitHub token held by HomeRun.

For a daemon managed by launchd or another service manager, configure both environment variables in the service's launch configuration and restart the daemon. Starting the bridge in an interactive shell will not configure an already-running service.

## Feature notes

- The app manages runners on the Mac/Windows host that runs homerund; it does not execute GitHub Actions jobs on the iPhone.
- Remote repository scans, runner operations, metrics, daemon logs, history, and preferences use the daemon API.
- Local workspace scans accept a path on the **host machine**, not a path on the iPhone.
- Starting a daemon that is not running, changing operating-system login settings, and some service-manager operations remain host/OS-level tasks. Service install/uninstall endpoints are exposed for compatibility and may report host-permission or platform errors.
- GitHub Device Flow is preferred; a PAT entry is also available. The PAT is sent directly to the daemon and is not saved by the iOS app.

## Security

The remote token grants access to powerful operations, including stopping/deleting runners and shutting down the daemon. Use a high-entropy token, restrict access at the HTTPS tunnel/proxy, keep the phone protected, and rotate the token if the phone or token is lost. HTTPS is required by the app except for loopback development endpoints.
