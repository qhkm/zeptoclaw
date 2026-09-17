//! Integration coverage for tool-schema sanitization and argument coercion.
//!
//! These exercise the two real entry points the unit tests stub out: schemas
//! supplied by an external MCP server (which we do not control) reaching a
//! provider request, and model-emitted arguments reaching a tool through
//! `kernel::execute_tool`.

use std::sync::Arc;

use serde_json::{json, Value};

use zeptoclaw::error::Result;
use zeptoclaw::tools::mcp::client::McpClient;
use zeptoclaw::tools::mcp::wrapper::McpToolWrapper;
use zeptoclaw::tools::{Tool, ToolContext, ToolOutput, ToolRegistry};
use zeptoclaw::utils::metrics::MetricsCollector;

const KEY_CHARS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_.-";

/// Collect every `properties` key anywhere in a schema tree.
fn property_keys(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::Object(props)) = map.get("properties") {
                out.extend(props.keys().cloned());
            }
            for v in map.values() {
                property_keys(v, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|v| property_keys(v, out)),
        _ => {}
    }
}

fn key_conforms(k: &str) -> bool {
    !k.is_empty() && k.len() <= 64 && k.chars().all(|c| KEY_CHARS.contains(c))
}

/// An MCP tool wrapper over a client that is never called — only its schema matters.
fn mcp_tool(input_schema: Value) -> McpToolWrapper {
    let client = Arc::new(McpClient::new("srv", "http://127.0.0.1:1/mcp", 1));
    McpToolWrapper::new(
        "srv",
        "remote",
        "an external MCP tool",
        input_schema,
        client,
    )
}

#[test]
fn mcp_schema_with_illegal_property_keys_is_sanitized_for_providers() {
    // The shape a real MCP server ships: keys a strict backend 400s on.
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(mcp_tool(json!({
        "type": "object",
        "properties": {
            "bad key!": { "type": "string" },
            "nested": {
                "type": "object",
                "properties": { "also bad/key": { "type": "integer" } }
            }
        }
    }))));

    let defs = registry.definitions();
    assert_eq!(defs.len(), 1);

    // Assert on the serialized form: this is what reaches the provider body.
    let body = serde_json::to_value(&defs[0]).unwrap();
    let mut keys = Vec::new();
    property_keys(&body, &mut keys);

    assert!(!keys.is_empty(), "expected property keys, got {body}");
    assert!(
        keys.iter().all(|k| key_conforms(k)),
        "non-conforming keys reached the provider body: {keys:?}"
    );
}

#[test]
fn mcp_bare_object_schema_gains_properties_for_grammar_converters() {
    // llama.cpp's GBNF converter fails on an object schema with no `properties`.
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(mcp_tool(json!({ "type": "object" }))));

    let defs = registry.definitions();
    assert_eq!(
        defs[0].parameters["properties"],
        json!({}),
        "bare object schema must gain an empty properties map"
    );
}

#[test]
fn mcp_schema_already_conforming_is_passed_through_unchanged() {
    let schema = json!({
        "type": "object",
        "properties": { "query": { "type": "string", "description": "the query" } },
        "required": ["query"]
    });
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(mcp_tool(schema.clone())));

    assert_eq!(registry.definitions()[0].parameters, schema);
}

/// Reflects the arguments it received so the test can assert what arrived.
struct ReflectTool;

#[async_trait::async_trait]
impl Tool for ReflectTool {
    fn name(&self) -> &str {
        "reflect"
    }

    fn description(&self) -> &str {
        "reflects its arguments"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "max results": { "type": "integer" },
                "verbose": { "type": "boolean" }
            }
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        Ok(ToolOutput::llm_only(args.to_string()))
    }
}

#[tokio::test]
async fn renamed_key_and_string_scalars_round_trip_through_the_kernel() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ReflectTool));

    // What the model was shown: the sanitized key.
    let renamed = registry.definitions()[0].parameters["properties"]
        .as_object()
        .unwrap()
        .keys()
        .find(|k| k.starts_with("max"))
        .cloned()
        .expect("renamed key present");
    assert_eq!(renamed, "max_results");

    // What a small local model emits: renamed key, scalars as strings.
    let output = zeptoclaw::kernel::execute_tool(
        &registry,
        "reflect",
        json!({ "max_results": "42", "verbose": "true" }),
        &ToolContext::default(),
        None,
        &MetricsCollector::new(),
        None,
    )
    .await
    .unwrap();

    let received: Value = serde_json::from_str(&output.for_llm).unwrap();
    assert_eq!(
        received,
        json!({ "max results": 42, "verbose": true }),
        "tool must receive its own wire key and real typed values"
    );
}

#[tokio::test]
async fn well_formed_args_reach_the_tool_untouched() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ReflectTool));

    let output = zeptoclaw::kernel::execute_tool(
        &registry,
        "reflect",
        json!({ "max results": 7, "verbose": false }),
        &ToolContext::default(),
        None,
        &MetricsCollector::new(),
        None,
    )
    .await
    .unwrap();

    let received: Value = serde_json::from_str(&output.for_llm).unwrap();
    assert_eq!(received, json!({ "max results": 7, "verbose": false }));
}
