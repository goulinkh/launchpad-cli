use std::io::Read;

use flate2::read::GzDecoder;
use reqwest::Method;
use serde::Serialize;
use serde_json::{Map, Value, json};
use url::Url;

use crate::auth;
use crate::client::{LaunchpadClient, LpError};
use crate::error::Error;
use crate::result::Result;

#[derive(Serialize)]
pub struct ApiOperation {
    pub operation_id: String,
    pub path: String,
    pub method: String,
    pub definition: Value,
}

pub fn specification() -> Result<Value> {
    let mut bytes = Vec::new();
    GzDecoder::new(&include_bytes!("../openapi/launchpad.json.gz")[..])
        .read_to_end(&mut bytes)
        .map_err(|source| Error::Io {
            path: "embedded OpenAPI".into(),
            source,
        })?;
    serde_json::from_slice(&bytes)
        .map_err(|source| Error::invalid(format!("cannot decode embedded OpenAPI: {source}")))
}

pub fn operations(specification: &Value) -> Vec<ApiOperation> {
    let mut operations = Vec::new();
    if let Some(paths) = specification["paths"].as_object() {
        for (path, item) in paths {
            collect_operations(path, item, &mut operations);
        }
    }
    operations.sort_by(|left, right| left.operation_id.cmp(&right.operation_id));
    operations
}

pub async fn call(
    operation: &ApiOperation,
    input: Map<String, Value>,
    dry_run: bool,
    yes: bool,
) -> Result<Value> {
    for key in input.keys() {
        if !matches!(key.as_str(), "params" | "body") {
            return Err(Error::invalid(format!("unknown API input key: {key}")));
        }
    }
    let params = match input.get("params") {
        Some(Value::Object(params)) => params.clone(),
        Some(_) => return Err(Error::invalid("params must be an object")),
        None => Map::new(),
    };
    let method = Method::from_bytes(operation.method.as_bytes())
        .map_err(|source| Error::invalid(source.to_string()))?;
    let writes = !matches!(method, Method::GET | Method::HEAD | Method::OPTIONS);
    if writes && !yes && !dry_run {
        return Err(Error::invalid("API writes require --yes"));
    }
    let url = operation_url(operation, &params)?;
    let body = input.get("body");
    let content = operation
        .definition
        .pointer("/requestBody/content")
        .and_then(Value::as_object);
    let form =
        content.is_some_and(|content| content.contains_key("application/x-www-form-urlencoded"));
    let json_body = content.is_some_and(|content| content.contains_key("application/json"));
    if body.is_some() && content.is_none() {
        return Err(Error::invalid("this API operation does not accept a body"));
    }
    if content.is_some() && !form && !json_body {
        return Err(Error::invalid(
            "body media type is not supported; use a dedicated Launchpad tool",
        ));
    }
    if operation
        .definition
        .pointer("/requestBody/required")
        .and_then(Value::as_bool)
        == Some(true)
        && body.is_none()
    {
        return Err(Error::invalid("this operation requires body"));
    }
    if let Some(body) = body {
        let media_type = if form {
            "application/x-www-form-urlencoded"
        } else {
            "application/json"
        };
        let schema = &operation.definition["requestBody"]["content"][media_type]["schema"];
        validate_input(schema, body, &specification()?)?;
    }
    if dry_run {
        return Ok(
            json!({ "operation_id": operation.operation_id, "method": operation.method, "url": url.as_str(), "body": body, "effect": if writes { "remote-write" } else { "read" } }),
        );
    }
    let client = client(writes)?;
    let pairs = if form {
        form_pairs(body.unwrap_or(&json!({})))?
    } else {
        Vec::new()
    };
    let references: Vec<_> = pairs
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    Ok(client
        .call(
            method,
            url.as_str(),
            if json_body { body } else { None },
            if form { Some(&references) } else { None },
        )
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

fn collect_operations(path: &str, item: &Value, output: &mut Vec<ApiOperation>) {
    for method in ["get", "post", "put", "patch", "delete", "head", "options"] {
        let definition = &item[method];
        if let Some(id) = definition["operationId"].as_str() {
            output.push(ApiOperation {
                operation_id: id.to_owned(),
                path: path.to_owned(),
                method: method.to_ascii_uppercase(),
                definition: definition.clone(),
            });
        }
    }
    if let Some(alternatives) = item["x-launchpad-route-alternatives"].as_array() {
        for alternative in alternatives {
            if let Some(path) = alternative["path"].as_str() {
                collect_operations(path, &alternative["item"], output);
            }
        }
    }
}

fn operation_url(operation: &ApiOperation, params: &Map<String, Value>) -> Result<Url> {
    let definitions = operation.definition["parameters"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for name in params.keys() {
        if !definitions
            .iter()
            .any(|parameter| parameter["name"].as_str() == Some(name))
        {
            return Err(Error::invalid(format!(
                "unknown parameter {name} for {}",
                operation.operation_id
            )));
        }
    }
    let mut path = operation.path.clone();
    for parameter in &definitions {
        let Some(name) = parameter["name"].as_str() else {
            continue;
        };
        let Some(value) = params.get(name) else {
            if parameter["required"].as_bool() == Some(true) {
                return Err(Error::invalid(format!("API parameter {name} is required")));
            }
            continue;
        };
        validate_input(&parameter["schema"], value, &Value::Null)?;
        if parameter["in"] == "path" {
            let value = scalar(value)?;
            if value == "." || value == ".." {
                return Err(Error::invalid("path segments cannot be dot traversal"));
            }
            let encoded =
                percent_encoding::utf8_percent_encode(&value, percent_encoding::NON_ALPHANUMERIC)
                    .to_string();
            path = path.replace(&format!("{{{name}}}"), &encoded);
        }
    }
    if path.contains('{') {
        return Err(Error::invalid(
            "API path template has unresolved parameters",
        ));
    }
    let raw = format!("{}/{}", auth::api_base_url()?, path.trim_start_matches('/'));
    let mut url = Url::parse(&raw).map_err(|source| Error::Url { url: raw, source })?;
    for parameter in &definitions {
        if parameter["in"] != "query" {
            continue;
        }
        let Some(name) = parameter["name"].as_str() else {
            continue;
        };
        if let Some(value) = params.get(name) {
            if let Value::Array(values) = value {
                for value in values {
                    url.query_pairs_mut().append_pair(name, &scalar(value)?);
                }
            } else {
                url.query_pairs_mut().append_pair(name, &scalar(value)?);
            }
        }
    }
    Ok(url)
}

fn validate_input(schema: &Value, value: &Value, specification: &Value) -> Result<()> {
    if let Some(reference) = schema["$ref"].as_str() {
        let resolved = specification
            .pointer(reference.trim_start_matches('#'))
            .ok_or_else(|| Error::invalid(format!("unresolved schema reference: {reference}")))?;
        return validate_input(resolved, value, specification);
    }
    if value.is_null() && schema["nullable"] == true {
        return Ok(());
    }
    let valid = match schema["type"].as_str() {
        Some("string") => value.is_string(),
        Some("integer") => value.is_i64() || value.is_u64(),
        Some("number") => value.is_number(),
        Some("boolean") => value.is_boolean(),
        Some("array") => value.is_array(),
        Some("object") => value.is_object(),
        None => true,
        Some(_) => false,
    };
    if !valid {
        return Err(Error::invalid(format!(
            "value does not match schema type {}",
            schema["type"]
        )));
    }
    if let Some(choices) = schema["enum"].as_array() {
        if !choices.contains(value) {
            return Err(Error::invalid(format!(
                "value must be one of {}",
                schema["enum"]
            )));
        }
    }
    if let Some(values) = value.as_array() {
        for value in values {
            validate_input(&schema["items"], value, specification)?;
        }
    }
    if let Some(fields) = value.as_object() {
        if let Some(required) = schema["required"].as_array() {
            for name in required.iter().filter_map(Value::as_str) {
                if !fields.contains_key(name) {
                    return Err(Error::invalid(format!("body field {name} is required")));
                }
            }
        }
        for (name, value) in fields {
            if let Some(field) = schema["properties"].get(name) {
                validate_input(field, value, specification)?;
            } else if schema["additionalProperties"] == false {
                return Err(Error::invalid(format!("unknown body field {name}")));
            }
        }
    }
    Ok(())
}

fn form_pairs(body: &Value) -> Result<Vec<(String, String)>> {
    let fields = body
        .as_object()
        .ok_or_else(|| Error::invalid("form body must be an object"))?;
    let mut pairs = Vec::new();
    for (name, value) in fields {
        if let Value::Array(values) = value {
            for value in values {
                pairs.push((name.clone(), scalar(value)?));
            }
        } else {
            pairs.push((name.clone(), scalar(value)?));
        }
    }
    Ok(pairs)
}

fn scalar(value: &Value) -> Result<String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Bool(_) | Value::Number(_) => Ok(value.to_string()),
        _ => Err(Error::invalid(
            "path, query, and form values must be scalar (or arrays for repeated parameters)",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_alternative_routes_and_semantic_ids() {
        let fixture = json!({ "paths": { "/first/{name}": { "get": { "operationId": "first-get" }, "x-launchpad-route-alternatives": [{ "path": "/second/{id}", "item": { "get": { "operationId": "second-get" } } }] } } });
        let operations = operations(&fixture);
        assert_eq!(operations.len(), 2);
        assert_eq!(operations[1].path, "/second/{id}");
        assert_eq!(operations[1].operation_id, "second-get");
    }

    #[test]
    fn converter_snapshot_is_valid_and_types_decode_pages() {
        let spec = specification().unwrap();
        assert!(!operations(&spec).is_empty());
        crate::generated::validate_component("git_ref-page", json!({ "start": 0, "entries": [] }))
            .unwrap();
        assert!(
            crate::generated::validate_component("git_ref-page", json!({ "entries": [] })).is_err()
        );
    }
}
