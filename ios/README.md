# HomeRun for iOS

**Minimum OS:** iOS 26 or later.

HomeRun for iOS controls runners managed by the existing HomeRun desktop app.

## Connect

In **HomeRun → Settings → iPhone / LAN API** on the Mac, enable the listener, generate and save a bearer token, then restart the daemon from the **Daemon** page. Keep the iPhone connected to the same Tailscale tailnet.

The endpoint field accepts bare hostnames, hostname:port, HTTP URLs, and HTTPS URLs:

- `macbook` becomes `http://macbook:47831`.
- `macbook:47831` becomes `http://macbook:47831`.
- Explicit `http://` and `https://` URLs are accepted as entered.

Replace `macbook` with the actual MagicDNS hostname shown by Tailscale. Enter the bearer token generated in HomeRun desktop settings.

## TLS and network warning

The app explicitly accepts every HTTPS server certificate, including self-signed, expired, and hostname-mismatched certificates. Normal TLS certificate and hostname verification is disabled for this app's API session.

Plain HTTP is also accepted. HTTP provides no encryption; because certificate checks are disabled, HTTPS cannot reliably authenticate the server either. The bearer token grants powerful permissions, including runner management and daemon shutdown. Use only on a private, trusted LAN or tailnet, keep the token private, and never forward TCP 47831 to the public Internet.
