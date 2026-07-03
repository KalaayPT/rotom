//! Online resolution of historical **v1** `*_scrcmd_database.json` from
//! [`scrcmd-database`](https://github.com/DS-Pokemon-Rom-Editor/scrcmd-database) using GitHub’s API.
//!
//! Picks the newest commit on the repo’s **default branch** that touched the file whose commit
//! time is still **at or before** the DSPRE export baseline (see `docs/dspre-onboarding-migration-plan.md` Phase 1).
//! The commits request uses the API's `until` filter so normal runs need one REST request instead of
//! walking commit-list pages.

use chrono::{DateTime, Local, Utc};
use serde::Deserialize;
use uxie::GameFamily;

use super::error::{ProjectError, Result};

const GITHUB_API: &str = "https://api.github.com";
const SCRCMD_REPO: &str = "DS-Pokemon-Rom-Editor/scrcmd-database";
const COMMITS_PER_PAGE: usize = 1;

/// Resolved vanilla v1 database JSON and the commit it came from (for logging and comparison with edited project v1).
#[derive(Debug, Clone)]
pub struct VanillaScrcmdV1 {
    pub json: String,
    /// Full 40-character SHA.
    pub commit_sha: String,
    /// Path in the repo (e.g. `platinum_scrcmd_database.json`).
    pub repo_path: &'static str,
}

/// Base name of the v1 JSON file in scrcmd-database for this game family.
pub const fn scrcmd_v1_repo_filename(family: GameFamily) -> &'static str {
    match family {
        GameFamily::Platinum => "platinum_scrcmd_database.json",
        GameFamily::HGSS => "hgss_scrcmd_database.json",
        GameFamily::DP => "diamond_pearl_scrcmd_database.json",
    }
}

/// v1 script DB for **Following Platinum** (DSPRE extended opcode set), relative to repo root — fetched at the same commit SHA as `platinum_scrcmd_database.json`.
pub const FOLLOWING_PLATINUM_SCRCMD_V1_REPO_PATH: &str =
    "custom_databases/following_platinum_scrcmd_database.json";

/// Fetches `custom_databases/following_platinum_scrcmd_database.json` at `commit_sha` (same commit
/// as `platinum_scrcmd_database.json`). Returns `Ok(None)` on 404 or invalid JSON.
pub fn fetch_following_platinum_scrcmd_v1_at_commit(
    commit_sha: &str,
) -> Result<Option<VanillaScrcmdV1>> {
    let Some(raw) =
        try_fetch_repo_file_at_commit(FOLLOWING_PLATINUM_SCRCMD_V1_REPO_PATH, commit_sha)?
    else {
        return Ok(None);
    };
    if verify_json_object(&raw).is_err() {
        return Ok(None);
    }
    Ok(Some(VanillaScrcmdV1 {
        json: raw,
        commit_sha: commit_sha.to_owned(),
        repo_path: FOLLOWING_PLATINUM_SCRCMD_V1_REPO_PATH,
    }))
}

/// Fetches `repo_path` at the newest commit on the default branch whose committer date is ≤
/// `baseline_local` (compared in UTC). Requires network; returns [`ProjectError::ScrcmdBaseline`] on failure.
pub fn fetch_vanilla_scrcmd_v1_at_baseline(
    family: GameFamily,
    baseline_local: DateTime<Local>,
) -> Result<VanillaScrcmdV1> {
    let repo_path = scrcmd_v1_repo_filename(family);

    let baseline_utc = baseline_local.with_timezone(&Utc);
    let sha = resolve_commit_sha_at_or_before(repo_path, baseline_utc)?;
    let json = fetch_repo_file_at_commit(repo_path, &sha)?;
    verify_json_object(&json)?;

    Ok(VanillaScrcmdV1 {
        json,
        commit_sha: sha,
        repo_path,
    })
}

/// Resolves the latest default-branch commit for `path` whose commit date is not after `baseline_utc`.
pub fn resolve_commit_sha_at_or_before(path: &str, baseline_utc: DateTime<Utc>) -> Result<String> {
    let user_agent = format!("rotom/{}", env!("CARGO_PKG_VERSION"));
    let url = github_commits_url_for_baseline(path, baseline_utc);
    let body = http_get_github_api(&url, &user_agent)?;
    let commits: Vec<GhListCommit> =
        serde_json::from_str(&body).map_err(|e| ProjectError::ScrcmdBaseline {
            message: format!("failed to parse GitHub commits JSON for '{path}': {e}"),
        })?;

    let Some(commit) = commits.first() else {
        return Err(ProjectError::ScrcmdBaseline {
            message: format!(
                "GitHub returned no commits for path '{path}' in {SCRCMD_REPO} at or before {}",
                baseline_utc.format("%Y-%m-%d %H:%M:%S UTC")
            ),
        });
    };

    let commit_time = parse_github_commit_time(commit)?;
    if commit_time <= baseline_utc {
        return Ok(commit.sha.clone());
    }

    Err(ProjectError::ScrcmdBaseline {
        message: format!(
            "no commit on the default branch for '{path}' at or before {}",
            baseline_utc.format("%Y-%m-%d %H:%M:%S UTC")
        ),
    })
}

/// Builds the commits URL used to ask for only the newest commit at or before the baseline.
fn github_commits_url_for_baseline(path: &str, baseline_utc: DateTime<Utc>) -> String {
    let until = baseline_utc.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    format!(
        "{GITHUB_API}/repos/{SCRCMD_REPO}/commits?path={path}&per_page={COMMITS_PER_PAGE}&until={until}"
    )
}

fn parse_github_commit_time(c: &GhListCommit) -> Result<DateTime<Utc>> {
    let raw = c
        .commit
        .committer
        .as_ref()
        .and_then(|p| p.date.as_deref())
        .or_else(|| c.commit.author.as_ref().and_then(|p| p.date.as_deref()))
        .ok_or_else(|| ProjectError::ScrcmdBaseline {
            message: "GitHub commit missing author/committer date".to_string(),
        })?;
    DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| ProjectError::ScrcmdBaseline {
            message: format!("invalid commit date from GitHub API '{raw}': {e}"),
        })
}

pub fn fetch_repo_file_at_commit(path: &str, sha: &str) -> Result<String> {
    let url = format!("https://raw.githubusercontent.com/{SCRCMD_REPO}/{sha}/{path}");
    let user_agent = format!("rotom/{}", env!("CARGO_PKG_VERSION"));
    http_get_raw(&url, &user_agent)
}

/// Like [`fetch_repo_file_at_commit`], but returns `Ok(None)` when the object is missing at `sha` (HTTP 404).
pub fn try_fetch_repo_file_at_commit(path: &str, sha: &str) -> Result<Option<String>> {
    let url = format!("https://raw.githubusercontent.com/{SCRCMD_REPO}/{sha}/{path}");
    let user_agent = format!("rotom/{}", env!("CARGO_PKG_VERSION"));
    let response = minreq::get(&url)
        .with_header("User-Agent", user_agent)
        .with_timeout(60)
        .send()
        .map_err(|e| ProjectError::ScrcmdBaseline {
            message: format!("HTTP GET failed for {url}: {e}"),
        })?;

    let status = response.status_code;
    if status == 404 {
        return Ok(None);
    }
    if !(200..300).contains(&status) {
        let body = response
            .as_str()
            .unwrap_or("")
            .chars()
            .take(500)
            .collect::<String>();
        return Err(ProjectError::ScrcmdBaseline {
            message: format!("HTTP {status} for {url}: {body}"),
        });
    }
    Ok(Some(response.as_str().unwrap_or("").to_string()))
}

/// Fetches REST API JSON, using `GITHUB_TOKEN` or `GH_TOKEN` when available for higher limits.
fn http_get_github_api(url: &str, user_agent: &str) -> Result<String> {
    let mut request = minreq::get(url)
        .with_header("User-Agent", user_agent)
        .with_header("Accept", "application/vnd.github+json")
        .with_timeout(60);
    if let Some(token) = std::env::var("GITHUB_TOKEN")
        .ok()
        .or_else(|| std::env::var("GH_TOKEN").ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
    {
        request = request.with_header("Authorization", format!("Bearer {token}"));
    }
    let response = request.send().map_err(|e| ProjectError::ScrcmdBaseline {
        message: format!("HTTP GET failed for {url}: {e}"),
    })?;

    check_status(&response, url)?;

    Ok(response.as_str().unwrap_or("").to_string())
}

fn http_get_raw(url: &str, user_agent: &str) -> Result<String> {
    let response = minreq::get(url)
        .with_header("User-Agent", user_agent)
        .with_timeout(60)
        .send()
        .map_err(|e| ProjectError::ScrcmdBaseline {
            message: format!("HTTP GET failed for {url}: {e}"),
        })?;

    check_status(&response, url)?;

    Ok(response.as_str().unwrap_or("").to_string())
}

fn check_status(response: &minreq::Response, url: &str) -> Result<()> {
    let status = response.status_code;
    if !(200..300).contains(&status) {
        let body = response
            .as_str()
            .unwrap_or("")
            .chars()
            .take(500)
            .collect::<String>();
        return Err(ProjectError::ScrcmdBaseline {
            message: format!("HTTP {status} for {url}: {body}"),
        });
    }
    Ok(())
}

pub fn verify_json_object(raw: &str) -> Result<()> {
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| ProjectError::ScrcmdBaseline {
            message: format!("fetched file text is not valid JSON: {e}"),
        })?;
    if !v.is_object() {
        return Err(ProjectError::ScrcmdBaseline {
            message: "fetched scrcmd JSON root is not an object".to_string(),
        });
    }
    Ok(())
}

#[derive(Deserialize)]
struct GhListCommit {
    sha: String,
    commit: GhCommitBody,
}

#[derive(Deserialize)]
struct GhCommitBody {
    #[serde(default)]
    committer: Option<GhPerson>,
    #[serde(default)]
    author: Option<GhPerson>,
}

#[derive(Deserialize)]
struct GhPerson {
    #[serde(default)]
    date: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn pick_sha_at_or_before_baseline(
        commits: &[(String, DateTime<Utc>)],
        baseline_utc: DateTime<Utc>,
    ) -> Option<&str> {
        for (sha, t) in commits {
            if *t <= baseline_utc {
                return Some(sha.as_str());
            }
        }
        None
    }

    #[test]
    fn github_commits_url_uses_until_filter_and_single_result() {
        let baseline = Utc.with_ymd_and_hms(2026, 5, 9, 15, 45, 1).unwrap();
        let url = github_commits_url_for_baseline("platinum_scrcmd_database.json", baseline);

        assert_eq!(
            url,
            "https://api.github.com/repos/DS-Pokemon-Rom-Editor/scrcmd-database/commits?path=platinum_scrcmd_database.json&per_page=1&until=2026-05-09T15:45:01Z"
        );
    }

    #[test]
    fn pick_sha_takes_first_commit_not_after_baseline_newest_first() {
        let commits = vec![
            (
                "aaa".to_string(),
                Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            ),
            (
                "bbb".to_string(),
                Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap(),
            ),
        ];
        let baseline = Utc.with_ymd_and_hms(2025, 12, 1, 0, 0, 0).unwrap();
        assert_eq!(
            pick_sha_at_or_before_baseline(&commits, baseline),
            Some("bbb")
        );
    }

    #[test]
    fn pick_sha_none_if_all_newer_than_baseline() {
        let commits = vec![(
            "aaa".to_string(),
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        )];
        let baseline = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(pick_sha_at_or_before_baseline(&commits, baseline), None);
    }

    #[test]
    #[ignore = "requires network to api.github.com and raw.githubusercontent.com"]
    fn fetch_vanilla_platinum_smoke() {
        use crate::GameFamily;
        use chrono::Local;

        let v =
            fetch_vanilla_scrcmd_v1_at_baseline(GameFamily::Platinum, Local::now()).expect("fetch");
        assert!(v.json.len() > 10_000);
        assert!(v.commit_sha.len() >= 7);
    }
}
