use reqwest::Method;
use serde_json::{Map, Value, json};

use crate::auth;
use crate::client::{LaunchpadClient, LpError};
use crate::error::Error;
use crate::result::Result;

use super::contract::{ApiOperation, Contract, contract_error};

pub async fn call(
    contract: &Contract,
    operation: &ApiOperation,
    input: Map<String, Value>,
    dry_run: bool,
    yes: bool,
) -> Result<Value> {
    let plan = contract.prepare(operation, input, &auth::api_base_url()?)?;
    if dry_run {
        let mut output = serde_json::to_value(&plan).map_err(contract_error)?;
        output["executed"] = json!(false);
        return Ok(output);
    }
    let writes = plan.effect != "read";
    if writes && !yes {
        return Err(Error::invalid(
            "API writes require --yes; inspect first with --dry-run",
        ));
    }
    let method = Method::from_bytes(plan.method.as_bytes()).map_err(contract_error)?;
    let references = plan.form_pairs.as_ref().map(|pairs| {
        pairs
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect::<Vec<_>>()
    });
    let body = if plan.content_type == Some("application/json") {
        plan.body.as_ref()
    } else {
        None
    };
    let client = client(writes)?;
    Ok(client
        .call(method, plan.url.as_str(), body, references.as_deref())
        .await?)
}

pub fn client(require_authentication: bool) -> Result<LaunchpadClient> {
    let anonymous = std::env::var("LAUNCHPAD_CLI_ANONYMOUS").is_ok_and(|value| value == "1");
    let credentials = if anonymous {
        if require_authentication {
            return Err(LpError::NotAuthenticated.into());
        }
        None
    } else {
        match auth::load_credentials() {
            Ok(credentials) => Some(credentials),
            Err(LpError::NotAuthenticated) if !require_authentication => None,
            Err(source) => return Err(source.into()),
        }
    };
    Ok(LaunchpadClient::new(credentials).with_base_url(auth::api_base_url()?))
}
