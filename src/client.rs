use std::collections::HashSet;
use std::time::Duration;

use reqwest::{Client, Method, Response, StatusCode, header::LOCATION};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use thiserror::Error;
use url::Url;

use crate::auth::Credentials;

#[derive(Debug, Error)]
pub enum LpError {
    #[error("cannot authenticate; run launchpad-cli auth login")]
    NotAuthenticated,
    #[error("cannot find resource: {0}")]
    NotFound(String),
    #[error("cannot complete Launchpad request: HTTP {status}: {message}")]
    Api { status: u16, message: String },
    #[error("cannot reach Launchpad: {0}")]
    Connect(reqwest::Error),
    #[error("cannot complete Launchpad request before timeout: {0}")]
    Timeout(reqwest::Error),
    #[error("cannot send Launchpad request: {0}")]
    Http(reqwest::Error),
    #[error("cannot decode Launchpad response: {0}")]
    Json(#[from] serde_json::Error),
    #[error("cannot configure Launchpad client: {0}")]
    Config(String),
}

impl From<reqwest::Error> for LpError {
    fn from(source: reqwest::Error) -> Self {
        if source.is_timeout() {
            Self::Timeout(source.without_url())
        } else if source.is_connect() {
            Self::Connect(source.without_url())
        } else {
            Self::Http(source.without_url())
        }
    }
}

pub type ApiResult<T> = std::result::Result<T, LpError>;

pub struct LaunchpadClient {
    http: std::result::Result<Client, reqwest::Error>,
    credentials: Option<Credentials>,
    base_url: String,
}

impl LaunchpadClient {
    pub fn new(credentials: Option<Credentials>) -> Self {
        let http = Client::builder()
            .user_agent(concat!("launchpad-cli/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(30))
            // Hypermedia and Location links are checked before sending any credentials.
            .redirect(reqwest::redirect::Policy::none())
            .build();
        Self {
            http,
            credentials,
            base_url: "https://api.launchpad.net/devel".to_owned(),
        }
    }

    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url.trim_end_matches('/').to_owned();
        self
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path.trim_start_matches('/'))
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> ApiResult<T> {
        self.get_url(&self.url(path)).await
    }

    pub async fn get_url<T: DeserializeOwned>(&self, url: &str) -> ApiResult<T> {
        let response = self.send(Method::GET, url, None, None).await?;
        let bytes = response.bytes().await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub async fn patch_url_with_value<T: DeserializeOwned>(
        &self,
        url: &str,
        body: &Value,
    ) -> ApiResult<T> {
        let response = self.send(Method::PATCH, url, Some(body), None).await?;
        let bytes = response.bytes().await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub async fn post_pairs_url_ok(&self, url: &str, pairs: &[(&str, &str)]) -> ApiResult<()> {
        self.send(Method::POST, url, None, Some(pairs)).await?;
        Ok(())
    }

    pub async fn post_pairs_created_location(
        &self,
        path: &str,
        pairs: &[(&str, &str)],
    ) -> ApiResult<String> {
        self.post_pairs_url_created_location(&self.url(path), pairs)
            .await
    }

    pub async fn post_pairs_url_created_location(
        &self,
        url: &str,
        pairs: &[(&str, &str)],
    ) -> ApiResult<String> {
        let response = self.send(Method::POST, url, None, Some(pairs)).await?;
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| LpError::Config("created response has no Location header".to_owned()))?;
        let resource = Url::parse(url)
            .and_then(|base| base.join(location))
            .map_err(|source| LpError::Config(source.to_string()))?;
        self.check_url(resource.as_str())?;
        Ok(resource.to_string())
    }

    pub async fn call(
        &self,
        method: Method,
        url: &str,
        json: Option<&Value>,
        pairs: Option<&[(&str, &str)]>,
    ) -> ApiResult<Value> {
        let response = self.send(method, url, json, pairs).await?;
        let status = response.status().as_u16();
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let bytes = response.bytes().await?;
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)?
        };
        Ok(serde_json::json!({ "status": status, "location": location, "body": body }))
    }

    pub fn check_url(&self, url: &str) -> ApiResult<()> {
        let base =
            Url::parse(&self.base_url).map_err(|source| LpError::Config(source.to_string()))?;
        let target = Url::parse(url).map_err(|source| LpError::Config(source.to_string()))?;
        let base_path = base.path().trim_end_matches('/');
        if target.origin() != base.origin()
            || !target.username().is_empty()
            || target.password().is_some()
            || !(target.path() == base_path || target.path().starts_with(&format!("{base_path}/")))
        {
            return Err(LpError::Config(
                "refusing API link outside the configured origin and version".to_owned(),
            ));
        }
        if self.credentials.is_some() && target.scheme() != "https" {
            return Err(LpError::Config(
                "refusing to send OAuth credentials without HTTPS".to_owned(),
            ));
        }
        Ok(())
    }

    async fn send(
        &self,
        method: Method,
        url: &str,
        json: Option<&Value>,
        pairs: Option<&[(&str, &str)]>,
    ) -> ApiResult<Response> {
        self.check_url(url)?;
        let http = self
            .http
            .as_ref()
            .map_err(|source| LpError::Config(source.to_string()))?;
        let mut request = http
            .request(method, url)
            .header("Accept", "application/json");
        if let Some(credentials) = &self.credentials {
            request = request.header("Authorization", credentials.header()?);
        }
        if let Some(body) = json {
            request = request.json(body);
        }
        if let Some(pairs) = pairs {
            request = request.form(pairs);
        }
        // Do not retry mutations: a timeout may have occurred after a successful write.
        let response = request.send().await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Err(LpError::NotFound(url.to_owned()));
        }
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response.text().await?;
            return Err(LpError::Api { status, message });
        }
        Ok(response)
    }
}

// A transport page, not an alternative definition of a Launchpad domain model.
#[derive(Debug, Deserialize)]
pub struct Collection<T> {
    pub entries: Vec<T>,
    pub next_collection_link: Option<String>,
}

#[derive(Debug)]
pub struct GitRef {
    pub path: Option<String>,
    pub self_link: Option<String>,
    pub commit_sha1: Option<String>,
}

/// Project hypermedia refs into the strings needed by local Git workflows.
pub async fn list_git_refs(client: &LaunchpadClient, repository: &str) -> ApiResult<Vec<GitRef>> {
    let mut url = client.url(&format!("/{repository}/refs"));
    let mut seen = HashSet::new();
    let mut refs = Vec::new();
    loop {
        if !seen.insert(url.clone()) {
            return Err(LpError::Config("collection repeated a page".to_owned()));
        }
        let page: Collection<Value> = client.get_url(&url).await?;
        refs.extend(page.entries.into_iter().map(|entry| {
            GitRef {
                path: entry.get("path").and_then(Value::as_str).map(str::to_owned),
                self_link: entry
                    .get("self_link")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                commit_sha1: entry
                    .get("commit_sha1")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            }
        }));
        match page.next_collection_link {
            Some(next) => url = next,
            None => break,
        }
    }
    Ok(refs)
}

pub fn urlenc(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confines_links_to_api_origin_and_version() {
        let client = LaunchpadClient::new(None);
        assert!(
            client
                .check_url("https://api.launchpad.net/devel/bugs/1")
                .is_ok()
        );
        for url in [
            "https://evil.example/devel/bugs/1",
            "https://api.launchpad.net/1.0/bugs/1",
            "https://api.launchpad.net/devel-other/bugs/1",
            "https://user@api.launchpad.net/devel/bugs/1",
        ] {
            assert!(client.check_url(url).is_err(), "{url}");
        }
    }
}
