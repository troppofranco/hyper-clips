//! HTTP access to GitHub. Only plain GET requests with a generic User-Agent.

use crate::release::{newest, parse_release, parse_release_list, Release};
use crate::{Result, UpdateError};
use std::io::Read;
use std::time::Duration;

const MAX_JSON: u64 = 2 * 1024 * 1024;
/// Upper bound for any downloaded file.
pub const MAX_DOWNLOAD: u64 = 200 * 1024 * 1024;

pub struct Client {
    agent: ureq::Agent,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Self {
        let agent = ureq::AgentBuilder::new()
            .user_agent("HyperClips-Updater")
            .timeout_connect(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .build();
        Self { agent }
    }

    fn get_bytes(&self, url: &str, limit: u64, accept: Option<&str>) -> Result<Vec<u8>> {
        let mut request = self.agent.get(url);
        if let Some(accept) = accept {
            request = request.set("Accept", accept);
        }
        let response = request
            .call()
            .map_err(|e| UpdateError::Http(e.to_string()))?;
        let mut body = Vec::new();
        response
            .into_reader()
            .take(limit + 1)
            .read_to_end(&mut body)
            .map_err(|e| UpdateError::Http(e.to_string()))?;
        if body.len() as u64 > limit {
            return Err(UpdateError::Http("response is too large".into()));
        }
        Ok(body)
    }

    /// Newest release for the channel: `releases/latest` for stable, the release list for beta.
    pub fn fetch_newest(&self, owner: &str, repo: &str, beta: bool) -> Result<Option<Release>> {
        let accept = Some("application/vnd.github+json");
        if beta {
            let url = format!("https://api.github.com/repos/{owner}/{repo}/releases?per_page=10");
            let body = self.get_bytes(&url, MAX_JSON, accept)?;
            let text = String::from_utf8_lossy(&body);
            Ok(newest(parse_release_list(&text)?, true))
        } else {
            let url = format!("https://api.github.com/repos/{owner}/{repo}/releases/latest");
            let body = self.get_bytes(&url, MAX_JSON, accept)?;
            Ok(Some(parse_release(&String::from_utf8_lossy(&body))?))
        }
    }

    pub fn download(&self, url: &str) -> Result<Vec<u8>> {
        self.get_bytes(url, MAX_DOWNLOAD, None)
    }

    pub fn download_text(&self, url: &str) -> Result<String> {
        let bytes = self.get_bytes(url, MAX_JSON, None)?;
        String::from_utf8(bytes).map_err(|e| UpdateError::Parse(e.to_string()))
    }
}
