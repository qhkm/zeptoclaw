//! Sanitize tool JSON schemas for strict LLM backends.
//!
//! llama.cpp's GBNF grammar converter fails on `{"type": "object"}` without
//! `properties`, on bare-string schemas and on `type` arrays; Anthropic (and
//! Bedrock/Vertex/Azure fronting it) rejects property keys outside
//! `^[a-zA-Z0-9_.-]{1,64}$` and nullable `anyOf` at the top of `input_schema`;
//! some backends reject `default` beside `$ref` or top-level combinators.
//!
//! Only those shapes are rewritten, on a deep copy. Renamed property keys are
//! reversed on the way back in by [`unrename_tool_args`].

use serde_json::{Map, Value};

/// Longest property key a strict backend accepts.
const MAX_KEY_LEN: usize = 64;

/// Keys whose value is a map of name -> schema.
const SCHEMA_MAP_KEYS: &[&str] = &[
    "properties",
    "$defs",
    "definitions",
    "patternProperties",
    "dependentSchemas",
];

/// Keys whose value is a single nested schema.
const SCHEMA_CHILD_KEYS: &[&str] = &[
    "items",
    "additionalProperties",
    "contains",
    "propertyNames",
    "if",
    "then",
    "else",
    "not",
];

/// Keys whose value is a list of schemas.
const SCHEMA_LIST_KEYS: &[&str] = &["allOf", "anyOf", "oneOf", "prefixItems"];

/// Combinators a strict backend rejects at the top of a tool's parameter schema.
const TOP_LEVEL_FORBIDDEN: &[&str] = &["allOf", "anyOf", "oneOf", "enum", "not"];

/// Siblings strict validators reject beside `$ref`.
const REF_FORBIDDEN_SIBLINGS: &[&str] = &["default"];

/// Map an arbitrary property key to one conforming to `^[a-zA-Z0-9_.-]{1,64}$`.
pub fn sanitize_property_key(key: &str) -> String {
    let replaced: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(MAX_KEY_LEN)
        .collect();
    if replaced.is_empty() {
        "param".to_string()
    } else {
        replaced
    }
}

/// Rewrite `schema` into a shape strict backends accept. The input is not mutated.
pub fn sanitize_schema(schema: &Value) -> Value {
    let collapsed = collapse_nullable_union(schema);
    let mut node = sanitize_node(&collapsed);
    if let Value::Object(map) = &mut node {
        for key in TOP_LEVEL_FORBIDDEN {
            map.remove(*key);
        }
    }
    node
}

/// Restore original property keys in `args`, inverting the rename that
/// [`sanitize_schema`] applied. `params_schema` is the **original** (wire) schema.
pub fn unrename_tool_args(params_schema: &Value, args: Value) -> Value {
    let Some(props) = params_schema.get("properties").and_then(Value::as_object) else {
        return args;
    };
    let renames = rename_map(props);
    if renames.is_empty() {
        return args;
    }
    let Value::Object(map) = args else {
        return args;
    };
    let restored = map
        .into_iter()
        .map(|(k, v)| match renames.get(&k) {
            Some(original) => (original.clone(), v),
            None => (k, v),
        })
        .collect();
    Value::Object(restored)
}

/// Sanitized key -> original key, for every property key that needs renaming.
///
/// Deterministic for a given schema: keys are walked in the map's own order and
/// a collision takes the next free `_N` suffix.
fn rename_map(props: &Map<String, Value>) -> std::collections::HashMap<String, String> {
    let mut taken: Vec<String> = Vec::new();
    let mut out = std::collections::HashMap::new();
    for original in props.keys() {
        let candidate = unique_key(&sanitize_property_key(original), &taken);
        taken.push(candidate.clone());
        if &candidate != original {
            out.insert(candidate, original.clone());
        }
    }
    out
}

/// First free variant of `base`, suffixing `_2`, `_3`, ... on collision.
fn unique_key(base: &str, taken: &[String]) -> String {
    if !taken.iter().any(|t| t == base) {
        return base.to_string();
    }
    (2..)
        .map(|n| {
            let suffix = format!("_{n}");
            let head_len = MAX_KEY_LEN.saturating_sub(suffix.len()).min(base.len());
            format!("{}{suffix}", &base[..head_len])
        })
        .find(|candidate| !taken.iter().any(|t| t == candidate))
        .unwrap_or_else(|| base.to_string())
}

/// Promote the non-null branch of a top-level `anyOf`/`oneOf` that only exists
/// to express nullability. Anthropic rejects that shape in `input_schema`.
fn collapse_nullable_union(schema: &Value) -> Value {
    let Some(obj) = schema.as_object() else {
        return schema.clone();
    };
    for union_key in ["anyOf", "oneOf"] {
        let Some(branches) = obj.get(union_key).and_then(Value::as_array) else {
            continue;
        };
        let non_null: Vec<&Value> = branches
            .iter()
            .filter(|b| b.get("type") != Some(&Value::String("null".into())))
            .collect();
        if non_null.len() == 1 && non_null.len() < branches.len() {
            let mut promoted = non_null[0].clone();
            if let (Some(target), Some(source)) = (promoted.as_object_mut(), schema.as_object()) {
                for (k, v) in source {
                    if k != union_key && !target.contains_key(k) {
                        target.insert(k.clone(), v.clone());
                    }
                }
            }
            return promoted;
        }
    }
    schema.clone()
}

/// Rewrite one schema node and everything below it.
fn sanitize_node(node: &Value) -> Value {
    // A bare type name standing in for a schema: `"string"` -> `{"type": "string"}`.
    if let Value::String(name) = node {
        return match name.as_str() {
            "object" | "string" | "number" | "integer" | "boolean" | "array" | "null" => {
                let mut map = Map::new();
                map.insert("type".into(), Value::String(name.clone()));
                sanitize_node(&Value::Object(map))
            }
            _ => node.clone(),
        };
    }

    let Some(obj) = node.as_object() else {
        return node.clone();
    };
    let mut out = Map::new();
    let mut lifted_required: Vec<String> = Vec::new();

    for (key, value) in obj {
        if SCHEMA_MAP_KEYS.contains(&key.as_str()) {
            out.insert(
                key.clone(),
                sanitize_schema_map(key, value, &mut lifted_required),
            );
        } else if SCHEMA_CHILD_KEYS.contains(&key.as_str()) {
            out.insert(key.clone(), sanitize_node(value));
        } else if SCHEMA_LIST_KEYS.contains(&key.as_str()) {
            let items = value
                .as_array()
                .map(|xs| Value::Array(xs.iter().map(sanitize_node).collect()))
                .unwrap_or_else(|| value.clone());
            out.insert(key.clone(), items);
        } else if key == "type" {
            normalize_type(value, &mut out);
        } else {
            out.insert(key.clone(), value.clone());
        }
    }

    if obj.contains_key("$ref") {
        for sibling in REF_FORBIDDEN_SIBLINGS {
            out.remove(*sibling);
        }
    }

    merge_required(&mut out, lifted_required);

    // llama.cpp's grammar converter needs `properties` on every object schema.
    if out.get("type") == Some(&Value::String("object".into())) && !out.contains_key("properties") {
        out.insert("properties".into(), Value::Object(Map::new()));
    }

    Value::Object(out)
}

/// Sanitize a `name -> schema` map, renaming non-conforming keys under
/// `properties` and collecting any legacy boolean `required` flags.
fn sanitize_schema_map(key: &str, value: &Value, lifted_required: &mut Vec<String>) -> Value {
    let Some(map) = value.as_object() else {
        return value.clone();
    };
    let renames_apply = key == "properties";
    let mut taken: Vec<String> = Vec::new();
    let mut out = Map::new();

    for (name, child) in map {
        let out_name = if renames_apply {
            let candidate = unique_key(&sanitize_property_key(name), &taken);
            taken.push(candidate.clone());
            candidate
        } else {
            name.clone()
        };

        let mut sanitized = sanitize_node(child);
        // A property-level `required: true` is legacy Draft-3 syntax; strict
        // backends want the name in the parent's `required` list instead.
        if let Some(flag) = sanitized.as_object_mut().and_then(|m| m.remove("required")) {
            match flag {
                Value::Bool(true) => lifted_required.push(out_name.clone()),
                Value::Bool(false) => {}
                other => {
                    if let Some(m) = sanitized.as_object_mut() {
                        m.insert("required".into(), other);
                    }
                }
            }
        }
        out.insert(out_name, sanitized);
    }

    Value::Object(out)
}

/// Collapse a `type` array into a single type plus a `nullable` hint.
fn normalize_type(value: &Value, out: &mut Map<String, Value>) {
    let Some(names) = value.as_array() else {
        out.insert("type".into(), value.clone());
        return;
    };
    let has_null = names.iter().any(|n| n == &Value::String("null".into()));
    let first_concrete = names
        .iter()
        .find(|n| *n != &Value::String("null".into()))
        .cloned();
    match first_concrete {
        Some(concrete) => {
            out.insert("type".into(), concrete);
            if has_null {
                out.insert("nullable".into(), Value::Bool(true));
            }
        }
        None => {
            out.insert("type".into(), Value::String("null".into()));
        }
    }
}

/// Merge lifted property names into the node's own `required` list.
fn merge_required(out: &mut Map<String, Value>, lifted: Vec<String>) {
    if lifted.is_empty() {
        return;
    }
    let mut names: Vec<Value> = out
        .get("required")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for name in lifted {
        let entry = Value::String(name);
        if !names.contains(&entry) {
            names.push(entry);
        }
    }
    out.insert("required".into(), Value::Array(names));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const KEY_RE_CHARS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_.-";

    fn key_is_conforming(k: &str) -> bool {
        !k.is_empty() && k.len() <= 64 && k.chars().all(|c| KEY_RE_CHARS.contains(c))
    }

    fn all_property_keys(schema: &Value, out: &mut Vec<String>) {
        match schema {
            Value::Object(map) => {
                if let Some(Value::Object(props)) = map.get("properties") {
                    out.extend(props.keys().cloned());
                }
                for v in map.values() {
                    all_property_keys(v, out);
                }
            }
            Value::Array(items) => items.iter().for_each(|v| all_property_keys(v, out)),
            _ => {}
        }
    }

    // ---- llama.cpp grammar-converter shapes ----------------------------

    #[test]
    fn adds_empty_properties_to_bare_object_schema() {
        let out = sanitize_schema(&json!({ "type": "object" }));
        assert_eq!(out, json!({ "type": "object", "properties": {} }));
    }

    #[test]
    fn expands_bare_string_schema_to_object_form() {
        let out = sanitize_schema(&json!({
            "type": "object",
            "properties": { "a": "string" }
        }));
        assert_eq!(
            out,
            json!({ "type": "object", "properties": { "a": { "type": "string" } } })
        );
    }

    #[test]
    fn collapses_type_array_to_single_type_with_nullable_hint() {
        let out = sanitize_schema(&json!({
            "type": "object",
            "properties": { "a": { "type": ["string", "null"] } }
        }));
        assert_eq!(
            out,
            json!({
                "type": "object",
                "properties": { "a": { "type": "string", "nullable": true } }
            })
        );
    }

    // ---- property-key conformance --------------------------------------

    #[test]
    fn renames_property_key_with_illegal_characters() {
        let out = sanitize_schema(&json!({
            "type": "object",
            "properties": { "bad key!": { "type": "string" } }
        }));
        assert_eq!(
            out,
            json!({
                "type": "object",
                "properties": { "bad_key_": { "type": "string" } }
            })
        );
    }

    #[test]
    fn truncates_over_long_property_key_to_64_chars() {
        let long = "a".repeat(80);
        let out = sanitize_schema(&json!({
            "type": "object",
            "properties": { long.clone(): { "type": "string" } }
        }));
        let mut keys = Vec::new();
        all_property_keys(&out, &mut keys);
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].len(), 64);
        assert!(key_is_conforming(&keys[0]));
    }

    #[test]
    fn maps_all_illegal_key_to_param() {
        assert_eq!(sanitize_property_key("!!!"), "___");
        assert_eq!(sanitize_property_key(""), "param");
    }

    #[test]
    fn keeps_colliding_renamed_keys_addressable() {
        let out = sanitize_schema(&json!({
            "type": "object",
            "properties": {
                "a b": { "type": "string" },
                "a!b": { "type": "integer" }
            }
        }));
        let mut keys = Vec::new();
        all_property_keys(&out, &mut keys);
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), 2, "colliding keys must stay distinct: {keys:?}");
        assert!(keys.iter().all(|k| key_is_conforming(k)));
    }

    #[test]
    fn renames_nested_property_keys_too() {
        let out = sanitize_schema(&json!({
            "type": "object",
            "properties": {
                "outer": {
                    "type": "object",
                    "properties": { "in ner": { "type": "string" } }
                }
            }
        }));
        let mut keys = Vec::new();
        all_property_keys(&out, &mut keys);
        assert!(keys.iter().all(|k| key_is_conforming(k)), "{keys:?}");
        assert!(keys.contains(&"in_ner".to_string()));
    }

    // ---- strict-validator shapes ---------------------------------------

    #[test]
    fn strips_default_beside_ref() {
        let out = sanitize_schema(&json!({
            "type": "object",
            "properties": { "a": { "$ref": "#/$defs/T", "default": 1 } },
            "$defs": { "T": { "type": "string" } }
        }));
        let a = &out["properties"]["a"];
        assert_eq!(a["$ref"], json!("#/$defs/T"));
        assert!(
            a.get("default").is_none(),
            "default must be stripped beside $ref"
        );
    }

    #[test]
    fn strips_each_top_level_combinator() {
        for key in ["allOf", "anyOf", "oneOf", "enum", "not"] {
            let mut schema = json!({ "type": "object", "properties": {} });
            schema[key] = json!([{ "type": "object" }]);
            let out = sanitize_schema(&schema);
            assert!(
                out.get(key).is_none(),
                "top-level {key} must be stripped, got {out}"
            );
        }
    }

    #[test]
    fn collapses_top_level_nullable_any_of() {
        let out = sanitize_schema(&json!({
            "anyOf": [
                { "type": "object", "properties": { "a": { "type": "string" } } },
                { "type": "null" }
            ]
        }));
        assert_eq!(out["type"], json!("object"));
        assert_eq!(out["properties"]["a"]["type"], json!("string"));
    }

    #[test]
    fn lifts_legacy_boolean_required_flag_into_parent_list() {
        let out = sanitize_schema(&json!({
            "type": "object",
            "properties": {
                "a": { "type": "string", "required": true },
                "b": { "type": "string", "required": false }
            }
        }));
        assert!(out["properties"]["a"].get("required").is_none());
        assert!(out["properties"]["b"].get("required").is_none());
        assert_eq!(out["required"], json!(["a"]));
    }

    // ---- invariants -----------------------------------------------------

    #[test]
    fn does_not_mutate_input() {
        let input = json!({ "type": "object" });
        let before = input.clone();
        let _ = sanitize_schema(&input);
        assert_eq!(input, before);
    }

    #[test]
    fn is_idempotent() {
        let cases = vec![
            json!({ "type": "object" }),
            json!({ "type": "object", "properties": { "bad key!": { "type": "string" } } }),
            json!({ "type": "object", "properties": { "a": { "type": ["string", "null"] } } }),
            json!({ "type": "object", "properties": { "a": "string" } }),
            json!({ "anyOf": [{ "type": "object", "properties": {} }, { "type": "null" }] }),
            json!({ "type": "object", "properties": { "a": { "$ref": "#/$defs/T", "default": 1 } } }),
        ];
        for case in cases {
            let once = sanitize_schema(&case);
            let twice = sanitize_schema(&once);
            assert_eq!(once, twice, "not idempotent for {case}");
        }
    }

    #[test]
    fn preserves_deeply_nested_schema_containers() {
        let input = json!({
            "type": "object",
            "properties": {
                "xs": {
                    "type": "array",
                    "items": { "type": "object", "properties": { "a": { "type": "integer" } } }
                }
            },
            "$defs": { "T": { "type": "object", "properties": { "b": { "type": "string" } } } }
        });
        let out = sanitize_schema(&input);
        assert_eq!(
            out["properties"]["xs"]["items"]["properties"]["a"]["type"],
            json!("integer")
        );
        assert_eq!(
            out["$defs"]["T"]["properties"]["b"]["type"],
            json!("string")
        );
    }

    #[test]
    fn leaves_already_conforming_schema_unchanged() {
        let input = json!({
            "type": "object",
            "properties": { "query": { "type": "string", "description": "the query" } },
            "required": ["query"]
        });
        assert_eq!(sanitize_schema(&input), input);
    }

    // ---- round trip -----------------------------------------------------

    #[test]
    fn unrenames_args_back_to_original_wire_keys() {
        let original = json!({
            "type": "object",
            "properties": { "bad key!": { "type": "string" } }
        });
        let sanitized = sanitize_schema(&original);
        let renamed_key = sanitized["properties"]
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap()
            .clone();

        let model_args = json!({ renamed_key.clone(): "v" });
        let restored = unrename_tool_args(&original, model_args);

        assert_eq!(restored, json!({ "bad key!": "v" }));
    }

    #[test]
    fn unrename_round_trips_every_rename_case() {
        let long_key = "a".repeat(80);
        let cases = vec![
            "bad key!",
            "with spaces",
            "emoji_🚀_key",
            "slash/key",
            long_key.as_str(),
        ];
        for key in cases {
            let original = json!({
                "type": "object",
                "properties": { key: { "type": "string" } }
            });
            let sanitized = sanitize_schema(&original);
            let renamed = sanitized["properties"]
                .as_object()
                .unwrap()
                .keys()
                .next()
                .unwrap()
                .clone();
            let restored = unrename_tool_args(&original, json!({ renamed: "v" }));
            assert_eq!(
                restored,
                json!({ key: "v" }),
                "round trip failed for key {key:?}"
            );
        }
    }

    #[test]
    fn unrename_leaves_conforming_keys_alone() {
        let original = json!({
            "type": "object",
            "properties": { "query": { "type": "string" } }
        });
        let args = json!({ "query": "hello" });
        assert_eq!(unrename_tool_args(&original, args.clone()), args);
    }
}
