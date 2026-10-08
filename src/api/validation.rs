use std::collections::BTreeSet;

use openapi_schema_to_json_schema::{Options, from_schema};
use serde_json::{Map, Value, json};

use crate::error::Error;
use crate::result::Result;

use super::contract::{Contract, contract_error};

#[derive(Copy, Clone)]
pub(super) enum Direction {
    Request,
    Response,
}

impl Contract {
    /// Bundle components without inlining: recursive and shared references stay intact.
    pub(super) fn validation_schema(&self, schema: Value, direction: Direction) -> Result<Value> {
        let components = referenced_components(&schema, &self.document)?;
        let document = json!({
            "allOf": [schema],
            "components": { "schemas": components },
        });
        let options = Options::new()
            .definition_keywords(vec!["components.schemas".to_owned()])
            .remove_read_only(matches!(direction, Direction::Request))
            .remove_write_only(matches!(direction, Direction::Response));
        from_schema(document, &options).map_err(contract_error)
    }

    pub fn validate_component(&self, name: &str, value: &Value) -> Result<()> {
        self.component(name)?;
        let name = name.replace('~', "~0").replace('/', "~1");
        let schema = self.validation_schema(
            json!({ "$ref": format!("#/components/schemas/{name}") }),
            Direction::Response,
        )?;
        validate(&schema, value)?;
        Ok(())
    }
}

/// Copy only reachable components so operation discovery does not dump the whole API.
/// This is a dependency walk, not reference evaluation; the validator evaluates refs.
fn referenced_components(schema: &Value, document: &Value) -> Result<Map<String, Value>> {
    let mut pending = vec![schema];
    let mut seen = BTreeSet::new();
    let mut components = Map::new();
    while let Some(value) = pending.pop() {
        match value {
            Value::Object(fields) => {
                if let Some(reference) = fields.get("$ref").and_then(Value::as_str) {
                    if seen.insert(reference) {
                        let fragment = reference.strip_prefix('#').ok_or_else(|| {
                            contract_error("external schema references are not supported")
                        })?;
                        let pointer = percent_encoding::percent_decode_str(fragment)
                            .decode_utf8()
                            .map_err(contract_error)?;
                        let target = document.pointer(&pointer).ok_or_else(|| {
                            contract_error(format!("unresolved schema reference: {reference}"))
                        })?;
                        let name = pointer
                            .strip_prefix("/components/schemas/")
                            .filter(|name| !name.contains('/'))
                            .ok_or_else(|| {
                                contract_error("only component schema references are supported")
                            })?;
                        let name = name.replace("~1", "/").replace("~0", "~");
                        components.insert(name, target.clone());
                        pending.push(target);
                    }
                }
                // Walk schema-bearing keywords only, never example/default JSON data.
                for keyword in [
                    "items",
                    "additionalProperties",
                    "not",
                    "allOf",
                    "anyOf",
                    "oneOf",
                ] {
                    if let Some(schema) = fields.get(keyword) {
                        pending.push(schema);
                    }
                }
                if let Some(properties) = fields.get("properties").and_then(Value::as_object) {
                    pending.extend(properties.values());
                }
            }
            Value::Array(schemas) => pending.extend(schemas),
            _ => {}
        }
    }
    Ok(components)
}

/// Delegate constraints and reference evaluation to the JSON Schema validator.
pub(super) fn validate(schema: &Value, value: &Value) -> Result<()> {
    // File and HTTP reference resolution are disabled in Cargo.toml. Validation
    // must never read files or make network calls, including during a dry run.
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft4)
        .should_validate_formats(false)
        .build(schema)
        .map_err(contract_error)?;
    if let Some(error) = validator.iter_errors(value).next() {
        return Err(Error::SchemaValidation {
            instance_path: error.instance_path().to_string(),
            schema_path: error.schema_path().to_string(),
            reason: error.to_string(),
        });
    }
    Ok(())
}
