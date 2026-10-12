use std::{
    collections::HashSet,
    path::Path,
    time::Duration,
};

use serde::{Deserialize, Serialize};

use crate::{github::GitHubClient, server::AppState};

#[derive(Debug, Default, Serialize, Deserialize)]
struct WatchSnapshot {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    repositories: HashSet<String>,
}

/// Poll GitHub independently of the desktop UI, so repository automation keeps
/// working while HomeRun is running in the background.
pub fn start(state: AppState) {
    tokio::spawn(async move {
        let snapshot_path = {
            let config = state.config.read().await;
            config.base_dir().join("repository-watch.json")
        };
        let mut snapshot = load_snapshot(&snapshot_path).await;
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            interval.tick().await;
            let preferences = state.config.read().await.preferences.clone();

            if !preferences.watch_new_repositories {
                if snapshot.enabled {
                    snapshot.enabled = false;
                    save_snapshot(&snapshot_path, &snapshot).await;
                }
                continue;
            }

            let Some(token) = state.auth.token().await else {
                continue;
            };
            let client = match GitHubClient::new(Some(token.clone())) {
                Ok(client) => client,
                Err(error) => {
                    tracing::warn!("Repository watcher could not initialize GitHub client: {error}");
                    continue;
                }
            };
            let repos = match client.list_repos().await {
                Ok(repos) => repos,
                Err(error) => {
                    tracing::warn!("Repository watcher failed to list repositories: {error}");
                    continue;
                }
            };

            let current_names: HashSet<String> =
                repos.iter().map(|repo| repo.full_name.clone()).collect();

            // When monitoring is first enabled, take a baseline instead of
            // creating runners for every repository that already exists.
            if !snapshot.enabled {
                snapshot.repositories = current_names;
                snapshot.enabled = true;
                save_snapshot(&snapshot_path, &snapshot).await;
                tracing::info!(
                    "Repository monitoring enabled; initialized with {} repositories",
                    snapshot.repositories.len()
                );
                continue;
            }

            let newly_created: Vec<_> = repos
                .into_iter()
                .filter(|repo| !snapshot.repositories.contains(&repo.full_name))
                .collect();

            if newly_created.is_empty() {
                snapshot.repositories = current_names;
                save_snapshot(&snapshot_path, &snapshot).await;
                continue;
            }

            for repo in &newly_created {
                tracing::info!("Detected new GitHub repository: {}", repo.full_name);
            }

            if !preferences.auto_create_runner_for_new_repositories {
                snapshot.repositories = current_names;
                save_snapshot(&snapshot_path, &snapshot).await;
                continue;
            }

            // Keep failed creations out of the snapshot so a transient error
            // is retried on the next poll, rather than silently losing the repo.
            let mut next_names = current_names;
            for repo in newly_created {
                let suffix = uuid::Uuid::new_v4().simple().to_string();
                let runner_name = format!("{}-{}-runner-{}", repo.owner, repo.name, &suffix[..8]);
                match state
                    .runner_manager
                    .create(&repo.full_name, Some(runner_name), None, None, None, None)
                    .await
                {
                    Ok(runner) => {
                        let runner_id = runner.config.id.clone();
                        let runner_name = runner.config.name.clone();
                        tracing::info!(
                            "Automatically created runner {runner_name} for {}",
                            repo.full_name
                        );

                        let manager = state.runner_manager.clone();
                        let registration_token = token.clone();
                        tokio::spawn(async move {
                            if let Err(error) = manager
                                .register_and_start(&runner_id, &registration_token)
                                .await
                            {
                                tracing::error!(
                                    "Failed to register automatically created runner {runner_id}: {error:#}"
                                );
                                let _ = manager
                                    .update_state_with_error(
                                        &runner_id,
                                        crate::runner::state::RunnerState::Error,
                                        Some(format!("{error:#}")),
                                    )
                                    .await;
                            }
                        });
                    }
                    Err(error) => {
                        tracing::error!(
                            "Failed to create a runner for new repository {}: {error:#}",
                            repo.full_name
                        );
                        next_names.remove(&repo.full_name);
                    }
                }
            }

            snapshot.repositories = next_names;
            save_snapshot(&snapshot_path, &snapshot).await;
        }
    });
}

async fn load_snapshot(path: &Path) -> WatchSnapshot {
    match tokio::fs::read(path).await {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                tracing::warn!("Ignoring invalid repository watcher state: {error}");
                WatchSnapshot::default()
            }
        },
        Err(_) => WatchSnapshot::default(),
    }
}

async fn save_snapshot(path: &Path, snapshot: &WatchSnapshot) {
    match serde_json::to_vec_pretty(snapshot) {
        Ok(bytes) => {
            if let Err(error) = tokio::fs::write(path, bytes).await {
                tracing::warn!("Failed to save repository watcher state: {error}");
            }
        }
        Err(error) => tracing::warn!("Failed to serialize repository watcher state: {error}"),
    }
}
