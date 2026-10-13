# HomeRun for iOS

HomeRun for iOS is a native SwiftUI companion for controlling runners managed by the existing HomeRun desktop app. It does not run GitHub Actions jobs on the iPhone.

## Connect directly over Tailscale or HTTP

The connection screen accepts both `http://` and `https://` endpoints, including Tailscale MagicDNS names.

For direct connection over Tailscale, enable **HomeRun → Settings → iPhone / LAN API** on the Mac, generate and save a bearer token, then restart the daemon from the **Daemon** page. On the iPhone (also connected to the same tailnet), enter the Mac's MagicDNS name and daemon port, for example:

```text
http://macname:47831
```

Replace `macname` with the actual MagicDNS hostname shown by Tailscale. You can also use the Mac's Tailscale IP, such as `http://100.x.y.z:47831`. Enter the generated token in the app.

For encrypted access, use the HTTPS hostname reported by Tailscale Serve, provided Serve is configured to forward to the daemon listener. HTTPS certificate validation remains enabled; the app does not bypass TLS certificate checks.

## Security warning

HTTP is unencrypted. The bearer token and all API traffic can be observed by a device able to intercept that network path. Use direct HTTP only over a trusted private LAN or tailnet; never forward TCP 47831 to the public Internet. For access across untrusted networks, configure HTTPS through Tailscale Serve or a trusted reverse proxy and use its HTTPS URL.

The API requires `Authorization: Bearer <token>` for every endpoint, including health checks. The token grants powerful permissions, including runner management and daemon shutdown. Keep it private and regenerate it if it may have been exposed.

## Feature notes

- Runner operations, repository discovery, logs, workflow history, metrics, and preferences run on the Mac host.
- Local workspace scans accept a path on the Mac, not a path on the iPhone.
- Some operating-system service actions remain host-side operations.
- GitHub Device Flow is preferred; if you use a PAT, it is sent to the daemon and is not saved by the iOS app.
