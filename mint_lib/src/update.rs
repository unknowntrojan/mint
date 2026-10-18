use crate::error::GenericError;
use crate::error::ResultExt;

/// GitHub repo that update checks and self-updates come from.
pub const GITHUB_REPO: &str = "unknowntrojan/mint";

#[derive(Debug, serde::Deserialize)]
pub struct GitHubRelease {
    pub html_url: String,
    pub tag_name: String,
    pub body: String,
}

pub async fn get_latest_release() -> Result<GitHubRelease, GenericError> {
    reqwest::Client::builder()
        .user_agent(GITHUB_REPO)
        .build()
        .generic("failed to construct reqwest client".to_string())?
        .get(format!(
            "https://api.github.com/repos/{GITHUB_REPO}/releases/latest"
        ))
        .send()
        .await
        .generic("check self update request failed".to_string())?
        .json::<GitHubRelease>()
        .await
        .generic("check self update response is error".to_string())
}
