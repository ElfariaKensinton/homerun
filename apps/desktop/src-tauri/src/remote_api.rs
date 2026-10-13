use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
};

const DEFAULT_BIND_ADDRESS: &str = "0.0.0.0:47831";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteApiSettings {
    pub enabled: bool,
    pub bind_address: String,
    pub token: String,
}

impl Default for RemoteApiSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_address: DEFAULT_BIND_ADDRESS.to_string(),
            token: String::new(),
        }
    }
}

fn settings_path() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "Could not locate the home directory".to_string())?;
    Ok(home.join(".homerun").join("remote-api.json"))
}

pub fn load() -> Result<RemoteApiSettings, String> {
    match fs::read_to_string(settings_path()?) {
        Ok(contents) => serde_json::from_str(&contents)
            .map_err(|error| format!("Could not parse LAN API settings: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(RemoteApiSettings::default())
        }
        Err(error) => Err(format!("Could not read LAN API settings: {error}")),
    }
}

pub fn save(enabled: bool, token: Option<String>) -> Result<RemoteApiSettings, String> {
    let mut settings = load()?;
    settings.enabled = enabled;
    settings.bind_address = DEFAULT_BIND_ADDRESS.to_string();

    if let Some(token) = token {
        let token = token.trim();
        if !token.is_empty() {
            settings.token = token.to_string();
        }
    }
    if settings.enabled && settings.token.len() < 32 {
        return Err("Generate a bearer token of at least 32 characters before enabling LAN access".into());
    }

    let path = settings_path()?;
    let parent = path.parent().ok_or_else(|| "Could not locate the HomeRun configuration directory".to_string())?;
    fs::create_dir_all(parent).map_err(|error| format!("Could not create HomeRun configuration directory: {error}"))?;

    let bytes = serde_json::to_vec_pretty(&settings)
        .map_err(|error| format!("Could not serialize LAN API settings: {error}"))?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)
        .map_err(|error| format!("Could not write LAN API settings: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("Could not protect LAN API token file: {error}"))?;
    }
    file.write_all(&bytes).map_err(|error| format!("Could not write LAN API settings: {error}"))?;
    file.sync_all().map_err(|error| format!("Could not flush LAN API settings: {error}"))?;
    Ok(settings)
}

pub fn generate_token() -> Result<String, String> {
    #[cfg(unix)]
    {
        let mut bytes = [0_u8; 32];
        let mut random = fs::File::open("/dev/urandom")
            .map_err(|error| format!("Could not access the secure random source: {error}"))?;
        random.read_exact(&mut bytes)
            .map_err(|error| format!("Could not generate a secure token: {error}"))?;
        Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
    }
    #[cfg(not(unix))]
    {
        Err("Secure token generation is currently available on macOS and Unix builds".into())
    }
}
