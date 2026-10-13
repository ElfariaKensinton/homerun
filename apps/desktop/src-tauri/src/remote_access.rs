use serde::Serialize;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    net::UdpSocket,
    path::PathBuf,
};
use uuid::Uuid;

pub const BIND_ADDRESS: &str = "0.0.0.0:47831";
pub const PORT: u16 = 47831;

#[derive(Debug, Serialize)]
pub struct RemoteAccessInfo {
    pub bind_address: String,
    pub port: u16,
    pub token: String,
    pub lan_url: Option<String>,
}

/// Persist a high-entropy API token in ~/.homerun, readable only by this user on Unix.
pub fn ensure_token() -> Result<String, String> {
    let data_dir = dirs::home_dir()
        .ok_or_else(|| "Could not locate the HomeRun data directory".to_string())?
        .join(".homerun");
    fs::create_dir_all(&data_dir)
        .map_err(|error| format!("Could not create HomeRun data directory: {error}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&data_dir, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("Could not secure HomeRun data directory: {error}"))?;
    }

    let path: PathBuf = data_dir.join("remote-api-token");
    if let Ok(saved) = fs::read_to_string(&path) {
        let saved = saved.trim().to_string();
        if saved.len() == 64 && saved.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Ok(saved);
        }
        let _ = fs::remove_file(&path);
    }

    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    match options.open(&path) {
        Ok(mut file) => {
            file.write_all(token.as_bytes())
                .map_err(|error| format!("Could not persist LAN API token: {error}"))?;
            file.sync_all()
                .map_err(|error| format!("Could not flush LAN API token: {error}"))?;
            Ok(token)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let saved = fs::read_to_string(&path)
                .map_err(|read_error| format!("Could not read LAN API token: {read_error}"))?;
            let saved = saved.trim().to_string();
            if saved.len() == 64 && saved.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                Ok(saved)
            } else {
                Err("The saved LAN API token is invalid; remove ~/.homerun/remote-api-token and restart HomeRun".to_string())
            }
        }
        Err(error) => Err(format!("Could not create LAN API token: {error}")),
    }
}

fn preferred_lan_ip() -> Option<std::net::IpAddr> {
    // UDP connect selects the active route without sending application data.
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:80").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

pub fn info() -> Result<RemoteAccessInfo, String> {
    let token = ensure_token()?;
    let lan_url = preferred_lan_ip().map(|ip| format!("http://{ip}:{PORT}"));
    Ok(RemoteAccessInfo {
        bind_address: BIND_ADDRESS.to_string(),
        port: PORT,
        token,
        lan_url,
    })
}
