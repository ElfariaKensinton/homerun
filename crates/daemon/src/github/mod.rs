pub mod types;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use types::{RepoInfo, RunnerRegistration};

/// Check whether an `anyhow::Error` wraps a GitHub 401 "Bad credentials" response.
pub fn is_bad_credentials(err: &anyhow::Error) -> bool {
    let msg = format!("{err:#}");
    // octocrab surfaces 401s as "GitHub: Bad credentials" or with status 401
    msg.contains("Bad credentials") || msg.contains("status code 401")
}

#[derive(Debug, Deserialize)]
struct GitHubJobStep {
    name: String,
    number: u16,
    #[serde(default)]
    started_at: Option<String>,
    #[serde(default)]
    completed_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GitHubJobDetails {
    #[serde(default)]
    steps: Vec<GitHubJobStep>,
}

pub struct GitHubClient {
    octocrab: octocrab::Octocrab,
    token: String,
}

impl GitHubClient {
    pub fn new(token: Option<String>) -> Result<Self> {
        let Some(token) = token else {
            bail!("Not authenticated — please login first");
        };
        let octocrab = octocrab::Octocrab::builder()
            .personal_token(token.clone())
            .build()?;
        Ok(Self { octocrab, token })
    }

    pub async fn list_repos(&self) -> Result<Vec<RepoInfo>> {
        let first_page = self
            .octocrab
            .current()
            .list_repos_for_authenticated_user()
            .per_page(100)
            .send()
            .await?;

        let repos = self.octocrab.all_pages(first_page).await?;

        let result = repos
            .into_iter()
            .filter_map(|repo| {
                let owner = repo.owner.as_ref()?;
                let is_org = owner.r#type == "Organization";
                Some(RepoInfo {
                    id: repo.id.0,
                    full_name: repo
                        .full_name
                        .clone()
                        .unwrap_or_else(|| format!("{}/{}", owner.login, repo.name)),
                    name: repo.name.clone(),
                    owner: owner.login.clone(),
                    private: repo.private.unwrap_or(false),
                    html_url: repo
                        .html_url
                        .as_ref()
                        .map(|u| u.to_string())
                        .unwrap_or_default(),
                    is_org,
                })
            })
            .collect();

        Ok(result)
    }

    /// Fetch `.github/workflows/` contents for `owner/repo` and return the
    /// relative file paths of workflow files that contain any of the given
    /// `runs-on:` labels, together with the set of labels that matched.
    pub async fn list_workflows_with_labels(
        &self,
        owner: &str,
        repo: &str,
        labels: &[String],
    ) -> Result<(Vec<String>, Vec<String>)> {
        #[derive(Deserialize)]
        struct ContentItem {
            name: String,
            download_url: Option<String>,
            #[serde(rename = "type")]
            item_type: String,
        }

        let route = format!("/repos/{owner}/{repo}/contents/.github/workflows");
        let items: Vec<ContentItem> = match self.octocrab.get(route, None::<&()>).await {
            Ok(v) => v,
            Err(_) => return Ok((Vec::new(), Vec::new())), // directory missing or no access
        };

        let mut matching: Vec<String> = Vec::new();
        let mut matched_labels: Vec<String> = Vec::new();

        for item in items {
            if item.item_type != "file" {
                continue;
            }
            let ext = std::path::Path::new(&item.name)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            if ext != "yml" && ext != "yaml" {
                continue;
            }

            let download_url = match item.download_url {
                Some(u) => u,
                None => continue,
            };

            // _get fetches a URL; body_to_string reads the response body
            let response = match self.octocrab._get(download_url).await {
                Ok(r) => r,
                Err(_) => continue,
            };
            let content = match self.octocrab.body_to_string(response).await {
                Ok(s) => s,
                Err(_) => continue,
            };

            let file_matched = labels
                .iter()
                .any(|label| content.contains(&format!("runs-on: {}", label)));
            if file_matched {
                matching.push(format!(".github/workflows/{}", item.name));
                for label in labels {
                    if content.contains(&format!("runs-on: {}", label))
                        && !matched_labels.contains(label)
                    {
                        matched_labels.push(label.clone());
                    }
                }
            }
        }

        matching.sort();
        matched_labels.sort();
        Ok((matching, matched_labels))
    }

    pub async fn get_runner_registration_token(
        &self,
        owner: &str,
        repo: &str,
    ) -> Result<RunnerRegistration> {
        #[derive(Deserialize)]
        struct RegistrationTokenResponse {
            token: String,
            expires_at: String,
        }

        let route = format!("/repos/{owner}/{repo}/actions/runners/registration-token");
        let response: RegistrationTokenResponse = self.octocrab.post(route, None::<&()>).await?;

        Ok(RunnerRegistration {
            token: response.token,
            expires_at: response.expires_at,
        })
    }

    /// Find the in-progress workflow run that matches this runner.
    ///
    /// Matches by job name (from runner stdout) against the GitHub API's job
    /// list, since the `runner_name` field on jobs may not be populated yet
    /// while a job is still in progress.
    pub async fn get_active_run_for_runner(
        &self,
        owner: &str,
        repo: &str,
        runner_name: &str,
        job_name: &str,
    ) -> Result<Option<crate::runner::types::JobContext>> {
        #[derive(Deserialize)]
        struct WorkflowRun {
            id: u64,
            head_branch: String,
            html_url: String,
            pull_requests: Vec<PullRequestRef>,
        }

        #[derive(Deserialize)]
        struct PullRequestRef {
            number: u64,
            url: String,
        }

        #[derive(Deserialize)]
        struct RunsResponse {
            workflow_runs: Vec<WorkflowRun>,
        }

        #[derive(Deserialize)]
        struct RunJob {
            id: u64,
            name: String,
            runner_name: Option<String>,
        }

        #[derive(Deserialize)]
        struct JobsResponse {
            jobs: Vec<RunJob>,
        }

        let route = format!("/repos/{owner}/{repo}/actions/runs?status=in_progress");
        let runs: RunsResponse = match self.octocrab.get(&route, None::<&()>).await {
            Ok(r) => r,
            Err(_) => return Ok(None),
        };

        for run in runs.workflow_runs {
            let jobs_route = format!("/repos/{owner}/{repo}/actions/runs/{}/jobs", run.id);
            let jobs: JobsResponse = match self.octocrab.get(&jobs_route, None::<&()>).await {
                Ok(j) => j,
                Err(_) => continue,
            };

            // Match by runner_name if available, otherwise fall back to job name
            let matched_job = jobs.jobs.iter().find(|j| {
                if let Some(rn) = j.runner_name.as_deref() {
                    rn == runner_name
                } else {
                    j.name == job_name
                }
            });

            if let Some(job) = matched_job {
                let (pr_number, pr_url) = if let Some(pr) = run.pull_requests.first() {
                    let html_url = pr
                        .url
                        .replace("api.github.com/repos", "github.com")
                        .replace("/pulls/", "/pull/");
                    (Some(pr.number), Some(html_url))
                } else {
                    (None, None)
                };

                return Ok(Some(crate::runner::types::JobContext {
                    branch: run.head_branch,
                    pr_number,
                    pr_url,
                    run_url: run.html_url,
                    job_id: Some(job.id),
                }));
            }
        }

        Ok(None)
    }

    /// Find a recent workflow run matching a job name (any status).
    /// Used at job completion to fill in missing context for fast jobs
    /// that complete before the periodic poller can fetch their context.
    pub async fn get_recent_run_for_job(
        &self,
        owner: &str,
        repo: &str,
        runner_name: &str,
        job_name: &str,
    ) -> Result<Option<crate::runner::types::JobContext>> {
        #[derive(Deserialize)]
        struct WorkflowRun {
            id: u64,
            head_branch: String,
            html_url: String,
            pull_requests: Vec<PullRequestRef>,
        }

        #[derive(Deserialize)]
        struct PullRequestRef {
            number: u64,
            url: String,
        }

        #[derive(Deserialize)]
        struct RunsResponse {
            workflow_runs: Vec<WorkflowRun>,
        }

        #[derive(Deserialize)]
        struct RunJob {
            id: u64,
            name: String,
            runner_name: Option<String>,
        }

        #[derive(Deserialize)]
        struct JobsResponse {
            jobs: Vec<RunJob>,
        }

        // Query recent runs (no status filter) to catch just-completed runs
        let route = format!("/repos/{owner}/{repo}/actions/runs?per_page=5");
        let runs: RunsResponse = match self.octocrab.get(&route, None::<&()>).await {
            Ok(r) => r,
            Err(_) => return Ok(None),
        };

        for run in runs.workflow_runs {
            let jobs_route = format!("/repos/{owner}/{repo}/actions/runs/{}/jobs", run.id);
            let jobs: JobsResponse = match self.octocrab.get(&jobs_route, None::<&()>).await {
                Ok(j) => j,
                Err(_) => continue,
            };

            let matched_job = jobs.jobs.iter().find(|j| {
                if let Some(rn) = j.runner_name.as_deref() {
                    rn == runner_name
                } else {
                    j.name == job_name
                }
            });

            if let Some(job) = matched_job {
                let (pr_number, pr_url) = if let Some(pr) = run.pull_requests.first() {
                    let html_url = pr
                        .url
                        .replace("api.github.com/repos", "github.com")
                        .replace("/pulls/", "/pull/");
                    (Some(pr.number), Some(html_url))
                } else {
                    (None, None)
                };

                return Ok(Some(crate::runner::types::JobContext {
                    branch: run.head_branch,
                    pr_number,
                    pr_url,
                    run_url: run.html_url,
                    job_id: Some(job.id),
                }));
            }
        }

        Ok(None)
    }

    /// Find an active job for an organization-level runner across all repositories.
    pub async fn get_active_run_for_org_runner(
        &self,
        organization: &str,
        runner_name: &str,
        job_name: &str,
    ) -> Result<Option<crate::runner::types::JobContext>> {
        self.find_org_runner_job(organization, runner_name, job_name, true).await
    }

    /// Find the most recent job for an organization-level runner across all repositories.
    pub async fn get_recent_run_for_org_runner(
        &self,
        organization: &str,
        runner_name: &str,
        job_name: &str,
    ) -> Result<Option<crate::runner::types::JobContext>> {
        self.find_org_runner_job(organization, runner_name, job_name, false).await
    }

    async fn find_org_runner_job(
        &self,
        organization: &str,
        runner_name: &str,
        job_name: &str,
        active_only: bool,
    ) -> Result<Option<crate::runner::types::JobContext>> {
        #[derive(Deserialize)]
        struct OrgRepo {
            name: String,
        }
        #[derive(Deserialize)]
        struct PullRequestRef {
            number: u64,
            url: String,
        }
        #[derive(Deserialize)]
        struct WorkflowRun {
            id: u64,
            head_branch: String,
            html_url: String,
            #[serde(default)]
            updated_at: Option<String>,
            #[serde(default)]
            pull_requests: Vec<PullRequestRef>,
        }
        #[derive(Deserialize)]
        struct RunsResponse {
            #[serde(default)]
            workflow_runs: Vec<WorkflowRun>,
        }
        #[derive(Deserialize)]
        struct RunJob {
            id: u64,
            name: String,
            runner_name: Option<String>,
        }
        #[derive(Deserialize)]
        struct JobsResponse {
            #[serde(default)]
            jobs: Vec<RunJob>,
        }

        if organization.trim().is_empty() || organization.contains('/') {
            bail!("Invalid organization login");
        }

        // GitHub exposes workflow runs per repository, not through an
        // /orgs/{org}/actions/runs endpoint. Enumerate repositories, then inspect
        // their recent runs to resolve which repository an organization runner used.
        let mut repository_page = 1_u32;
        let mut runner_matches: Vec<(Option<String>, crate::runner::types::JobContext)> =
            Vec::new();
        let mut name_matches: Vec<(Option<String>, crate::runner::types::JobContext)> =
            Vec::new();

        loop {
            let repositories_route = format!(
                "/orgs/{organization}/repos?type=all&sort=updated&direction=desc&per_page=100&page={repository_page}"
            );
            let repositories: Vec<OrgRepo> =
                match self.octocrab.get(&repositories_route, None::<&()>).await {
                    Ok(repositories) => repositories,
                    Err(error) => {
                        tracing::debug!(
                            organization,
                            repository_page,
                            error = %error,
                            "Could not list organization repositories for runner job lookup"
                        );
                        break;
                    }
                };

            if repositories.is_empty() {
                break;
            }

            for repository in &repositories {
                let status_filter = if active_only {
                    "&status=in_progress"
                } else {
                    ""
                };
                let runs_route = format!(
                    "/repos/{organization}/{}/actions/runs?per_page=20{status_filter}",
                    repository.name
                );
                let runs: RunsResponse =
                    match self.octocrab.get(&runs_route, None::<&()>).await {
                        Ok(runs) => runs,
                        Err(error) => {
                            tracing::trace!(
                                organization,
                                repository = %repository.name,
                                error = %error,
                                "Could not list repository workflow runs for organization runner lookup"
                            );
                            continue;
                        }
                    };

                for run in runs.workflow_runs {
                    let jobs_route = format!(
                        "/repos/{organization}/{}/actions/runs/{}/jobs?per_page=100",
                        repository.name, run.id
                    );
                    let jobs: JobsResponse =
                        match self.octocrab.get(&jobs_route, None::<&()>).await {
                            Ok(jobs) => jobs,
                            Err(_) => continue,
                        };

                    let (pr_number, pr_url) = if let Some(pr) = run.pull_requests.first() {
                        (
                            Some(pr.number),
                            Some(pr.url
                                .replace("api.github.com/repos", "github.com")
                                .replace("/pulls/", "/pull/")),
                        )
                    } else {
                        (None, None)
                    };

                    for job in jobs.jobs {
                        let context = crate::runner::types::JobContext {
                            branch: run.head_branch.clone(),
                            pr_number,
                            pr_url: pr_url.clone(),
                            run_url: run.html_url.clone(),
                            job_id: Some(job.id),
                        };

                        if job.runner_name.as_deref() == Some(runner_name) {
                            // An active runner can only be executing one job at a time.
                            if active_only {
                                return Ok(Some(context));
                            }
                            runner_matches.push((run.updated_at.clone(), context));
                        } else if job.runner_name.is_none() && job.name == job_name {
                            // Runner names are occasionally omitted by the API. Only
                            // accept job-name fallback when it is unambiguous org-wide.
                            name_matches.push((run.updated_at.clone(), context));
                        }
                    }
                }
            }

            if repositories.len() < 100 {
                break;
            }
            repository_page += 1;
        }

        if !runner_matches.is_empty() {
            // The same organization runner appears in older completed jobs. Prefer
            // the most recently updated workflow run across all repositories.
            runner_matches.sort_by(|left, right| right.0.cmp(&left.0));
            return Ok(runner_matches.into_iter().next().map(|(_, context)| context));
        }

        if name_matches.len() == 1 {
            Ok(name_matches.pop().map(|(_, context)| context))
        } else {
            Ok(None)
        }
    }

    /// Check the status of a workflow run. Returns (status, conclusion).
    /// Status: "queued", "in_progress", "completed", etc.
    /// Conclusion (only when completed): "success", "failure", "cancelled", etc.
    pub async fn get_run_status(
        &self,
        owner: &str,
        repo: &str,
        run_id: u64,
    ) -> Result<(String, Option<String>)> {
        #[derive(Deserialize)]
        struct RunInfo {
            status: String,
            conclusion: Option<String>,
        }
        let route = format!("/repos/{owner}/{repo}/actions/runs/{run_id}");
        let info: RunInfo = self.octocrab.get(&route, None::<&()>).await?;
        Ok((info.status, info.conclusion))
    }

    /// Re-run a workflow run by its run ID.
    pub async fn rerun_workflow(&self, owner: &str, repo: &str, run_id: u64) -> Result<()> {
        let url =
            format!("https://api.github.com/repos/{owner}/{repo}/actions/runs/{run_id}/rerun");
        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "homerun")
            .send()
            .await?;

        if !response.status().is_success() {
            bail!(
                "Failed to re-run workflow: HTTP {}",
                response.status().as_u16()
            );
        }
        Ok(())
    }

    /// Fetch the raw log content for a specific job from GitHub Actions.
    ///
    /// The GitHub API endpoint returns a 302 redirect to blob storage serving
    /// plain text. Since octocrab expects JSON responses, we use reqwest directly.
    pub async fn get_job_logs(&self, owner: &str, repo: &str, job_id: u64) -> Result<String> {
        let url = format!("https://api.github.com/repos/{owner}/{repo}/actions/jobs/{job_id}/logs");
        let client = reqwest::Client::new();
        let response = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "homerun")
            .send()
            .await?;

        if !response.status().is_success() {
            bail!(
                "Failed to fetch job logs: HTTP {}",
                response.status().as_u16()
            );
        }

        let body = response.text().await?;
        Ok(body)
    }

    async fn get_job_details(
        &self,
        owner: &str,
        repo: &str,
        job_id: u64,
    ) -> Result<GitHubJobDetails> {
        let route = format!("/repos/{owner}/{repo}/actions/jobs/{job_id}");
        Ok(self.octocrab.get(&route, None::<&()>).await?)
    }

    /// Resolve a locally discovered step name to the actual GitHub step number.
    /// The local list omits GitHub's synthetic "Set up job" entry, so using the
    /// local index to open the runner's step-{number}.log sidecar selects the prior
    /// step (for example, local Checkout #1 vs GitHub Checkout #2).
    pub async fn get_job_step_number(
        &self,
        owner: &str,
        repo: &str,
        job_id: u64,
        step_name: &str,
        local_step_number: u16,
    ) -> Result<u16> {
        let job = self.get_job_details(owner, repo, job_id).await?;
        resolve_job_step_number(&job.steps, step_name, local_step_number)
    }

    /// Fetch plain-text logs for one GitHub Actions step.
    ///
    /// HomeRun step numbers are 1-based. GitHub's step-log endpoint uses a
    /// zero-based position, so resolve the local display name against job
    /// metadata first. Fall back to the same position when names differ.
    pub async fn get_job_step_logs(
        &self,
        owner: &str,
        repo: &str,
        job_id: u64,
        step_name: &str,
        local_step_number: u16,
    ) -> Result<String> {
        let job = self.get_job_details(owner, repo, job_id).await?;
        let step_index = resolve_job_step_index(
            &job.steps.iter().map(|step| step.name.clone()).collect::<Vec<_>>(),
            step_name,
            local_step_number,
        )?;

        let url = format!(
            "https://api.github.com/repos/{owner}/{repo}/actions/jobs/{job_id}/steps/{step_index}/logs"
        );
        let response = reqwest::Client::new()
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "homerun")
            .send()
            .await?;

        if response.status().is_success() {
            return Ok(response.text().await?);
        }

        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        // GitHub sometimes returns 404 for an individual step-log download even
        // when the job log itself is available. Recover from the full job log,
        // using the step's own API timestamps so neighboring steps are never mixed
        // into this response. Never fall back to the local step number or raw
        // Worker diagnostics: both can attribute output to the wrong step.
        if status.as_u16() == 404 {
            if let (Some(started_at), Some(completed_at)) = (
                job.steps[step_index].started_at.as_deref(),
                job.steps[step_index].completed_at.as_deref(),
            ) {
                let full_log = self
                    .get_job_logs(owner, repo, job_id)
                    .await
                    .with_context(|| {
                        format!(
                            "Step '{step_name}' log download returned 404 and the full job log could not be fetched"
                        )
                    })?;
                if let Some(step_log) =
                    extract_step_logs_by_time(&full_log, started_at, completed_at)
                {
                    return Ok(step_log);
                }
            }
        }

        let detail: String = body.chars().take(240).collect();
        bail!(
            "Failed to fetch logs for step '{step_name}' (GitHub step index {step_index}): HTTP {}{}",
            status.as_u16(),
            if detail.trim().is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        )
    }

    /// Fetch the error/failure message for a completed job via check-run annotations.
    /// Searches recent runs to find the job by runner name, then fetches annotations.
    pub async fn get_job_failure_message(
        &self,
        owner: &str,
        repo: &str,
        runner_name: &str,
        job_name: &str,
    ) -> Result<Option<String>> {
        // Find the job_id by searching recent runs
        #[derive(Deserialize)]
        struct WorkflowRun {
            id: u64,
        }
        #[derive(Deserialize)]
        struct RunsResponse {
            workflow_runs: Vec<WorkflowRun>,
        }
        #[derive(Deserialize)]
        struct RunJob {
            id: u64,
            name: String,
            runner_name: Option<String>,
        }
        #[derive(Deserialize)]
        struct JobsResponse {
            jobs: Vec<RunJob>,
        }

        let route = format!("/repos/{owner}/{repo}/actions/runs?per_page=5");
        let runs: RunsResponse = self.octocrab.get(&route, None::<&()>).await?;

        let mut job_id = None;
        for run in runs.workflow_runs {
            let jobs_route = format!("/repos/{owner}/{repo}/actions/runs/{}/jobs", run.id);
            if let Ok(jobs) = self
                .octocrab
                .get::<JobsResponse, _, _>(&jobs_route, None::<&()>)
                .await
            {
                if let Some(j) = jobs
                    .jobs
                    .iter()
                    .find(|j| j.runner_name.as_deref() == Some(runner_name) || j.name == job_name)
                {
                    job_id = Some(j.id);
                    break;
                }
            }
        }

        let Some(job_id) = job_id else {
            return Ok(None);
        };

        // Fetch annotations for this job (reuse get_annotations_by_job_id)
        self.get_annotations_by_job_id(owner, repo, job_id).await
    }

    /// Fetch error/failure annotations directly by job ID.
    ///
    /// Preferred over [`get_job_failure_message`] when the job ID is already known
    /// (e.g. from [`JobContext::job_id`]), as it avoids searching recent runs which
    /// can match the wrong run under concurrency cancellation.
    pub async fn get_annotations_by_job_id(
        &self,
        owner: &str,
        repo: &str,
        job_id: u64,
    ) -> Result<Option<String>> {
        let url =
            format!("https://api.github.com/repos/{owner}/{repo}/check-runs/{job_id}/annotations");
        let client = reqwest::Client::new();
        let response = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "homerun")
            .send()
            .await?;

        if !response.status().is_success() {
            return Ok(None);
        }

        let body = response.text().await?;
        Ok(extract_annotation_message(&body))
    }
}

/// Parse a GitHub Actions run URL and return its repository owner/name.
pub fn repository_from_run_url(run_url: &str) -> Option<(String, String)> {
    let url = reqwest::Url::parse(run_url).ok()?;
    let segments = url.path_segments()?.filter(|segment| !segment.is_empty()).collect::<Vec<_>>();
    let actions_index = segments.iter().position(|segment| *segment == "actions")?;
    if actions_index < 2 {
        return None;
    }
    Some((
        segments[actions_index - 2].to_string(),
        segments[actions_index - 1].to_string(),
    ))
}

/// Extract the first annotation message from a GitHub check-run annotations JSON response.
fn extract_annotation_message(json: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Annotation {
        message: Option<String>,
    }

    let annotations: Vec<Annotation> = serde_json::from_str(json).ok()?;
    annotations.into_iter().find_map(|a| a.message)
}

/// Extract only lines whose GitHub log timestamps fall inside one step's API time window.
/// Returns None if the step timestamps cannot be parsed; an empty string is a valid
/// result when GitHub has no output lines inside that step's interval.
fn extract_step_logs_by_time(
    raw_log: &str,
    started_at: &str,
    completed_at: &str,
) -> Option<String> {
    let start = chrono::DateTime::parse_from_rfc3339(started_at).ok()?;
    let end = chrono::DateTime::parse_from_rfc3339(completed_at).ok()?;
    if end < start {
        return None;
    }

    let mut lines = Vec::new();
    for raw_line in raw_log.lines() {
        let line = raw_line.trim_start_matches('\u{feff}');
        let Some((timestamp, message)) = line.split_once(' ') else {
            continue;
        };
        let Ok(timestamp) = chrono::DateTime::parse_from_rfc3339(timestamp) else {
            continue;
        };

        if timestamp >= start && timestamp <= end {
            lines.push(message.to_string());
        }
    }

    Some(lines.join("\n"))
}

/// Resolve a local displayed step to its actual GitHub step number.
/// The returned number is the timeline order used by the patched runner's sidecar.
fn resolve_job_step_number(
    steps: &[GitHubJobStep],
    step_name: &str,
    local_step_number: u16,
) -> Result<u16> {
    let names = steps.iter().map(|step| step.name.clone()).collect::<Vec<_>>();
    let index = resolve_job_step_index(&names, step_name, local_step_number)?;
    Ok(steps[index].number)
}

/// Resolve a HomeRun step to exactly one GitHub job step by normalized name.
///
/// Never fall back to the local numeric position: Worker-derived step numbers can be
/// offset from GitHub's job-step list (which also contains setup and post steps).
/// Returning an error is safer than presenting a different step's log under this name.
fn resolve_job_step_index(
    step_names: &[String],
    step_name: &str,
    local_step_number: u16,
) -> anyhow::Result<usize> {
    let normalize = |name: &str| -> String {
        name.chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .map(|ch| ch.to_ascii_lowercase())
            .collect()
    };
    let requested = normalize(step_name);
    let matches = step_names
        .iter()
        .enumerate()
        .filter(|(_, candidate)| normalize(candidate) == requested)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();

    match matches.as_slice() {
        [index] => Ok(*index),
        [] => anyhow::bail!(
            "Could not safely map HomeRun step {local_step_number} ('{step_name}') to a GitHub job step by name; GitHub reported {} steps",
            step_names.len()
        ),
        _ => anyhow::bail!(
            "Could not safely map HomeRun step {local_step_number} ('{step_name}') because its name matches multiple GitHub steps at zero-based positions {matches:?}; refusing to return potentially incorrect logs"
        ),
    }
}

/// Parse raw GitHub Actions job log text into sections by step name.
///
/// The log format uses `##[group]Step Name` / `##[endgroup]` markers to delimit
/// steps, with each line prefixed by a 29-character timestamp
/// (`2026-03-23T07:54:51.0000000Z `).
///
/// Returns a `Vec<(step_name, log_content)>`.
pub fn parse_job_log_sections(raw_log: &str) -> Vec<(String, String)> {
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_lines: Vec<String> = Vec::new();

    for line in raw_log.lines() {
        // Strip the timestamp prefix (29 chars: 28 for timestamp + 'Z', then a space)
        let content = if line.len() > 29 { &line[29..] } else { line };
        let content = content.trim_start();

        if let Some(name) = content.strip_prefix("##[group]") {
            // Close any open section before starting a new one
            if let Some(name) = current_name.take() {
                sections.push((name, current_lines.join("\n")));
                current_lines.clear();
            }
            current_name = Some(name.to_string());
        } else if content == "##[endgroup]" {
            if let Some(name) = current_name.take() {
                sections.push((name, current_lines.join("\n")));
                current_lines.clear();
            }
        } else if current_name.is_some() {
            current_lines.push(content.to_string());
        }
    }

    // Handle unterminated section (step still running)
    if let Some(name) = current_name.take() {
        sections.push((name, current_lines.join("\n")));
    }

    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_log_time_fallback_excludes_neighboring_steps() {
        let raw_log = concat!(
            "2026-10-15T04:00:00.0000000Z previous step output\n",
            "2026-10-15T04:00:01.0000000Z Build command\n",
            "2026-10-15T04:00:02.0000000Z Build result\n",
            "2026-10-15T04:00:03.0000000Z next step output\n",
        );

        let output = extract_step_logs_by_time(
            raw_log,
            "2026-10-15T04:00:01Z",
            "2026-10-15T04:00:02Z",
        )
        .unwrap();

        assert_eq!(output, "Build command\nBuild result");
    }

    #[test]
    fn step_log_time_fallback_handles_bom_and_invalid_lines() {
        let raw_log = concat!(
            "\u{feff}2026-10-15T04:00:01.0000000Z Build output\n",
            "not a timestamp\n",
        );

        let output = extract_step_logs_by_time(
            raw_log,
            "2026-10-15T04:00:01Z",
            "2026-10-15T04:00:02Z",
        ).unwrap();

        assert_eq!(output, "Build output");
    }

    #[test]
    fn live_sidecar_uses_github_step_number_not_compact_local_number() {
        let steps = vec![
            GitHubJobStep {
                name: "Set up job".to_string(),
                number: 1,
                started_at: None,
                completed_at: None,
            },
            GitHubJobStep {
                name: "Run actions/checkout@v4".to_string(),
                number: 2,
                started_at: None,
                completed_at: None,
            },
            GitHubJobStep {
                name: "Generate project".to_string(),
                number: 3,
                started_at: None,
                completed_at: None,
            },
            GitHubJobStep {
                name: "Build".to_string(),
                number: 4,
                started_at: None,
                completed_at: None,
            },
        ];

        assert_eq!(resolve_job_step_number(&steps, "Run actions/checkout@v4", 1).unwrap(), 2);
        assert_eq!(resolve_job_step_number(&steps, "Build", 3).unwrap(), 4);
    }

    #[test]
    fn step_logs_resolve_by_name_even_when_local_number_is_offset() {
        let names = vec![
            "Set up job".to_string(),
            "Run actions/checkout@v6".to_string(),
            "Build".to_string(),
        ];
        assert_eq!(resolve_job_step_index(&names, "Build", 1).unwrap(), 2);
    }

    #[test]
    fn step_logs_do_not_fall_back_to_an_unrelated_position() {
        let names = vec![
            "Set up job".to_string(),
            "Checkout".to_string(),
            "Build".to_string(),
        ];
        let error = resolve_job_step_index(&names, "Lint", 2).unwrap_err();
        assert!(error.to_string().contains("Could not safely map"));
    }

    #[test]
    fn step_logs_reject_ambiguous_duplicate_names() {
        let names = vec!["Test".to_string(), "Test".to_string()];
        let error = resolve_job_step_index(&names, "Test", 2).unwrap_err();
        assert!(error.to_string().contains("multiple GitHub steps"));
    }

    #[test]
    fn test_github_client_requires_token() {
        let result = GitHubClient::new(None);
        assert!(result.is_err());
    }

    #[test]
    fn test_github_client_none_error_message() {
        let err = GitHubClient::new(None).err().unwrap();
        assert!(err.to_string().contains("Not authenticated") || err.to_string().contains("login"));
    }

    #[tokio::test]
    async fn test_github_client_with_token() {
        let client = GitHubClient::new(Some("ghp_test".to_string()));
        assert!(client.is_ok());
    }

    #[tokio::test]
    async fn test_github_client_with_some_token_creates_ok() {
        // Any non-empty token string should create a client (validation happens on API call)
        let client = GitHubClient::new(Some("fake-token-value".to_string()));
        assert!(client.is_ok());
    }

    #[tokio::test]
    async fn test_github_client_empty_token_creates_ok() {
        // An empty token string is still a Some(...), so it should build successfully
        // — authentication errors only surface when making actual API calls.
        let result = GitHubClient::new(Some(String::new()));
        assert!(result.is_ok());
    }

    #[test]
    fn test_github_client_none_error_contains_not_authenticated() {
        let err = GitHubClient::new(None).err().unwrap();
        let msg = err.to_string();
        assert!(
            msg.contains("Not authenticated"),
            "expected 'Not authenticated' in error, got: {msg}"
        );
    }

    #[test]
    fn test_github_client_none_error_contains_login() {
        let err = GitHubClient::new(None).err().unwrap();
        let msg = err.to_string();
        assert!(
            msg.contains("login"),
            "expected 'login' in error, got: {msg}"
        );
    }

    /// Verify that list_repos returns an error (not a panic) when called
    /// with an invalid token — exercises the API call error path.
    #[tokio::test]
    async fn test_list_repos_with_invalid_token_returns_err() {
        let client = GitHubClient::new(Some("invalid_token_xyz".to_string())).unwrap();
        let result = client.list_repos().await;
        assert!(
            result.is_err(),
            "expected API call to fail with invalid token"
        );
    }

    /// Verify that get_runner_registration_token returns an error for an invalid token.
    #[tokio::test]
    async fn test_get_runner_registration_token_invalid_token_returns_err() {
        let client = GitHubClient::new(Some("invalid_token_xyz".to_string())).unwrap();
        let result = client
            .get_runner_registration_token("fake-owner", "fake-repo")
            .await;
        assert!(
            result.is_err(),
            "expected API call to fail with invalid token"
        );
    }

    /// Verify that list_workflows_with_labels returns empty tuples (not an error)
    /// when the workflows directory is inaccessible (error is swallowed by design).
    #[tokio::test]
    async fn test_list_workflows_with_labels_inaccessible_repo_returns_empty() {
        let client = GitHubClient::new(Some("invalid_token_xyz".to_string())).unwrap();
        let labels = vec!["self-hosted".to_string()];
        let result = client
            .list_workflows_with_labels("fake-owner-xyz-123", "fake-repo-xyz-456", &labels)
            .await;
        // By design the function swallows errors and returns Ok((Vec::new(), Vec::new()))
        assert!(result.is_ok());
        let (files, matched) = result.unwrap();
        assert!(files.is_empty());
        assert!(matched.is_empty());
    }

    #[test]
    fn test_runner_registration_type_fields() {
        use types::RunnerRegistration;
        let reg = RunnerRegistration {
            token: "token123".to_string(),
            expires_at: "2030-01-01T00:00:00Z".to_string(),
        };
        assert_eq!(reg.token, "token123");
        assert_eq!(reg.expires_at, "2030-01-01T00:00:00Z");
    }

    #[test]
    fn test_repo_info_fields() {
        use types::RepoInfo;
        let repo = RepoInfo {
            id: 42,
            full_name: "owner/repo".to_string(),
            name: "repo".to_string(),
            owner: "owner".to_string(),
            private: true,
            html_url: "https://github.com/owner/repo".to_string(),
            is_org: false,
        };
        assert_eq!(repo.id, 42);
        assert!(repo.private);
        assert!(!repo.is_org);
    }

    #[test]
    fn test_repo_info_is_org_true() {
        use types::RepoInfo;
        let repo = RepoInfo {
            id: 1,
            full_name: "myorg/tool".to_string(),
            name: "tool".to_string(),
            owner: "myorg".to_string(),
            private: false,
            html_url: "https://github.com/myorg/tool".to_string(),
            is_org: true,
        };
        assert!(repo.is_org);
        assert!(!repo.private);
    }

    #[test]
    fn test_repo_info_serialization_roundtrip() {
        use types::RepoInfo;
        let repo = RepoInfo {
            id: 99,
            full_name: "a/b".to_string(),
            name: "b".to_string(),
            owner: "a".to_string(),
            private: false,
            html_url: "https://github.com/a/b".to_string(),
            is_org: false,
        };
        let json = serde_json::to_string(&repo).unwrap();
        let back: RepoInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, repo.id);
        assert_eq!(back.full_name, repo.full_name);
        assert_eq!(back.is_org, repo.is_org);
    }

    #[test]
    fn test_runner_registration_serialization_roundtrip() {
        use types::RunnerRegistration;
        let reg = RunnerRegistration {
            token: "abc123".to_string(),
            expires_at: "2030-12-31T23:59:59Z".to_string(),
        };
        let json = serde_json::to_string(&reg).unwrap();
        let back: RunnerRegistration = serde_json::from_str(&json).unwrap();
        assert_eq!(back.token, "abc123");
        assert_eq!(back.expires_at, "2030-12-31T23:59:59Z");
    }

    #[tokio::test]
    async fn test_github_client_builder_with_valid_token() {
        // Creating a client with a valid-format token should succeed
        let client = GitHubClient::new(Some("ghp_abcdefghij0123456789".to_string()));
        assert!(
            client.is_ok(),
            "valid-format token should build client successfully"
        );
    }

    #[tokio::test]
    async fn test_list_repos_with_empty_token_returns_err() {
        // An empty string token will fail GitHub's API validation
        let client = GitHubClient::new(Some(String::new())).unwrap();
        let result = client.list_repos().await;
        assert!(result.is_err(), "expected error with empty token");
    }

    #[test]
    fn test_parse_job_log_into_steps() {
        let raw_log = "2026-03-23T07:54:51.0000000Z ##[group]Run actions/checkout@v6\n\
            2026-03-23T07:54:52.0000000Z Syncing repository: owner/repo\n\
            2026-03-23T07:54:53.0000000Z ##[endgroup]\n\
            2026-03-23T07:54:53.0000000Z ##[group]Check formatting\n\
            2026-03-23T07:54:54.0000000Z + cargo fmt --check\n\
            2026-03-23T07:54:55.0000000Z ##[endgroup]\n";

        let sections = parse_job_log_sections(raw_log);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].0, "Run actions/checkout@v6");
        assert!(sections[0].1.contains("Syncing repository"));
        assert_eq!(sections[1].0, "Check formatting");
        assert!(sections[1].1.contains("cargo fmt"));
    }

    #[test]
    fn test_parse_job_log_unterminated_section() {
        let raw_log = "2026-03-23T07:54:51.0000000Z ##[group]Running step\n\
            2026-03-23T07:54:52.0000000Z Some output here\n";

        let sections = parse_job_log_sections(raw_log);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].0, "Running step");
        assert!(sections[0].1.contains("Some output here"));
    }

    #[test]
    fn test_parse_job_log_empty_input() {
        let sections = parse_job_log_sections("");
        assert!(sections.is_empty());
    }

    #[tokio::test]
    async fn test_list_workflows_with_labels_empty_token_returns_empty() {
        // Even with an empty token, the function should return Ok(empty tuples)
        // because it swallows directory-access errors.
        let client = GitHubClient::new(Some(String::new())).unwrap();
        let labels = vec!["self-hosted".to_string()];
        let result = client
            .list_workflows_with_labels("any-owner", "any-repo", &labels)
            .await;
        assert!(result.is_ok());
        let (files, matched) = result.unwrap();
        assert!(files.is_empty());
        assert!(matched.is_empty());
    }

    #[test]
    fn test_extract_annotation_message_with_cancellation() {
        let json = r#"[
            {"message": "Canceling since a higher priority waiting request exists", "annotation_level": "failure"},
            {"message": "The operation was canceled.", "annotation_level": "failure"}
        ]"#;
        let result = extract_annotation_message(json);
        assert_eq!(
            result.as_deref(),
            Some("Canceling since a higher priority waiting request exists")
        );
    }

    #[test]
    fn test_extract_annotation_message_with_test_failure() {
        let json = r#"[
            {"message": "Process completed with exit code 1.", "annotation_level": "failure"}
        ]"#;
        let result = extract_annotation_message(json);
        assert_eq!(
            result.as_deref(),
            Some("Process completed with exit code 1.")
        );
    }

    #[test]
    fn test_extract_annotation_message_empty_annotations() {
        let json = "[]";
        let result = extract_annotation_message(json);
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_annotation_message_no_message_field() {
        let json = r#"[{"annotation_level": "failure"}]"#;
        let result = extract_annotation_message(json);
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_annotation_message_null_message() {
        let json = r#"[{"message": null}]"#;
        let result = extract_annotation_message(json);
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_annotation_message_invalid_json() {
        let result = extract_annotation_message("not json");
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_annotation_message_skips_null_finds_real() {
        let json = r#"[{"message": null}, {"message": "Real error"}]"#;
        let result = extract_annotation_message(json);
        assert_eq!(result.as_deref(), Some("Real error"));
    }
}
