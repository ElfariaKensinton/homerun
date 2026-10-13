# HomeRun for iOS

HomeRun for iOS is a native SwiftUI companion for controlling runners managed by the existing HomeRun desktop app. It does not run GitHub Actions jobs on the iPhone.

## Connect over your LAN

1. On the Mac, open the existing **HomeRun** desktop app and go to **Settings → iPhone / LAN API**.
2. Enable **LAN API**, select **Generate token**, and save the settings.
3. Restart the daemon from the desktop app's **Daemon** page so it reads the new configuration.
4. Find your Mac's Wi-Fi/Ethernet IP address in macOS Network settings (for example, `192.168.1.25`).
5. In HomeRun for iOS, set the endpoint to `http://192.168.1.25:47831` using your Mac's actual IP, then enter the generated bearer token.

The listener binds to `0.0.0.0:47831`, so devices that can reach the Mac over the network can connect. The desktop app writes the configuration to `~/.homerun/remote-api.json`; on macOS the file is restricted to the account owner.

## Network security

The API requires `Authorization: Bearer <token>` for every endpoint, including health checks. The token grants powerful permissions, including runner management and daemon shutdown. Keep it private and regenerate it if it may have been exposed.

The LAN endpoint uses plain HTTP: neither the bearer token nor API traffic is encrypted in transit. Use it only on a trusted private network, keep macOS firewall rules restrictive, and **do not expose or port-forward TCP 47831 to the public Internet**. For access when the phone is away from the LAN, use a trusted HTTPS tunnel or reverse proxy and connect through its HTTPS URL instead.

## Feature notes

- Runner operations, repository discovery, logs, workflow history, metrics, and preferences run on the Mac host.
- Local workspace scans accept a path on the Mac, not a path on the iPhone.
- Some operating-system service actions remain host-side operations.
- GitHub Device Flow is preferred; if you use a PAT, it is sent to the daemon and is not saved by the iOS app.
