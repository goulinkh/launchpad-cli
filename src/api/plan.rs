use std::collections::BTreeSet;

use openapiv3::{
    MediaType, Parameter, ParameterSchemaOrContent, PathStyle, QueryStyle, ReferenceOr,
};
use serde::Serialize;
use serde_json::{Map, Value, json};
use url::Url;

use crate::error::Error;
use crate::result::Result;

use super::contract::{ApiOperation, Contract, contract_error};
use super::validation::{Direction, validate};

const FORM: &str = "application/x-www-form-urlencoded";
const JSON: &str = "application/json";

/// A fully validated request, prepared without credentials, network or Git access.
#[derive(Debug, Serialize)]
pub struct RequestPlan {
    pub operation_id: String,
    pub method: String,
    pub url: Url,
    pub body: Option<Value>,
    pub content_type: Option<&'static str>,
    pub effect: &'static str,
    #[serde(skip)]
    pub form_pairs: Option<Vec<(String, String)>>,
}

impl Contract {
    pub fn describe(&self, operation: &ApiOperation) -> Result<Value> {
        let (input_schema, unsupported_reason) = match self.input_schema(operation) {
            Ok(schema) => (Some(schema), None),
            Err(Error::InvalidRequest { reason }) => (None, Some(reason)),
            Err(error) => return Err(error),
        };
        Ok(json!({
            "operation": operation,
            "effect": operation.effect(),
            "input_schema": input_schema,
            "unsupported_reason": unsupported_reason,
            "invocation": format!("launchpad-cli api call {} --input FILE|-", operation.operation_id),
            "requires_yes": operation.effect() != "read",
        }))
    }

    pub fn prepare(
        &self,
        operation: &ApiOperation,
        input: Map<String, Value>,
        base_url: &str,
    ) -> Result<RequestPlan> {
        let schema = self.input_schema(operation)?;
        validate(&schema, &Value::Object(input.clone()))?;
        let parameters = parameters(operation)?;
        let params = input
            .get("params")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let url = operation_url(operation, &parameters, &params, base_url)?;
        let mut body = input.get("body").cloned();
        let mut content_type = body_media(operation)?.map(|(name, _)| name);
        // Launchpad dispatches named POST operations from form data, not the URL
        // query used to distinguish these operations in the converter document.
        if operation.method == "POST" {
            if let Some((_, name)) = url.query_pairs().find(|(key, _)| key == "ws.op") {
                if content_type.is_some_and(|content_type| content_type != FORM) {
                    return Err(Error::invalid(
                        "named POST operations require form encoding",
                    ));
                }
                let fields = body
                    .get_or_insert_with(|| json!({}))
                    .as_object_mut()
                    .ok_or_else(|| Error::invalid("form body must be an object"))?;
                if fields.contains_key("ws.op") {
                    return Err(Error::invalid("cannot override fixed form parameter ws.op"));
                }
                fields.insert("ws.op".to_owned(), json!(name));
                content_type = Some(FORM);
            }
        }
        let form_pairs = if content_type == Some(FORM) {
            body.as_ref().map(form_pairs).transpose()?
        } else {
            None
        };
        Ok(RequestPlan {
            operation_id: operation.operation_id.clone(),
            method: operation.method.clone(),
            url,
            body,
            content_type,
            effect: operation.effect(),
            form_pairs,
        })
    }

    fn input_schema(&self, operation: &ApiOperation) -> Result<Value> {
        if operation.method == "TRACE" {
            return Err(Error::invalid("TRACE is not supported by api call"));
        }
        let mut properties = Map::new();
        let mut required = Vec::new();
        for parameter in parameters(operation)? {
            check_serialisation(parameter)?;
            let data = parameter.parameter_data_ref();
            let ParameterSchemaOrContent::Schema(schema) = &data.format else {
                return Err(Error::invalid(
                    "parameter content encoding is not supported",
                ));
            };
            if properties.contains_key(&data.name) {
                return Err(Error::invalid(format!(
                    "parameter {} appears in multiple locations; flat params would be ambiguous",
                    data.name
                )));
            }
            properties.insert(
                data.name.clone(),
                serde_json::to_value(schema).map_err(contract_error)?,
            );
            if data.required {
                required.push(data.name.clone());
            }
        }
        let mut input_required = Vec::new();
        if !required.is_empty() {
            input_required.push("params");
        }
        let mut params =
            json!({ "type": "object", "properties": properties, "additionalProperties": false });
        if !required.is_empty() {
            params["required"] = json!(required);
        }
        let mut properties = json!({ "params": params });
        if let Some((_, media)) = body_media(operation)? {
            properties["body"] = match &media.schema {
                Some(schema) => serde_json::to_value(schema).map_err(contract_error)?,
                None => json!({}),
            };
            if operation
                .definition
                .request_body
                .as_ref()
                .and_then(ReferenceOr::as_item)
                .is_some_and(|body| body.required)
            {
                input_required.push("body");
            }
        }
        let mut schema =
            json!({ "type": "object", "properties": properties, "additionalProperties": false });
        if !input_required.is_empty() {
            schema["required"] = json!(input_required);
        }
        self.validation_schema(schema, Direction::Request)
    }
}

fn parameters(operation: &ApiOperation) -> Result<Vec<&Parameter>> {
    operation
        .definition
        .parameters
        .iter()
        .map(|parameter| {
            parameter
                .as_item()
                .ok_or_else(|| contract_error("unresolved parameter"))
        })
        .collect()
}

fn body_media(operation: &ApiOperation) -> Result<Option<(&'static str, &MediaType)>> {
    let Some(body) = &operation.definition.request_body else {
        return Ok(None);
    };
    let body = body
        .as_item()
        .ok_or_else(|| contract_error("unresolved request body"))?;
    for name in [FORM, JSON] {
        if let Some(media) = body.content.get(name) {
            if !media.encoding.is_empty() {
                return Err(Error::invalid("custom body encoding is not supported"));
            }
            return Ok(Some((name, media)));
        }
    }
    Err(Error::invalid(
        "body media type is not supported; use a dedicated Launchpad tool",
    ))
}

fn check_serialisation(parameter: &Parameter) -> Result<()> {
    match parameter {
        Parameter::Path {
            style: PathStyle::Simple,
            ..
        }
        | Parameter::Query {
            style: QueryStyle::Form,
            allow_reserved: false,
            ..
        } => Ok(()),
        _ => Err(Error::invalid(format!(
            "unsupported location or serialisation for parameter {}",
            parameter.parameter_data_ref().name
        ))),
    }
}

fn operation_url(
    operation: &ApiOperation,
    parameters: &[&Parameter],
    params: &Map<String, Value>,
    base_url: &str,
) -> Result<Url> {
    let mut path = operation.path.clone();
    for parameter in parameters {
        if let Parameter::Path { parameter_data, .. } = parameter {
            if let Some(value) = params.get(&parameter_data.name) {
                let value = scalar(value)?;
                if matches!(value.as_str(), "" | "." | "..") {
                    return Err(Error::invalid(
                        "path segments cannot be empty or dot traversal",
                    ));
                }
                let encoded = percent_encoding::utf8_percent_encode(
                    &value,
                    percent_encoding::NON_ALPHANUMERIC,
                );
                path = path.replace(
                    &format!("{{{}}}", parameter_data.name),
                    &encoded.to_string(),
                );
            }
        }
    }
    if path.contains(['{', '}']) {
        return Err(Error::invalid(
            "API path template has unresolved parameters",
        ));
    }
    let raw = format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        path.trim_start_matches('/')
    );
    let mut url = Url::parse(&raw).map_err(|source| Error::Url { url: raw, source })?;
    let fixed: BTreeSet<_> = url
        .query_pairs()
        .map(|(name, _)| name.into_owned())
        .collect();
    for parameter in parameters {
        if let Parameter::Query { parameter_data, .. } = parameter {
            let name = &parameter_data.name;
            if let Some(value) = params.get(name) {
                if fixed.contains(name) {
                    return Err(Error::invalid(format!(
                        "cannot override fixed query parameter {name}"
                    )));
                }
                let values = scalar_values(value)?;
                if parameter_data.explode == Some(false) && value.is_array() {
                    url.query_pairs_mut().append_pair(name, &values.join(","));
                } else {
                    for value in values {
                        url.query_pairs_mut().append_pair(name, &value);
                    }
                }
            }
        }
    }
    Ok(url)
}

fn form_pairs(body: &Value) -> Result<Vec<(String, String)>> {
    let fields = body
        .as_object()
        .ok_or_else(|| Error::invalid("form body must be an object"))?;
    let mut pairs = Vec::new();
    for (name, value) in fields {
        for value in scalar_values(value)? {
            pairs.push((name.clone(), value));
        }
    }
    Ok(pairs)
}

fn scalar_values(value: &Value) -> Result<Vec<String>> {
    match value {
        Value::Array(values) => values.iter().map(scalar).collect(),
        _ => Ok(vec![scalar(value)?]),
    }
}

fn scalar(value: &Value) -> Result<String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Bool(_) | Value::Number(_) => Ok(value.to_string()),
        _ => Err(Error::invalid(
            "path, query and form values must be scalar (or arrays for query and form)",
        )),
    }
}
