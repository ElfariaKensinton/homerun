/// Resolve the full PATH from the user's login shell.
/// This picks up paths added by nvm, fnm, Homebrew, etc. that aren't
/// available in a bare launchd environment.
#[cfg(unix)]
pub fn resolve_shell_path() -> Option<String> {
    use std::process::Stdio;

    // Fall back to /bin/sh (guaranteed on any Unix) rather than a specific
    // shell like zsh — zsh is the macOS default but is absent on minimal Linux
    // (e.g. a container), where the fallback would otherwise fail to spawn.
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let output = std::process::Command::new(&shell)
        .args(["-l", "-c", "echo $PATH"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}

/// On Windows the system PATH is usually inherited from the environment,
/// but the daemon may start before the full PATH is available (e.g. after
/// a reboot or when launched from the desktop app).  We dynamically discover
/// tool locations and ensure essential directories are on the PATH so that
/// GitHub Actions runners can find bash, git, pwsh, etc.
#[cfg(windows)]
pub fn resolve_shell_path() -> Option<String> {
    use std::path::PathBuf;

    let current = std::env::var("PATH").unwrap_or_default();
    let current_lower: Vec<String> = current.split(';').map(|p| p.to_lowercase()).collect();

    let mut essential_dirs: Vec<PathBuf> = Vec::new();

    // Discover Git install location from the registry, then derive
    // the subdirectories that runners need (bin, usr\bin, cmd).
    if let Some(git_root) = find_git_install_dir() {
        for subdir in &["bin", "usr\\bin", "cmd"] {
            essential_dirs.push(git_root.join(subdir));
        }
    }

    // PowerShell directories (system-known locations).
    essential_dirs.push(PathBuf::from(r"C:\Windows\System32\WindowsPowerShell\v1.0"));
    // PowerShell 7 can be installed anywhere, but the default is:
    if let Ok(pf) = std::env::var("ProgramFiles") {
        essential_dirs.push(PathBuf::from(pf).join("PowerShell\\7"));
    }

    let mut path = current;
    for dir in &essential_dirs {
        let dir_str = dir.to_string_lossy();
        if !current_lower.iter().any(|p| p == &dir_str.to_lowercase()) && dir.is_dir() {
            path = format!("{path};{dir_str}");
        }
    }

    // Always return Some so the runner process gets an explicit PATH,
    // even when no additions were needed — this prevents the runner from
    // inheriting a stripped-down environment.
    Some(path)
}

/// Find the Git installation directory on Windows by checking the registry
/// and falling back to well-known locations.
#[cfg(windows)]
fn find_git_install_dir() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    // Try the Windows registry (works from native Windows processes).
    let output = Command::new("reg")
        .args(["query", r"HKLM\SOFTWARE\GitForWindows", "/v", "InstallPath"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        // Output line looks like: "    InstallPath    REG_SZ    C:\Program Files\Git"
        if let Some(pos) = line.find("REG_SZ") {
            let path = line[pos + "REG_SZ".len()..].trim();
            if !path.is_empty() {
                let p = PathBuf::from(path);
                if p.is_dir() {
                    return Some(p);
                }
            }
        }
    }

    // Fallback: check well-known locations
    for candidate in &[
        std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".to_string()) + r"\Git",
        r"C:\Program Files\Git".to_string(),
        r"C:\Program Files (x86)\Git".to_string(),
    ] {
        let p = PathBuf::from(candidate);
        if p.is_dir() {
            return Some(p);
        }
    }

    None
}

/// Environment variable read by the patched runner to redirect artifact uploads
/// to a self-hosted Results API.
pub const ARTIFACT_RESULTS_URL_ENV: &str = "ACTIONS_ARTIFACTS_RESULTS_URL_OVERRIDE";

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// Read one variable from the user's login shell. A daemon started by launchd
/// or by the desktop app does not inherit variables exported in shell startup
/// files, so the process environment alone is not enough.
#[cfg(unix)]
pub fn resolve_shell_env_var(name: &str) -> Option<String> {
    use std::process::Stdio;

    const BEGIN: &str = "__HOMERUN_ENV_BEGIN__";
    const END: &str = "__HOMERUN_ENV_END__";

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let script = format!("printf '%s' \"{BEGIN}${{{name}}}{END}\"");

    // Login shell first (.zprofile/.profile); then login + interactive so
    // exports placed in .zshrc/.bashrc are picked up as well.
    for flags in [&["-l", "-c"][..], &["-l", "-i", "-c"][..]] {
        let output = std::process::Command::new(&shell)
            .args(flags)
            .arg(&script)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok();
        let Some(output) = output else { continue };
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Shell startup files may print noise; only trust the marked section.
        if let Some(start) = stdout.rfind(BEGIN) {
            let rest = &stdout[start + BEGIN.len()..];
            if let Some(end) = rest.find(END) {
                if let Some(value) = non_empty(Some(&rest[..end])) {
                    return Some(value);
                }
            }
        }
    }
    None
}

#[cfg(windows)]
pub fn resolve_shell_env_var(_name: &str) -> Option<String> {
    None
}

/// Cached artifact server URL exported in the user's shell, resolved once.
pub static SHELL_ARTIFACT_RESULTS_URL: std::sync::LazyLock<Option<String>> =
    std::sync::LazyLock::new(|| {
        let value = resolve_shell_env_var(ARTIFACT_RESULTS_URL_ENV);
        if let Some(ref v) = value {
            tracing::info!("Resolved {ARTIFACT_RESULTS_URL_ENV} from shell: {v}");
        }
        value
    });

fn pick_artifact_results_url(
    preference: Option<&str>,
    process_env: Option<&str>,
    shell_env: Option<&str>,
) -> Option<String> {
    non_empty(preference)
        .or_else(|| non_empty(process_env))
        .or_else(|| non_empty(shell_env))
}

/// Artifact server URL for runners. Precedence: the app preference, then the
/// daemon's own environment, then the user's login shell environment.
pub fn resolve_artifact_results_url(preference: Option<&str>) -> Option<String> {
    let process_env = std::env::var(ARTIFACT_RESULTS_URL_ENV).ok();
    pick_artifact_results_url(
        preference,
        process_env.as_deref(),
        SHELL_ARTIFACT_RESULTS_URL.as_deref(),
    )
}

/// Cached shell PATH resolved once at first use.
pub static SHELL_PATH: std::sync::LazyLock<Option<String>> = std::sync::LazyLock::new(|| {
    let path = resolve_shell_path();
    if let Some(ref p) = path {
        tracing::info!("Resolved shell PATH: {p}");
    }
    path
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_path_lazy_lock_is_accessible() {
        // Just verify the LazyLock can be dereferenced without panic.
        let _val: &Option<String> = &SHELL_PATH;
    }

    #[test]
    fn artifact_results_url_precedence_is_preference_then_process_then_shell() {
        assert_eq!(
            pick_artifact_results_url(Some(" http://a/ "), Some("http://b/"), Some("http://c/")),
            Some("http://a/".to_string())
        );
        assert_eq!(
            pick_artifact_results_url(Some("  "), Some("http://b/"), Some("http://c/")),
            Some("http://b/".to_string())
        );
        assert_eq!(
            pick_artifact_results_url(None, None, Some("http://c/")),
            Some("http://c/".to_string())
        );
        assert_eq!(pick_artifact_results_url(None, Some(""), None), None);
    }

    #[cfg(windows)]
    #[test]
    fn resolve_shell_path_always_returns_some_on_windows() {
        // On Windows, resolve_shell_path always returns Some so that the
        // runner process gets an explicit PATH.
        let result = resolve_shell_path();
        assert!(result.is_some(), "expected Some on Windows, got None");
        let path = result.unwrap();
        assert!(!path.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn resolve_shell_path_returns_some_on_unix() {
        // On a typical Unix system with a valid $SHELL, we expect Some.
        // If SHELL is unset the function falls back to /bin/sh, which exists on
        // any Unix (macOS, Linux dev machines, and minimal containers alike).
        let result = resolve_shell_path();
        assert!(result.is_some(), "expected Some on Unix, got None");
        let path = result.unwrap();
        assert!(!path.is_empty());
    }
}
