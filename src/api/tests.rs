use serde_json::{Value, json};

use super::contract::Contract;
use super::validation::validate;

const BASE: &str = "http://127.0.0.1:1/devel";

fn contract(paths: Value, components: Value) -> Contract {
    Contract::from_document(json!({
        "openapi": "3.0.3",
        "info": { "title": "Test API", "version": "1" },
        "paths": paths,
        "components": components,
    }))
    .unwrap()
}

#[test]
fn snapshot_parses_and_all_components_compile() {
    let contract = Contract::embedded().unwrap();
    assert_eq!(contract.operations.len(), 4154);
    for name in contract.document["components"]["schemas"]
        .as_object()
        .unwrap()
        .keys()
    {
        let result = contract.validate_component(name, &Value::Null);
        assert!(
            !matches!(result, Err(crate::error::Error::ApiContract { .. })),
            "{name}: {result:?}"
        );
    }
    contract
        .validate_component("git_ref-page", &json!({ "start": 0, "entries": [] }))
        .unwrap();
    assert!(
        contract
            .validate_component("git_ref-page", &json!({ "entries": [] }))
            .is_err()
    );
}

#[test]
fn preserves_alternative_routes_and_rejects_ambiguous_ids() {
    let contract = contract(
        json!({
            "/first": {
                "get": { "operationId": "first", "responses": {} },
                "x-launchpad-route-alternatives": [
                    { "path": "/second", "item": { "get": { "operationId": "second", "responses": {} } } },
                    { "path": "/third", "item": { "get": { "operationId": "first", "responses": {} } } }
                ]
            }
        }),
        json!({}),
    );
    assert_eq!(contract.operations.len(), 3);
    assert_eq!(contract.operation("second").unwrap().path, "/second");
    assert!(contract.operation("first").is_err());
    assert!(contract.operation("invented").is_err());
}

#[test]
fn resolves_path_parameter_and_body_refs_with_operation_overrides() {
    let contract = contract(
        json!({
            "/things/{id}": {
                "parameters": [{ "$ref": "#/components/parameters/Id" }],
                "post": {
                    "operationId": "create", "responses": {},
                    "parameters": [{ "name": "id", "in": "path", "required": true,
                        "schema": { "$ref": "#/components/schemas/Id" } }],
                    "requestBody": { "$ref": "#/components/requestBodies/Create" }
                }
            },
            "/alias/{id}": { "$ref": "#/paths/~1things~1{id}" }
        }),
        json!({
            "parameters": { "Id": { "name": "id", "in": "path", "required": true, "schema": { "type": "integer" } } },
            "schemas": { "Id": { "type": "string", "pattern": "^[a-z]+$" } },
            "requestBodies": { "Create": { "required": true, "content": { "application/json": {
                "schema": { "type": "object", "required": ["title"], "properties": { "title": { "type": "string" } } }
            } } } }
        }),
    );
    let operation = &contract.operations[0];
    assert_eq!(operation.definition.parameters.len(), 1);
    let input = json!({ "params": { "id": "abc" }, "body": { "title": "Title" } });
    let plan = contract
        .prepare(operation, input.as_object().unwrap().clone(), BASE)
        .unwrap();
    assert!(plan.url.path().ends_with("/abc"));
    assert_eq!(plan.content_type, Some("application/json"));
    for input in [
        json!({}),
        json!({ "params": { "id": 1 }, "body": {} }),
        json!({ "params": { "id": "BAD" }, "body": {} }),
    ] {
        assert!(
            contract
                .prepare(operation, input.as_object().unwrap().clone(), BASE)
                .is_err()
        );
    }
}

#[test]
fn schema_validation_handles_constraints_composition_and_recursive_refs() {
    let contract = contract(
        json!({}),
        json!({ "schemas": {
        "Node": {
            "type": "object", "additionalProperties": false, "required": ["name"],
            "properties": {
                "name": { "type": "string", "minLength": 2, "pattern": "^[a-z]+$" },
                "count": { "type": "integer", "minimum": 1, "maximum": 5 },
                "choice": { "oneOf": [{ "type": "string" }, { "type": "integer" }] },
                "labels": { "type": "array", "minItems": 1, "uniqueItems": true, "items": { "type": "string" } },
                "metadata": { "type": "object", "additionalProperties": { "type": "integer" } },
                "nullable": { "type": "string", "nullable": true },
                "children": { "type": "array", "items": { "$ref": "#/components/schemas/Node" } }
            }
        }
    } }),
    );
    contract
        .validate_component(
            "Node",
            &json!({ "name": "ok", "nullable": null, "children": [{ "name": "child" }] }),
        )
        .unwrap();
    for invalid in [
        json!({}),
        json!({ "name": "X" }),
        json!({ "name": null }),
        json!({ "name": "ok", "count": 6 }),
        json!({ "name": "ok", "unknown": true }),
        json!({ "name": "ok", "choice": false }),
        json!({ "name": "ok", "labels": ["x", "x"] }),
        json!({ "name": "ok", "metadata": { "key": "not a number" } }),
        json!({ "name": "ok", "children": [{ "name": "X" }] }),
    ] {
        assert!(
            contract.validate_component("Node", &invalid).is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn validation_is_offline_and_rejects_external_references() {
    for reference in ["https://example.invalid/schema", "file:///etc/passwd"] {
        assert!(validate(&json!({ "$ref": reference }), &json!({})).is_err());
    }
    let contract = contract(
        json!({}),
        json!({ "schemas": { "Broken": { "$ref": "#/components/schemas/Missing" } } }),
    );
    assert!(contract.validate_component("Broken", &Value::Null).is_err());
}

#[test]
fn query_arrays_obey_explode_and_cannot_override_fixed_operations() {
    let contract = contract(
        json!({
            "/things?ws.op=find": { "get": { "operationId": "find", "responses": {}, "parameters": [
                { "name": "tag", "in": "query", "schema": { "type": "array", "items": { "type": "string" } } },
                { "name": "ids", "in": "query", "explode": false, "schema": { "type": "array", "items": { "type": "integer" } } },
                { "name": "ws.op", "in": "query", "schema": { "type": "string" } }
            ] } }
        }),
        json!({}),
    );
    let operation = contract.operation("find").unwrap();
    let input = json!({ "params": { "tag": ["a&b", "c"], "ids": [1, 2] } });
    let plan = contract
        .prepare(operation, input.as_object().unwrap().clone(), BASE)
        .unwrap();
    assert_eq!(
        plan.url.query(),
        Some("ws.op=find&tag=a%26b&tag=c&ids=1%2C2")
    );
    let input = json!({ "params": { "ws.op": "delete" } });
    assert!(
        contract
            .prepare(operation, input.as_object().unwrap().clone(), BASE)
            .is_err()
    );
}

#[test]
fn dry_run_rejects_unsupported_serialisation_before_execution() {
    let contract = contract(
        json!({
            "/things": { "post": { "operationId": "create", "responses": {}, "requestBody": {
                "content": { "application/x-www-form-urlencoded": { "schema": { "type": "object" } } }
            } } },
            "/headers": { "get": { "operationId": "headers", "responses": {}, "parameters": [
                { "name": "custom", "in": "header", "schema": { "type": "string" } }
            ] } },
            "/multipart": { "post": { "operationId": "upload", "responses": {}, "requestBody": {
                "content": { "multipart/form-data": {} }
            } } }
        }),
        json!({}),
    );
    let input = json!({ "body": { "nested": { "key": "value" } } });
    assert!(
        contract
            .prepare(
                contract.operation("create").unwrap(),
                input.as_object().unwrap().clone(),
                BASE
            )
            .is_err()
    );
    for id in ["headers", "upload"] {
        let operation = contract.operation(id).unwrap();
        assert!(contract.describe(operation).unwrap()["unsupported_reason"].is_string());
        assert!(
            contract
                .prepare(operation, Default::default(), BASE)
                .is_err()
        );
    }
}

#[test]
fn read_only_and_write_only_required_fields_respect_direction() {
    let contract = contract(
        json!({
            "/things": { "post": { "operationId": "create", "responses": {}, "requestBody": {
                "required": true,
                "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Thing" } } }
            } } }
        }),
        json!({ "schemas": {
        "Thing": { "type": "object", "required": ["id", "secret"], "properties": {
            "id": { "type": "integer", "readOnly": true },
            "secret": { "type": "string", "writeOnly": true }
        } }
    } }),
    );
    let input = json!({ "body": { "secret": "value" } });
    let operation = contract.operation("create").unwrap();
    contract
        .prepare(operation, input.as_object().unwrap().clone(), BASE)
        .unwrap();
    contract
        .validate_component("Thing", &json!({ "id": 1 }))
        .unwrap();
    assert!(contract.validate_component("Thing", &json!({})).is_err());
    assert!(
        contract
            .prepare(
                operation,
                json!({ "body": {} }).as_object().unwrap().clone(),
                BASE
            )
            .is_err()
    );
}

#[test]
fn descriptions_bundle_only_reachable_schemas_and_ignore_default_data() {
    let contract = contract(
        json!({
            "/things": { "post": { "operationId": "create", "responses": {}, "requestBody": {
                "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Parent" } } }
            } } }
        }),
        json!({ "schemas": {
        "Parent": { "type": "object", "properties": {
            "child": { "$ref": "#/components/schemas/a~1b~0c" },
            "data": { "type": "object", "default": { "$ref": "not-a-schema-reference" } }
        } },
        "a/b~c": { "type": "array", "items": { "$ref": "#/components/schemas/Parent" } },
        "Unrelated": { "type": "string" }
    } }),
    );
    let description = contract
        .describe(contract.operation("create").unwrap())
        .unwrap();
    let schemas = description["input_schema"]["components"]["schemas"]
        .as_object()
        .unwrap();
    assert_eq!(schemas.len(), 2);
    assert!(schemas.contains_key("Parent"));
    assert!(schemas.contains_key("a/b~c"));
    assert_eq!(description["requires_yes"], true);
    validate(
        &description["input_schema"],
        &json!({ "body": { "child": [] } }),
    )
    .unwrap();
}

#[test]
fn named_posts_put_the_fixed_selector_in_form_data() {
    let contract = contract(
        json!({
            "/things?ws.op=new": { "post": { "operationId": "create", "responses": {},
                "requestBody": { "content": { "application/x-www-form-urlencoded": {
                    "schema": { "type": "object" }
                } } }
            } },
            "/things?ws.op=refresh": { "post": { "operationId": "refresh", "responses": {} } }
        }),
        json!({}),
    );
    let operation = contract.operation("create").unwrap();
    let input = json!({ "body": { "name": "test" } });
    let plan = contract
        .prepare(operation, input.as_object().unwrap().clone(), BASE)
        .unwrap();
    assert_eq!(plan.body.as_ref().unwrap()["ws.op"], "new");
    assert!(
        plan.form_pairs
            .unwrap()
            .contains(&("ws.op".into(), "new".into()))
    );
    let input = json!({ "body": { "ws.op": "delete" } });
    assert!(
        contract
            .prepare(operation, input.as_object().unwrap().clone(), BASE)
            .is_err()
    );
    let plan = contract
        .prepare(
            contract.operation("refresh").unwrap(),
            Default::default(),
            BASE,
        )
        .unwrap();
    assert_eq!(plan.content_type, Some("application/x-www-form-urlencoded"));
    assert_eq!(plan.body.unwrap(), json!({ "ws.op": "refresh" }));
}

#[test]
fn path_values_cannot_change_routes_and_forms_preserve_repeated_fields() {
    let contract = contract(
        json!({
            "/things/{id}": { "post": { "operationId": "create", "responses": {},
                "parameters": [{ "name": "id", "in": "path", "required": true, "schema": { "type": "string" } }],
                "requestBody": { "required": true, "content": { "application/x-www-form-urlencoded": {
                    "schema": { "type": "object", "properties": { "tags": { "type": "array", "items": { "type": "string" } } } }
                } } }
            } }
        }),
        json!({}),
    );
    let operation = contract.operation("create").unwrap();
    for id in [".", "..", ""] {
        let input = json!({ "params": { "id": id }, "body": {} });
        assert!(
            contract
                .prepare(operation, input.as_object().unwrap().clone(), BASE)
                .is_err()
        );
    }
    let input = json!({ "params": { "id": "a/b?x=1" }, "body": { "tags": ["one", "two"] } });
    let plan = contract
        .prepare(operation, input.as_object().unwrap().clone(), BASE)
        .unwrap();
    assert_eq!(plan.url.path(), "/devel/things/a%2Fb%3Fx%3D1");
    assert!(plan.url.query().is_none());
    assert_eq!(
        plan.form_pairs.unwrap(),
        vec![("tags".into(), "one".into()), ("tags".into(), "two".into())]
    );
}

#[test]
fn malformed_and_cyclic_contract_references_do_not_panic() {
    for document in [
        json!({ "paths": {} }),
        json!({
            "openapi": "3.0.3", "info": { "title": "Test", "version": "1" },
            "paths": { "/loop": { "$ref": "#/paths/~1loop" } }
        }),
    ] {
        assert!(Contract::from_document(document).is_err());
    }
}
