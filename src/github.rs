//! Thin GitHub REST API client for claim-issue operations.
//!
//! Wraps `reqwest` with the small surface the bot actually needs:
//! create + update issues, append comments, manage labels. No
//! `octocrab` dependency — this is a closed surface, octocrab's
//! object model + auth flow would add weight without saving code.
//!
//! Auth: `Authorization: Bearer <token>`. Spec says v0 supports both
//! a fine-grained PAT and a GitHub App; this struct just holds a
//! bearer token so either source works (PAT directly; App-installed
//! installation token via `actions/create-github-app-token` upstream).
//!
//! Repo identity is `owner/repo` strings throughout — matches how the
//! `Config.state_repo` env var is shaped, lets the bot operate on
//! multiple repos in the future without re-plumbing.

use anyhow::{Context, Result, bail};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use serde::{Deserialize, Serialize};

const GITHUB_API: &str = "https://api.github.com";

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    token: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub labels: Vec<Label>,
    pub html_url: String,
    pub state: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Label {
    Object {
        name: String,
    },
    /// PATCH /labels takes plain strings on the wire.
    Name(String),
}

impl Label {
    pub fn name(&self) -> &str {
        match self {
            Label::Object { name } => name,
            Label::Name(n) => n,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Comment {
    pub id: u64,
    pub body: String,
    pub html_url: String,
}

impl Client {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            token: token.into(),
        }
    }

    fn headers(&self) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", self.token))
                .expect("bearer header is ASCII"),
        );
        h.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        h.insert(USER_AGENT, HeaderValue::from_static("satsforcompute"));
        h.insert(
            "X-GitHub-Api-Version",
            HeaderValue::from_static("2022-11-28"),
        );
        h
    }

    fn url(&self, path: &str) -> String {
        format!("{GITHUB_API}{path}")
    }

    /// `GET /repos/{owner}/{repo}/issues?labels=l1,l2&state=open` —
    /// list open issues matching ALL the given labels (GitHub `&`s
    /// the filter). Used by the lifecycle orchestrator to find
    /// claims in a specific state for the next transition.
    ///
    /// Pulls one page (default 30 issues) — sufficient for an
    /// operator-scale claim volume; pagination is a follow-up if
    /// the queue ever grows past it.
    pub async fn list_open_issues_by_labels(
        &self,
        repo: &str,
        labels: &[&str],
    ) -> Result<Vec<Issue>> {
        let labels_csv: String = labels.join(",");
        let url = self.url(&format!(
            "/repos/{repo}/issues?state=open&per_page=100&labels={labels_csv}"
        ));
        let resp = self
            .http
            .get(&url)
            .headers(self.headers())
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        if !resp.status().is_success() {
            let s = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("GET {url} → {s}: {body}");
        }
        // GitHub returns a flat array of issue+PR objects. Filter
        // out PRs (they show up in /issues unless the search API
        // is used; PRs have a `pull_request` field). The Issue
        // deserializer ignores unknown fields by default but we
        // still skip PRs to keep the orchestrator focused.
        let raw: Vec<serde_json::Value> = resp.json().await?;
        let issues: Vec<Issue> = raw
            .into_iter()
            .filter(|v| v.get("pull_request").is_none())
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect();
        Ok(issues)
    }

    /// `GET /repos/{owner}/{repo}/issues/{number}`. Used by claim
    /// loaders to fetch the canonical manifest body.
    pub async fn get_issue(&self, repo: &str, number: u64) -> Result<Issue> {
        let url = self.url(&format!("/repos/{repo}/issues/{number}"));
        let resp = self
            .http
            .get(&url)
            .headers(self.headers())
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        if !resp.status().is_success() {
            let s = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("GET {url} → {s}: {body}");
        }
        Ok(resp.json().await?)
    }

    /// `POST /repos/{owner}/{repo}/issues` — create a new issue with
    /// the given title, body, and labels. Used by `claim.create`.
    pub async fn create_issue(
        &self,
        repo: &str,
        title: &str,
        body: &str,
        labels: &[&str],
    ) -> Result<Issue> {
        #[derive(Serialize)]
        struct Req<'a> {
            title: &'a str,
            body: &'a str,
            labels: &'a [&'a str],
        }
        let url = self.url(&format!("/repos/{repo}/issues"));
        let resp = self
            .http
            .post(&url)
            .headers(self.headers())
            .json(&Req {
                title,
                body,
                labels,
            })
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        if !resp.status().is_success() {
            let s = resp.status();
            let txt = resp.text().await.unwrap_or_default();
            bail!("POST {url} → {s}: {txt}");
        }
        Ok(resp.json().await?)
    }

    /// `PATCH /repos/{owner}/{repo}/issues/{number}` — update the
    /// body with a freshly-rendered manifest. The bot calls this on
    /// every state transition. Body-only by default; pass labels via
    /// the dedicated label endpoints to avoid clobbering ones the
    /// operator added by hand.
    pub async fn update_issue_body(&self, repo: &str, number: u64, body: &str) -> Result<Issue> {
        #[derive(Serialize)]
        struct Req<'a> {
            body: &'a str,
        }
        let url = self.url(&format!("/repos/{repo}/issues/{number}"));
        let resp = self
            .http
            .patch(&url)
            .headers(self.headers())
            .json(&Req { body })
            .send()
            .await
            .with_context(|| format!("PATCH {url}"))?;
        if !resp.status().is_success() {
            let s = resp.status();
            let txt = resp.text().await.unwrap_or_default();
            bail!("PATCH {url} → {s}: {txt}");
        }
        Ok(resp.json().await?)
    }

    /// `POST /repos/{owner}/{repo}/issues/{number}/comments` —
    /// append-only event log entry on the claim issue.
    pub async fn add_comment(&self, repo: &str, number: u64, body: &str) -> Result<Comment> {
        #[derive(Serialize)]
        struct Req<'a> {
            body: &'a str,
        }
        let url = self.url(&format!("/repos/{repo}/issues/{number}/comments"));
        let resp = self
            .http
            .post(&url)
            .headers(self.headers())
            .json(&Req { body })
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        if !resp.status().is_success() {
            let s = resp.status();
            let txt = resp.text().await.unwrap_or_default();
            bail!("POST {url} → {s}: {txt}");
        }
        Ok(resp.json().await?)
    }

    /// `POST /repos/{owner}/{repo}/issues/{number}/labels` — add the
    /// labels (idempotent, GitHub dedups). Used by state-transition
    /// helpers that flip `state:pending-payment` → `state:active`,
    /// etc. Caller should remove the old state label separately.
    pub async fn add_labels(&self, repo: &str, number: u64, labels: &[&str]) -> Result<()> {
        #[derive(Serialize)]
        struct Req<'a> {
            labels: &'a [&'a str],
        }
        let url = self.url(&format!("/repos/{repo}/issues/{number}/labels"));
        let resp = self
            .http
            .post(&url)
            .headers(self.headers())
            .json(&Req { labels })
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        if !resp.status().is_success() {
            let s = resp.status();
            let txt = resp.text().await.unwrap_or_default();
            bail!("POST {url} → {s}: {txt}");
        }
        Ok(())
    }

    /// `POST /repos/{owner}/{repo}/actions/workflows/{file}/dispatches` —
    /// fire a `workflow_dispatch` event with typed inputs. 204 No
    /// Content on success; the API never returns the run ID, so
    /// callers track which run "owns" the dispatch via the inputs they
    /// embedded (e.g. claim_id) and a follow-up listing if needed.
    ///
    /// `inputs` keys must match the workflow file's `inputs:` block.
    /// All values are sent as strings — that's how `workflow_dispatch`
    /// inputs work on the wire even when the workflow declares them as
    /// boolean/number; the receiving workflow coerces.
    pub async fn dispatch_workflow(
        &self,
        repo: &str,
        workflow_filename: &str,
        ref_name: &str,
        inputs: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<()> {
        #[derive(Serialize)]
        struct Req<'a> {
            #[serde(rename = "ref")]
            ref_: &'a str,
            inputs: &'a serde_json::Map<String, serde_json::Value>,
        }
        let url = self.url(&format!(
            "/repos/{repo}/actions/workflows/{workflow_filename}/dispatches"
        ));
        let resp = self
            .http
            .post(&url)
            .headers(self.headers())
            .json(&Req {
                ref_: ref_name,
                inputs,
            })
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        if !resp.status().is_success() {
            let s = resp.status();
            let txt = resp.text().await.unwrap_or_default();
            bail!("POST {url} → {s}: {txt}");
        }
        Ok(())
    }

    /// `DELETE /repos/{owner}/{repo}/issues/{number}/labels/{name}` —
    /// remove a single label. 404 is treated as success (GitHub
    /// returns it when the label isn't present).
    pub async fn remove_label(&self, repo: &str, number: u64, label: &str) -> Result<()> {
        // URL-encode the label name in case it has spaces/colons.
        let encoded: String = label
            .chars()
            .flat_map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
                    vec![c]
                } else {
                    format!("%{:02X}", c as u32).chars().collect()
                }
            })
            .collect();
        let url = self.url(&format!("/repos/{repo}/issues/{number}/labels/{encoded}"));
        let resp = self
            .http
            .delete(&url)
            .headers(self.headers())
            .send()
            .await
            .with_context(|| format!("DELETE {url}"))?;
        let s = resp.status();
        if s.is_success() || s.as_u16() == 404 {
            return Ok(());
        }
        let txt = resp.text().await.unwrap_or_default();
        bail!("DELETE {url} → {s}: {txt}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_struct_form_deserializes() {
        let j = r#"{"name":"state:active"}"#;
        let l: Label = serde_json::from_str(j).unwrap();
        assert_eq!(l.name(), "state:active");
    }

    #[test]
    fn label_string_form_deserializes() {
        let j = r#""state:active""#;
        let l: Label = serde_json::from_str(j).unwrap();
        assert_eq!(l.name(), "state:active");
    }

    #[test]
    fn url_helper_concats_against_api_root() {
        let c = Client::new("ignored");
        assert_eq!(
            c.url("/repos/foo/bar/issues/1"),
            "https://api.github.com/repos/foo/bar/issues/1"
        );
    }

    #[test]
    fn issue_with_object_labels_deserializes() {
        let j = r#"{
            "number": 7,
            "title": "claim_x",
            "body": "...",
            "labels": [{"name":"state:active"}, {"name":"integrity:tainted"}],
            "html_url": "https://github.com/o/r/issues/7",
            "state": "open"
        }"#;
        let i: Issue = serde_json::from_str(j).unwrap();
        assert_eq!(i.number, 7);
        assert_eq!(i.labels.len(), 2);
        assert_eq!(i.labels[0].name(), "state:active");
    }
}
