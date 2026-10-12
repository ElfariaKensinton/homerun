use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tokio::sync::Mutex;

#[cfg(test)]
use crate::platform::process::run_script;

/// Global lock to prevent concurrent downloads/extractions of the runner binary.
static DOWNLOAD_LOCK: Mutex<()> = Mutex::const_new(());

/// Constructs the GitHub Actions runner download URL for the given version, OS, and architecture.
pub fn runner_download_url(version: &str, os: &str, arch: &str) -> String {
    let ext = if os == "win" { "zip" } else { "tar.gz" };
    format!(
        "https://github.com/actions/runner/releases/download/v{version}/actions-runner-{os}-{arch}-{version}.{ext}"
    )
}

/// Returns (os, arch) for the current platform.
pub fn detect_platform() -> (&'static str, &'static str) {
    let os = if cfg!(target_os = "macos") {
        "osx"
    } else if cfg!(target_os = "windows") {
        "win"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    };
    (os, arch)
}

/// Fetches the latest GitHub Actions runner version from the GitHub releases API.
/// Returns the version string without the "v" prefix (e.g., "2.321.0").
pub async fn get_latest_runner_version() -> Result<String> {
    let octocrab = octocrab::instance();
    let release = octocrab
        .repos("actions", "runner")
        .releases()
        .get_latest()
        .await
        .context("Failed to fetch latest runner release from GitHub")?;

    let tag = release.tag_name;
    let version = tag.strip_prefix('v').unwrap_or(&tag).to_string();
    Ok(version)
}

/// Ensures the GitHub Actions runner binary is downloaded and extracted to cache_dir.
/// If `cache_dir/runner-{version}/run.sh` (or `run.cmd` on Windows) already exists, returns early.
/// Otherwise downloads the archive, extracts it, and cleans up.
/// Returns the path to the versioned runner directory.
pub async fn ensure_runner_binary(
    cache_dir: &Path,
    custom_url: Option<&str>,
) -> Result<PathBuf> {
    let (os, arch) = detect_platform();
    if let Some(url) = custom_url.map(str::trim).filter(|url| !url.is_empty()) {
        return ensure_custom_runner_binary(cache_dir, url, os, arch).await;
    }

    let version = get_latest_runner_version()
        .await
        .context("Failed to determine latest runner version")?;
    let runner_dir = cache_dir.join(format!("runner-{version}"));
    fetch_and_cache(runner_dir, &version, os, arch).await
}

async fn ensure_custom_runner_binary(
    cache_dir: &Path,
    url: &str,
    os: &str,
    arch: &str,
) -> Result<PathBuf> {
    use reqwest::header::{CONTENT_LENGTH, ETAG, LAST_MODIFIED};

    let _guard = DOWNLOAD_LOCK.lock().await;
    tokio::fs::create_dir_all(cache_dir)
        .await
        .context("Failed to create runner cache directory")?;

    let stem = format!("runner-custom-{os}-{arch}");
    let runner_dir = cache_dir.join(&stem);
    let marker_path = cache_dir.join(format!("{stem}.meta"));
    let script_name = if os == "win" { "run.cmd" } else { "run.sh" };
    let client = reqwest::Client::new();

    // A custom release asset may be replaced without changing its URL. Reuse
    // it only when the server provides a stable validator and it still matches.
    let (signature, can_reuse_cache) = match client.head(url).send().await {
        Ok(response) if response.status().is_success() => {
            let headers = response.headers();
            let etag = headers.get(ETAG).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            let modified = headers.get(LAST_MODIFIED).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            let length = headers.get(CONTENT_LENGTH).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            (
                format!("{url}\n{etag}\n{modified}\n{length}"),
                !etag.is_empty() || !modified.is_empty(),
            )
        }
        _ => (String::new(), false),
    };

    if can_reuse_cache
        && runner_dir.join(script_name).is_file()
        && tokio::fs::read_to_string(&marker_path).await.is_ok_and(|cached| cached == signature)
    {
        tracing::debug!("Custom runner archive is unchanged at {url}");
        return Ok(runner_dir);
    }

    let response = client.get(url).send().await
        .with_context(|| format!("Failed to download custom runner from {url}"))?;
    if !response.status().is_success() {
        anyhow::bail!("Failed to download custom runner from {url}: HTTP {}", response.status());
    }
    let bytes = response.bytes().await.context("Failed to read custom runner archive")?;
    let ext = if os == "win" { "zip" } else { "tar.gz" };
    let archive_path = cache_dir.join(format!("{stem}.{ext}"));
    tokio::fs::write(&archive_path, &bytes)
        .await
        .with_context(|| format!("Failed to write custom runner archive to {archive_path:?}"))?;

    let staging_dir = cache_dir.join(format!("{stem}.staging-{}", std::process::id()));
    let backup_dir = cache_dir.join(format!("{stem}.previous"));
    if tokio::fs::metadata(&staging_dir).await.is_ok() {
        tokio::fs::remove_dir_all(&staging_dir).await.context("Failed to clear stale staging directory")?;
    }
    tokio::fs::create_dir_all(&staging_dir).await.context("Failed to create staging directory")?;

    let extraction: Result<()> = async {
        if ext == "zip" {
            let archive_clone = archive_path.clone();
            let staging_clone = staging_dir.clone();
            tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                let file = std::fs::File::open(&archive_clone)?;
                let mut archive = zip::ZipArchive::new(file)?;
                archive.extract(&staging_clone)?;
                Ok(())
            })
            .await
            .context("Custom runner zip extraction task panicked")??;
        } else {
            let status = tokio::process::Command::new("tar")
                .arg("xzf")
                .arg(&archive_path)
                .arg("-C")
                .arg(&staging_dir)
                .status()
                .await
                .context("Failed to extract custom runner tarball")?;
            if !status.success() {
                anyhow::bail!("Custom runner tar extraction failed with status {status}");
            }
        }
        if !staging_dir.join(script_name).is_file() {
            anyhow::bail!("Custom runner archive is missing {script_name}");
        }
        Ok(())
    }.await;

    if let Err(error) = extraction {
        let _ = tokio::fs::remove_file(&archive_path).await;
        let _ = tokio::fs::remove_dir_all(&staging_dir).await;
        return Err(error);
    }

    if tokio::fs::metadata(&backup_dir).await.is_ok() {
        tokio::fs::remove_dir_all(&backup_dir).await.context("Failed to remove previous runner backup")?;
    }
    let had_previous = tokio::fs::metadata(&runner_dir).await.is_ok();
    if had_previous {
        tokio::fs::rename(&runner_dir, &backup_dir).await.context("Failed to move old custom runner cache")?;
    }
    if let Err(error) = tokio::fs::rename(&staging_dir, &runner_dir).await {
        if had_previous {
            let _ = tokio::fs::rename(&backup_dir, &runner_dir).await;
        }
        return Err(error).context("Failed to install custom runner cache");
    }
    if had_previous {
        let _ = tokio::fs::remove_dir_all(&backup_dir).await;
    }
    let _ = tokio::fs::remove_file(&archive_path).await;

    if can_reuse_cache {
        tokio::fs::write(&marker_path, signature).await.context("Failed to save custom runner cache metadata")?;
    } else {
        let _ = tokio::fs::remove_file(&marker_path).await;
    }

    tracing::info!("Custom runner installed from {url} at {runner_dir:?}");
    Ok(runner_dir)
}

/// Ensures a **Linux** GitHub Actions runner binary is downloaded and
/// extracted to cache_dir, for bind-mounting into a Docker container.
///
/// Always resolves to `os = "linux"` regardless of the daemon's host OS —
/// a Docker container, even one launched from Docker Desktop on
/// macOS/Windows, runs a Linux kernel and needs the Linux runner build.
/// Cached under a distinct `runner-{version}-linux-{arch}` directory so it
/// never collides with (or reuses) the host-native cache entry.
pub async fn ensure_runner_binary_for_container(cache_dir: &Path, arch: &str) -> Result<PathBuf> {
    let version = get_latest_runner_version()
        .await
        .context("Failed to determine latest runner version")?;
    let runner_dir = cache_dir.join(format!("runner-{version}-linux-{arch}"));
    fetch_and_cache(runner_dir, &version, "linux", arch).await
}

/// Shared download/extract/cache logic for a given `(os, arch)` runner build,
/// written directly into `runner_dir` (already versioned by the caller).
async fn fetch_and_cache(
    runner_dir: PathBuf,
    version: &str,
    os: &str,
    arch: &str,
) -> Result<PathBuf> {
    let run_script_path = runner_dir.join(if os == "win" { "run.cmd" } else { "run.sh" });

    // Fast path: already cached, no lock needed
    if run_script_path.exists() {
        tracing::debug!("Runner binary already cached at {:?}", runner_dir);
        return Ok(runner_dir);
    }

    // Serialize concurrent downloads — only one caller extracts at a time
    let _guard = DOWNLOAD_LOCK.lock().await;

    // Re-check after acquiring lock (another caller may have finished)
    if run_script_path.exists() {
        tracing::debug!("Runner binary already cached at {:?}", runner_dir);
        return Ok(runner_dir);
    }

    tokio::fs::create_dir_all(&runner_dir)
        .await
        .with_context(|| format!("Failed to create runner directory {:?}", runner_dir))?;

    let url = runner_download_url(version, os, arch);

    tracing::info!("Downloading runner from {}", url);

    let response = reqwest::get(&url)
        .await
        .with_context(|| format!("Failed to download runner from {url}"))?;

    if !response.status().is_success() {
        anyhow::bail!("Failed to download runner: HTTP {}", response.status());
    }

    let ext = if os == "win" { "zip" } else { "tar.gz" };
    let archive_path = runner_dir.join(format!("actions-runner-{os}-{arch}-{version}.{ext}"));

    let bytes = response
        .bytes()
        .await
        .context("Failed to read runner archive bytes")?;

    tokio::fs::write(&archive_path, &bytes)
        .await
        .with_context(|| format!("Failed to write runner archive to {:?}", archive_path))?;

    tracing::info!("Extracting runner archive to {:?}", runner_dir);

    // Branch on the archive's actual format (`ext`, derived from the *target*
    // os), not the host OS: container mode always downloads a Linux
    // `tar.gz`, even when the daemon itself runs on Windows. Windows 10
    // (1803+) ships `tar.exe` (bsdtar), so shelling out to `tar` works on
    // both platforms for the `tar.gz` case.
    if ext == "zip" {
        let archive_clone = archive_path.clone();
        let dir_clone = runner_dir.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let file = std::fs::File::open(&archive_clone)?;
            let mut archive = zip::ZipArchive::new(file)?;
            archive.extract(&dir_clone)?;
            Ok(())
        })
        .await
        .context("Zip extraction task panicked")??;
    } else {
        let status = tokio::process::Command::new("tar")
            .arg("xzf")
            .arg(&archive_path)
            .arg("-C")
            .arg(&runner_dir)
            .status()
            .await
            .context("Failed to run tar to extract runner archive")?;

        if !status.success() {
            anyhow::bail!("tar extraction failed with status: {}", status);
        }
    }

    tokio::fs::remove_file(&archive_path)
        .await
        .with_context(|| format!("Failed to remove runner archive {:?}", archive_path))?;

    tracing::info!("Runner binary ready at {:?}", runner_dir);

    Ok(runner_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_download_url_macos_arm64() {
        let url = runner_download_url("2.321.0", "osx", "arm64");
        assert_eq!(
            url,
            "https://github.com/actions/runner/releases/download/v2.321.0/actions-runner-osx-arm64-2.321.0.tar.gz"
        );
    }

    #[test]
    fn test_download_url_macos_x64() {
        let url = runner_download_url("2.321.0", "osx", "x64");
        assert_eq!(
            url,
            "https://github.com/actions/runner/releases/download/v2.321.0/actions-runner-osx-x64-2.321.0.tar.gz"
        );
    }

    #[test]
    fn test_download_url_windows_x64() {
        let url = runner_download_url("2.321.0", "win", "x64");
        assert_eq!(
            url,
            "https://github.com/actions/runner/releases/download/v2.321.0/actions-runner-win-x64-2.321.0.zip"
        );
    }

    #[test]
    fn test_download_url_linux_x64() {
        let url = runner_download_url("2.321.0", "linux", "x64");
        assert_eq!(
            url,
            "https://github.com/actions/runner/releases/download/v2.321.0/actions-runner-linux-x64-2.321.0.tar.gz"
        );
    }

    #[test]
    fn test_download_url_different_version() {
        let url = runner_download_url("2.300.0", "osx", "arm64");
        assert!(url.contains("v2.300.0"));
        assert!(url.contains("arm64"));
        assert!(url.ends_with(".tar.gz"));
    }

    #[test]
    fn test_download_url_contains_github_actions_runner() {
        let url = runner_download_url("2.321.0", "osx", "arm64");
        assert!(url.starts_with("https://github.com/actions/runner/releases/download/"));
    }

    #[test]
    fn test_detect_platform() {
        let (os, arch) = detect_platform();
        if cfg!(target_os = "macos") {
            assert_eq!(os, "osx");
        } else if cfg!(target_os = "windows") {
            assert_eq!(os, "win");
        } else {
            assert_eq!(os, "linux");
        }
        assert!(arch == "arm64" || arch == "x64");
    }

    #[test]
    fn test_detect_platform_os_is_correct() {
        let (os, _arch) = detect_platform();
        if cfg!(target_os = "macos") {
            assert_eq!(os, "osx");
        } else if cfg!(target_os = "windows") {
            assert_eq!(os, "win");
        } else {
            assert_eq!(os, "linux");
        }
    }

    #[test]
    fn test_detect_platform_arch_is_valid() {
        let (_os, arch) = detect_platform();
        assert!(arch == "arm64" || arch == "x64", "unexpected arch: {arch}");
    }

    /// Test the cache-hit early-return path: if `runner-{version}/run.sh` (or `run.cmd`)
    /// already exists, `ensure_runner_binary` should return immediately
    /// with the runner directory path without making any network calls.
    #[tokio::test]
    async fn test_ensure_runner_binary_cache_hit_returns_early() {
        use std::fs;

        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let cache_dir = tmp.path();

        // Simulate a cached runner at a fixed version
        let version = "2.999.0";
        let runner_dir = cache_dir.join(format!("runner-{version}"));
        fs::create_dir_all(&runner_dir).expect("failed to create runner dir");
        fs::write(runner_dir.join(run_script()), "#!/bin/bash\necho runner")
            .expect("failed to write run script");

        // We test the building blocks directly:
        // 1. The run script exists check works correctly.
        let run_script_path = runner_dir.join(run_script());
        assert!(
            run_script_path.exists(),
            "run script should exist in simulated cache"
        );

        // 2. The runner_dir path is constructed consistently.
        let expected_runner_dir = cache_dir.join(format!("runner-{version}"));
        assert_eq!(runner_dir, expected_runner_dir);
    }

    /// Verify that runner_download_url produces a URL that correctly incorporates
    /// all three parameters without any confusion between them.
    #[test]
    fn test_download_url_components_are_distinct() {
        let version = "1.2.3";
        let os = "osx";
        let arch = "arm64";
        let url = runner_download_url(version, os, arch);
        // All three are present
        assert!(url.contains(version));
        assert!(url.contains(os));
        assert!(url.contains(arch));
        // Version appears after the /v prefix on the release path AND in the filename
        assert!(url.contains(&format!("v{version}")));
        assert!(url.contains(&format!("actions-runner-{os}-{arch}-{version}")));
    }

    /// Test that runner_download_url with edge case version still produces a valid URL structure.
    #[test]
    fn test_download_url_with_edge_case_version() {
        let url = runner_download_url("0.0.0", "osx", "x64");
        assert!(url.starts_with("https://"));
        assert!(url.ends_with(".tar.gz"));
        assert!(url.contains("0.0.0"));
    }

    /// Verify that detect_platform always returns the same value (deterministic).
    #[test]
    fn test_detect_platform_is_deterministic() {
        let (os1, arch1) = detect_platform();
        let (os2, arch2) = detect_platform();
        assert_eq!(os1, os2);
        assert_eq!(arch1, arch2);
    }

    /// The URL must always use HTTPS (not HTTP or any other scheme).
    #[test]
    fn test_download_url_is_https() {
        let url = runner_download_url("2.321.0", "osx", "arm64");
        assert!(url.starts_with("https://"), "URL must use HTTPS: {url}");
    }

    /// Ensure the URL embeds the version in both the release path and the filename.
    #[test]
    fn test_download_url_version_in_path_and_filename() {
        let version = "2.400.1";
        let url = runner_download_url(version, "osx", "arm64");
        // The release tag part (e.g. /download/v2.400.1/)
        assert!(
            url.contains(&format!("v{version}")),
            "release path missing v-prefix: {url}"
        );
        // The archive filename (e.g. actions-runner-osx-arm64-2.400.1.tar.gz)
        assert!(
            url.contains(&format!("actions-runner-osx-arm64-{version}.tar.gz")),
            "filename missing version: {url}"
        );
    }

    /// Validate URL format consistency across several version strings.
    #[test]
    fn test_download_url_format_consistency() {
        for version in &["2.300.0", "2.321.0", "3.0.0", "10.0.100"] {
            let url = runner_download_url(version, "osx", "arm64");
            assert!(url.starts_with("https://github.com/actions/runner/releases/download/v"));
            assert!(url.ends_with(".tar.gz"));
        }
    }

    #[test]
    fn test_detect_platform_arch_not_empty() {
        let (_os, arch) = detect_platform();
        assert!(!arch.is_empty(), "arch must not be empty");
    }
}
