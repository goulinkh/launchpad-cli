use std::io::Read;

use flate2::read::GzDecoder;
use openapiv3::{OpenAPI, Operation, Parameter, PathItem, ReferenceOr};
use openapiv3_resolve::{ResolveOptionalWithOpenAPI, ResolveWithOpenAPI};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::Error;
use crate::result::Result;

/// The converter document is the authority; typed views are used for execution.
pub struct Contract {
    pub document: Value,
    pub operations: Vec<ApiOperation>,
}

impl Contract {
    pub fn embedded() -> Result<Self> {
        let mut bytes = Vec::new();
        GzDecoder::new(&include_bytes!("../../openapi/launchpad.json.gz")[..])
            .read_to_end(&mut bytes)
            .map_err(|source| Error::Io {
                path: "embedded OpenAPI".into(),
                source,
            })?;
        let document = serde_json::from_slice(&bytes).map_err(contract_error)?;
        Self::from_document(document)
    }

    pub fn from_document(document: Value) -> Result<Self> {
        let specification: OpenAPI =
            serde_json::from_value(document.clone()).map_err(contract_error)?;
        if !specification.openapi.starts_with("3.0.") {
            return Err(contract_error("only OpenAPI 3.0 is supported"));
        }
        let mut operations = Vec::new();
        for (path, item) in specification.paths.iter() {
            let item = item.resolve(&specification).map_err(contract_error)?;
            collect_operations(&specification, path, item, &mut operations)?;
        }
        operations.sort_by(|left, right| left.operation_id.cmp(&right.operation_id));
        Ok(Self {
            document,
            operations,
        })
    }

    pub fn operation(&self, id: &str) -> Result<&ApiOperation> {
        let mut candidates = self
            .operations
            .iter()
            .filter(|operation| operation.operation_id == id);
        let operation = candidates.next().ok_or_else(|| {
            Error::invalid(format!(
                "unknown converter operation ID {id}; use api operations"
            ))
        })?;
        if candidates.next().is_some() {
            return Err(contract_error(format!("ambiguous operation ID {id}")));
        }
        Ok(operation)
    }

    pub fn component(&self, name: &str) -> Result<&Value> {
        self.document["components"]["schemas"]
            .get(name)
            .ok_or_else(|| Error::invalid(format!("unknown component schema {name}")))
    }
}

#[derive(Debug, Serialize)]
pub struct ApiOperation {
    pub operation_id: String,
    pub path: String,
    pub method: String,
    /// Includes resolved parameter and request-body references and inherited parameters.
    pub definition: Operation,
}

impl ApiOperation {
    pub fn effect(&self) -> &'static str {
        match self.method.as_str() {
            "GET" | "HEAD" | "OPTIONS" => "read",
            _ => "remote-write",
        }
    }

    pub fn summary(&self) -> Value {
        json!({
            "operation_id": self.operation_id,
            "method": self.method,
            "path": self.path,
            "summary": self.definition.summary,
            "tags": self.definition.tags,
            "effect": self.effect(),
        })
    }
}

/// Only the Launchpad extension is interpreted locally, not the OpenAPI grammar.
fn collect_operations(
    specification: &OpenAPI,
    path: &str,
    item: &PathItem,
    output: &mut Vec<ApiOperation>,
) -> Result<()> {
    for (method, operation) in item.iter() {
        let Some(operation_id) = &operation.operation_id else {
            continue;
        };
        let mut definition = operation.clone();
        definition.parameters = effective_parameters(specification, item, operation)?;
        definition.request_body = operation
            .request_body
            .resolve_optional(specification)
            .map_err(contract_error)?
            .cloned()
            .map(ReferenceOr::Item);
        output.push(ApiOperation {
            operation_id: operation_id.clone(),
            path: path.to_owned(),
            method: method.to_ascii_uppercase(),
            definition,
        });
    }
    if let Some(alternatives) = item.extensions.get("x-launchpad-route-alternatives") {
        let alternatives: Vec<RouteAlternative> =
            serde_json::from_value(alternatives.clone()).map_err(contract_error)?;
        for alternative in alternatives {
            collect_operations(specification, &alternative.path, &alternative.item, output)?;
        }
    }
    Ok(())
}

fn effective_parameters(
    specification: &OpenAPI,
    item: &PathItem,
    operation: &Operation,
) -> Result<Vec<ReferenceOr<Parameter>>> {
    let mut parameters: Vec<Parameter> = Vec::new();
    for reference in item.parameters.iter().chain(&operation.parameters) {
        let parameter = reference.resolve(specification).map_err(contract_error)?;
        let existing = parameters.iter_mut().find(|existing| {
            existing.parameter_data_ref().name == parameter.parameter_data_ref().name
                && std::mem::discriminant(*existing) == std::mem::discriminant(parameter)
        });
        if let Some(existing) = existing {
            *existing = parameter.clone();
        } else {
            parameters.push(parameter.clone());
        }
    }
    Ok(parameters.into_iter().map(ReferenceOr::Item).collect())
}

#[derive(Deserialize)]
struct RouteAlternative {
    path: String,
    item: PathItem,
}

pub(super) fn contract_error(reason: impl std::fmt::Display) -> Error {
    Error::ApiContract {
        reason: reason.to_string(),
    }
}
