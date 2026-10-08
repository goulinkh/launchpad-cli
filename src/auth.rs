use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

use crate::client::{ApiResult, LpError};

const OAUTH_ENCODE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    pub consumer_key: String,
    pub token: String,
    pub secret: String,
}

impl Credentials {
    pub fn header(&self) -> ApiResult<String> {
        self.validate()?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|source| LpError::Config(source.to_string()))?
            .as_secs();
        let nonce: u128 = rand::random();
        let signature = encode(&format!("&{}", encode(&self.secret)));
        Ok(format!(
            "OAuth oauth_consumer_key=\"{}\", oauth_token=\"{}\", oauth_signature_method=\"PLAINTEXT\", oauth_timestamp=\"{timestamp}\", oauth_nonce=\"{nonce:032x}\", oauth_version=\"1.0\", oauth_signature=\"{signature}\"",
            encode(&self.consumer_key),
            encode(&self.token),
        ))
    }

    fn validate(&self) -> ApiResult<()> {
        if self.consumer_key.is_empty() || self.token.is_empty() || self.secret.is_empty() {
            return Err(LpError::Config(
                "credential fields cannot be empty".to_owned(),
            ));
        }
        Ok(())
    }
}

pub fn api_base_url() -> ApiResult<String> {
    let instance = env::var("LAUNCHPAD_CLI_INSTANCE").unwrap_or_else(|_| "production".to_owned());
    let host = match instance.as_str() {
        "production" => "api.launchpad.net",
        "staging" => "api.staging.launchpad.net",
        "qastaging" => "api.qastaging.launchpad.net",
        "development" => "api.launchpad.test",
        _ => {
            return Err(LpError::Config(format!(
                "unknown Launchpad instance: {instance}"
            )));
        }
    };
    let base =
        env::var("LAUNCHPAD_CLI_API_BASE").unwrap_or_else(|_| format!("https://{host}/devel"));
    let url = Url::parse(base.trim_end_matches('/'))
        .map_err(|source| LpError::Config(source.to_string()))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(LpError::Config(
            "API base must be an HTTP(S) URL without credentials, query, or fragment".to_owned(),
        ));
    }
    Ok(url.to_string().trim_end_matches('/').to_owned())
}

pub fn git_host() -> ApiResult<&'static str> {
    let base =
        Url::parse(&api_base_url()?).map_err(|source| LpError::Config(source.to_string()))?;
    match base.host_str() {
        Some("api.launchpad.net") => Ok("git.launchpad.net"),
        Some("api.staging.launchpad.net") => Ok("git.staging.launchpad.net"),
        Some("api.qastaging.launchpad.net") => Ok("git.qastaging.launchpad.net"),
        Some("api.launchpad.test") => Ok("git.launchpad.test"),
        _ => Err(LpError::Config(
            "cannot infer a Git host from this API base; use a local checkout".to_owned(),
        )),
    }
}

pub fn credentials_path() -> ApiResult<PathBuf> {
    if let Some(path) = env::var_os("LAUNCHPAD_CLI_CREDENTIALS") {
        return Ok(PathBuf::from(path));
    }
    let config = dirs::config_dir()
        .ok_or_else(|| LpError::Config("cannot determine config directory".to_owned()))?;
    let base =
        Url::parse(&api_base_url()?).map_err(|source| LpError::Config(source.to_string()))?;
    let profile = encode(&base.origin().ascii_serialization());
    Ok(config.join("launchpad-cli").join(format!("{profile}.json")))
}

pub fn load_credentials() -> ApiResult<Credentials> {
    load(&credentials_path()?)
}

pub fn save_credentials(credentials: &Credentials) -> ApiResult<()> {
    credentials.validate()?;
    save(&credentials_path()?, credentials)?;
    Ok(())
}

pub fn logout() -> ApiResult<Value> {
    let path = credentials_path()?;
    remove(&path)?;
    remove(&path.with_extension("pending.json"))?;
    Ok(json!({ "authenticated": false }))
}

pub fn status() -> ApiResult<Value> {
    let authenticated = match load_credentials() {
        Ok(_) => true,
        Err(LpError::NotAuthenticated) => false,
        Err(source) => return Err(source),
    };
    Ok(
        json!({ "authenticated": authenticated, "api_base": api_base_url()?, "credentials_file": credentials_path()?, "verified": false }),
    )
}

pub async fn login_start() -> ApiResult<Value> {
    let origin = oauth_origin()?;
    let body = oauth_post(
        &format!("{origin}/+request-token"),
        &[
            ("oauth_consumer_key", "launchpad-cli"),
            ("oauth_signature_method", "PLAINTEXT"),
            ("oauth_signature", "&"),
        ],
    )
    .await?;
    let credentials = parse_token(&body)?;
    save(
        &credentials_path()?.with_extension("pending.json"),
        &credentials,
    )?;
    Ok(
        json!({ "authorization_url": format!("{origin}/+authorize-token?oauth_token={}", encode(&credentials.token)), "next": "launchpad-cli auth login --finish" }),
    )
}

pub async fn login_finish() -> ApiResult<Value> {
    let path = credentials_path()?.with_extension("pending.json");
    let pending = load(&path)?;
    let signature = format!("&{}", encode(&pending.secret));
    let body = oauth_post(
        &format!("{}/+access-token", oauth_origin()?),
        &[
            ("oauth_consumer_key", &pending.consumer_key),
            ("oauth_token", &pending.token),
            ("oauth_signature_method", "PLAINTEXT"),
            ("oauth_signature", &signature),
        ],
    )
    .await?;
    let credentials = parse_token(&body)?;
    save_credentials(&credentials)?;
    remove(&path)?;
    status()
}

fn oauth_origin() -> ApiResult<&'static str> {
    let base =
        Url::parse(&api_base_url()?).map_err(|source| LpError::Config(source.to_string()))?;
    match base.host_str() {
        Some("api.launchpad.net") if base.scheme() == "https" => Ok("https://launchpad.net"),
        Some("api.staging.launchpad.net") if base.scheme() == "https" => {
            Ok("https://staging.launchpad.net")
        }
        Some("api.qastaging.launchpad.net") if base.scheme() == "https" => {
            Ok("https://qastaging.launchpad.net")
        }
        Some("api.launchpad.test") if base.scheme() == "https" => Ok("https://launchpad.test"),
        _ => Err(LpError::Config(
            "browser login requires a recognised Launchpad HTTPS instance".to_owned(),
        )),
    }
}

async fn oauth_post(url: &str, pairs: &[(&str, &str)]) -> ApiResult<String> {
    let response = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(30))
        .build()?
        .post(url)
        .form(pairs)
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        return Err(LpError::Api {
            status: status.as_u16(),
            message: body,
        });
    }
    Ok(body)
}

fn parse_token(body: &str) -> ApiResult<Credentials> {
    let pairs: std::collections::HashMap<_, _> = url::form_urlencoded::parse(body.as_bytes())
        .into_owned()
        .collect();
    let credentials = Credentials {
        consumer_key: "launchpad-cli".to_owned(),
        token: pairs
            .get("oauth_token")
            .cloned()
            .ok_or_else(|| LpError::Config("OAuth response has no token".to_owned()))?,
        secret: pairs
            .get("oauth_token_secret")
            .cloned()
            .ok_or_else(|| LpError::Config("OAuth response has no token secret".to_owned()))?,
    };
    credentials.validate()?;
    Ok(credentials)
}

fn load(path: &PathBuf) -> ApiResult<Credentials> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Err(LpError::NotAuthenticated);
        }
        Err(source) => {
            return Err(LpError::Config(format!(
                "cannot read credentials: {source}"
            )));
        }
    };
    let credentials: Credentials = serde_json::from_slice(&bytes)?;
    credentials.validate()?;
    Ok(credentials)
}

fn save(path: &PathBuf, credentials: &Credentials) -> ApiResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| LpError::Config(source.to_string()))?;
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let temporary = path.with_extension(format!("{:032x}.tmp", rand::random::<u128>()));
    let result = (|| {
        let mut file = options
            .open(&temporary)
            .map_err(|source| LpError::Config(source.to_string()))?;
        file.write_all(&serde_json::to_vec(credentials)?)
            .map_err(|source| LpError::Config(source.to_string()))?;
        file.sync_all()
            .map_err(|source| LpError::Config(source.to_string()))?;
        fs::rename(&temporary, path).map_err(|source| LpError::Config(source.to_string()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn remove(path: &PathBuf) -> ApiResult<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(LpError::Config(source.to_string())),
    }
}

fn encode(value: &str) -> String {
    utf8_percent_encode(value, OAUTH_ENCODE).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_signature_encodes_secret_twice() {
        let credentials = Credentials {
            consumer_key: "test".to_owned(),
            token: "token".to_owned(),
            secret: "a&b".to_owned(),
        };
        assert!(
            credentials
                .header()
                .unwrap()
                .contains("oauth_signature=\"%26a%2526b\"")
        );
    }

    #[test]
    fn saves_private_credentials() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("credentials.json");
        let credentials = parse_token("oauth_token=t&oauth_token_secret=s").unwrap();
        save(&path, &credentials).unwrap();
        assert_eq!(load(&path).unwrap().token, "t");
        save(&path, &credentials).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
