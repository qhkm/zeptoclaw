//! Tool-argument type coercion: repair string-typed values the model emitted
//! against a tool's JSON Schema.
//!
//! Small local models emit `"42"` for integers, `"true"` for booleans,
//! JSON-encoded strings for arrays/objects (also nested inside containers),
//! and bare scalars where an array is expected. Coercion is schema-guided and
//! conservative: the original value is kept whenever a repair is not
//! unambiguous — in particular a string is never rewritten when the schema
//! already permits a string, so identifiers like `"07030"` survive a
//! `["string", "integer"]` union intact.

use serde_json::Value;

/// Coerce string-typed args to the JSON-Schema types declared by `params_schema`.
///
/// `params_schema` is the tool's `parameters` schema (the object carrying
/// `properties`). Values that cannot be repaired unambiguously are left alone.
pub fn coerce_tool_args(params_schema: &Value, args: Value) -> Value {
    let Some(props) = params_schema.get("properties").and_then(Value::as_object) else {
        return args;
    };
    let Value::Object(mut map) = args else {
        return args;
    };

    let keys: Vec<String> = map.keys().cloned().collect();
    for key in keys {
        let Some(prop) = props.get(&key) else {
            continue;
        };
        let Some(value) = map.get(&key) else { continue };
        if let Some(fixed) = coerce_at(value, prop, &key) {
            map.insert(key, fixed);
        }
    }

    Value::Object(map)
}

/// Repair `value` against `schema` at any depth; `None` means "leave as is".
///
/// `path` names the position for logging only.
fn coerce_at(value: &Value, schema: &Value, path: &str) -> Option<Value> {
    if !schema.is_object() {
        return None;
    }
    let accepted = accepted_types(schema);
    let wants_array = accepted.contains(&"array");

    if let Value::String(s) = value {
        // A nullable position spelled as the literal string "null".
        if allows_null(schema, &accepted) && s.trim().eq_ignore_ascii_case("null") {
            return Some(Value::Null);
        }
        // The schema already permits a string, so nothing here is broken and
        // any rewrite would be a guess. Keeps `"07030"` a zip code.
        if accepted.contains(&"string") {
            return None;
        }
        // A JSON-encoded container, which may itself hold more repairable values.
        if let Some(parsed) = parse_container(s, &accepted) {
            return Some(coerce_at(&parsed, schema, path).unwrap_or(parsed));
        }
        if let Some(scalar) = coerce_scalar(s, &accepted) {
            return Some(scalar);
        }
        if wants_array {
            if s.trim_start().starts_with('[') {
                tracing::warn!(
                    property = path,
                    "tool arg looks like a JSON array string but could not be parsed; \
                     wrapping it as a single-element list"
                );
            }
            tracing::debug!(property = path, "wrapped bare tool arg in a list");
            return Some(Value::Array(vec![value.clone()]));
        }
        return None;
    }

    // A bare, non-array value where an array is declared. An explicit null is
    // preserved: the tool's own default handling decides between "omit" and
    // "empty list".
    if wants_array && !value.is_null() && !value.is_array() {
        tracing::debug!(property = path, "wrapped bare tool arg in a list");
        let item = schema
            .get("items")
            .and_then(|items| coerce_at(value, items, path))
            .unwrap_or_else(|| value.clone());
        return Some(Value::Array(vec![item]));
    }

    // Native containers: repair each position against its own schema.
    match value {
        Value::Array(items) => {
            let item_schema = schema.get("items")?;
            let mut changed = false;
            let out: Vec<Value> = items
                .iter()
                .map(|item| match coerce_at(item, item_schema, path) {
                    Some(fixed) => {
                        changed = true;
                        fixed
                    }
                    None => item.clone(),
                })
                .collect();
            changed.then_some(Value::Array(out))
        }
        Value::Object(fields) => {
            let props = schema.get("properties").and_then(Value::as_object)?;
            let mut out = fields.clone();
            let mut changed = false;
            for (name, prop_schema) in props {
                if let Some(field) = fields.get(name) {
                    if let Some(fixed) = coerce_at(field, prop_schema, name) {
                        out.insert(name.clone(), fixed);
                        changed = true;
                    }
                }
            }
            changed.then_some(Value::Object(out))
        }
        _ => None,
    }
}

/// Every JSON type name the schema permits, following `anyOf`/`oneOf`/`allOf`.
fn accepted_types(schema: &Value) -> Vec<&str> {
    let mut out = type_names(schema.get("type"));
    for union_key in ["anyOf", "oneOf", "allOf"] {
        if let Some(branches) = schema.get(union_key).and_then(Value::as_array) {
            for branch in branches {
                out.extend(accepted_types(branch));
            }
        }
    }
    out
}

/// The JSON type names a `type` entry declares (`"x"` or `["x", "y"]`).
fn type_names(expected: Option<&Value>) -> Vec<&str> {
    match expected {
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

/// True when the schema explicitly permits null, via `type`, a union branch, or
/// the OpenAPI-style `nullable` flag.
fn allows_null(schema: &Value, accepted: &[&str]) -> bool {
    accepted.contains(&"null") || schema.get("nullable") == Some(&Value::Bool(true))
}

/// Parse a JSON-encoded container when the schema expects one at this position.
///
/// Schema-guided, so a legitimately JSON-looking `type: string` value is never
/// parsed (that case returns earlier, before this is reached).
fn parse_container(s: &str, accepted: &[&str]) -> Option<Value> {
    let trimmed = s.trim();
    let expects_array = accepted.contains(&"array");
    let expects_object = accepted.contains(&"object");
    let plausible =
        (expects_array && trimmed.starts_with('[')) || (expects_object && trimmed.starts_with('{'));
    if !plausible {
        return None;
    }
    let parsed: Value = serde_json::from_str(trimmed).ok()?;
    ((parsed.is_array() && expects_array) || (parsed.is_object() && expects_object))
        .then_some(parsed)
}

/// Coerce a string to the first scalar type the schema accepts it as.
fn coerce_scalar(s: &str, accepted: &[&str]) -> Option<Value> {
    accepted.iter().find_map(|t| match *t {
        "integer" => coerce_number(s, true),
        "number" => coerce_number(s, false),
        "boolean" => coerce_boolean(s),
        _ => None,
    })
}

/// Parse `s` as a number. Rejects non-finite values (not JSON-serializable)
/// and, when `integer_only`, anything with a fractional part.
fn coerce_number(s: &str, integer_only: bool) -> Option<Value> {
    let f: f64 = s.trim().parse().ok()?;
    if !f.is_finite() {
        return None;
    }
    if f.fract() == 0.0 && f.abs() <= i64::MAX as f64 {
        return Some(Value::from(f as i64));
    }
    (!integer_only).then(|| Value::from(f))
}

/// Parse `"true"`/`"false"` case-insensitively; `None` for anything else.
fn coerce_boolean(s: &str) -> Option<Value> {
    match s.trim().to_ascii_lowercase().as_str() {
        "true" => Some(Value::Bool(true)),
        "false" => Some(Value::Bool(false)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema(props: Value) -> Value {
        json!({ "type": "object", "properties": props })
    }

    fn coerce(props: Value, args: Value) -> Value {
        coerce_tool_args(&schema(props), args)
    }

    // ---- repairs -------------------------------------------------------

    #[test]
    fn coerces_string_to_integer() {
        let out = coerce(json!({ "n": { "type": "integer" } }), json!({ "n": "42" }));
        assert_eq!(out, json!({ "n": 42 }));
    }

    #[test]
    fn coerces_string_to_number() {
        let out = coerce(json!({ "n": { "type": "number" } }), json!({ "n": "1.5" }));
        assert_eq!(out, json!({ "n": 1.5 }));
    }

    #[test]
    fn coerces_string_to_boolean_case_insensitively() {
        let props = json!({ "b": { "type": "boolean" } });
        assert_eq!(
            coerce(props.clone(), json!({ "b": "true" })),
            json!({ "b": true })
        );
        assert_eq!(
            coerce(props.clone(), json!({ "b": "FALSE" })),
            json!({ "b": false })
        );
        assert_eq!(
            coerce(props, json!({ "b": " True " })),
            json!({ "b": true })
        );
    }

    #[test]
    fn parses_json_encoded_array_string() {
        let out = coerce(
            json!({ "xs": { "type": "array", "items": { "type": "integer" } } }),
            json!({ "xs": "[1,2]" }),
        );
        assert_eq!(out, json!({ "xs": [1, 2] }));
    }

    #[test]
    fn parses_json_encoded_object_string() {
        let out = coerce(
            json!({ "o": { "type": "object", "properties": { "a": { "type": "integer" } } } }),
            json!({ "o": "{\"a\":1}" }),
        );
        assert_eq!(out, json!({ "o": { "a": 1 } }));
    }

    #[test]
    fn wraps_bare_string_in_array() {
        let out = coerce(
            json!({ "xs": { "type": "array", "items": { "type": "string" } } }),
            json!({ "xs": "x" }),
        );
        assert_eq!(out, json!({ "xs": ["x"] }));
    }

    #[test]
    fn wraps_bare_number_in_array() {
        let out = coerce(
            json!({ "xs": { "type": "array", "items": { "type": "integer" } } }),
            json!({ "xs": 5 }),
        );
        assert_eq!(out, json!({ "xs": [5] }));
    }

    #[test]
    fn coerces_null_string_when_type_array_allows_null() {
        let out = coerce(
            json!({ "s": { "type": ["string", "null"] } }),
            json!({ "s": "null" }),
        );
        assert_eq!(out, json!({ "s": null }));
    }

    #[test]
    fn coerces_null_string_when_nullable_flag_set() {
        let out = coerce(
            json!({ "s": { "type": "string", "nullable": true } }),
            json!({ "s": "null" }),
        );
        assert_eq!(out, json!({ "s": null }));
    }

    #[test]
    fn coerces_null_string_when_any_of_has_null_branch() {
        let out = coerce(
            json!({ "s": { "anyOf": [{ "type": "string" }, { "type": "null" }] } }),
            json!({ "s": "null" }),
        );
        assert_eq!(out, json!({ "s": null }));
    }

    #[test]
    fn union_type_picks_first_unambiguous_branch() {
        // No `string` branch, so "7" cannot have been meant as a string.
        let out = coerce(
            json!({ "v": { "type": ["integer", "boolean"] } }),
            json!({ "v": "7" }),
        );
        assert_eq!(out, json!({ "v": 7 }));
    }

    #[test]
    fn parses_json_string_element_inside_native_array() {
        let out = coerce(
            json!({ "xs": {
                "type": "array",
                "items": { "type": "object", "properties": { "a": { "type": "integer" } } }
            } }),
            json!({ "xs": ["{\"a\":1}"] }),
        );
        assert_eq!(out, json!({ "xs": [{ "a": 1 }] }));
    }

    #[test]
    fn parses_json_string_inside_object_property() {
        let out = coerce(
            json!({ "o": {
                "type": "object",
                "properties": { "xs": { "type": "array", "items": { "type": "integer" } } }
            } }),
            json!({ "o": { "xs": "[1,2]" } }),
        );
        assert_eq!(out, json!({ "o": { "xs": [1, 2] } }));
    }

    #[test]
    fn coerces_scalar_string_inside_nested_object() {
        let out = coerce(
            json!({ "o": { "type": "object", "properties": { "n": { "type": "integer" } } } }),
            json!({ "o": { "n": "42" } }),
        );
        assert_eq!(out, json!({ "o": { "n": 42 } }));
    }

    #[test]
    fn coerces_scalar_string_inside_array_items() {
        let out = coerce(
            json!({ "xs": { "type": "array", "items": { "type": "integer" } } }),
            json!({ "xs": ["42"] }),
        );
        assert_eq!(out, json!({ "xs": [42] }));
    }

    #[test]
    fn coerces_scalar_string_two_levels_deep() {
        let out = coerce(
            json!({ "o": {
                "type": "object",
                "properties": { "inner": {
                    "type": "object",
                    "properties": { "flag": { "type": "boolean" } }
                } }
            } }),
            json!({ "o": { "inner": { "flag": "true" } } }),
        );
        assert_eq!(out, json!({ "o": { "inner": { "flag": true } } }));
    }

    // ---- regression guards: must NOT touch -----------------------------

    #[test]
    fn preserves_string_when_schema_also_permits_string() {
        // "7" already satisfies `string`, so the intended type is ambiguous.
        let out = coerce(
            json!({ "v": { "type": ["integer", "string"] } }),
            json!({ "v": "7" }),
        );
        assert_eq!(out, json!({ "v": "7" }));
    }

    #[test]
    fn preserves_leading_zero_identifier_for_string_or_integer_union() {
        let out = coerce(
            json!({ "zip": { "type": ["string", "integer"] } }),
            json!({ "zip": "07030" }),
        );
        assert_eq!(out, json!({ "zip": "07030" }));
    }

    #[test]
    fn does_not_truncate_decimal_for_integer_schema() {
        let out = coerce(
            json!({ "n": { "type": "integer" } }),
            json!({ "n": "42.5" }),
        );
        assert_eq!(out, json!({ "n": "42.5" }));
    }

    #[test]
    fn does_not_coerce_non_boolean_words() {
        let props = json!({ "b": { "type": "boolean" } });
        assert_eq!(
            coerce(props.clone(), json!({ "b": "yes" })),
            json!({ "b": "yes" })
        );
        assert_eq!(coerce(props, json!({ "b": "1" })), json!({ "b": "1" }));
    }

    #[test]
    fn does_not_parse_json_looking_string_for_string_schema() {
        let out = coerce(
            json!({ "s": { "type": "string" } }),
            json!({ "s": "{\"a\":1}" }),
        );
        assert_eq!(out, json!({ "s": "{\"a\":1}" }));
    }

    #[test]
    fn does_not_coerce_non_finite_numbers() {
        let props = json!({ "n": { "type": "number" } });
        assert_eq!(
            coerce(props.clone(), json!({ "n": "inf" })),
            json!({ "n": "inf" })
        );
        assert_eq!(
            coerce(props.clone(), json!({ "n": "nan" })),
            json!({ "n": "nan" })
        );
        assert_eq!(
            coerce(props, json!({ "n": "-inf" })),
            json!({ "n": "-inf" })
        );
    }

    #[test]
    fn does_not_coerce_null_string_for_non_nullable_schema() {
        let out = coerce(json!({ "s": { "type": "string" } }), json!({ "s": "null" }));
        assert_eq!(out, json!({ "s": "null" }));
    }

    #[test]
    fn leaves_properties_absent_from_schema_untouched() {
        let out = coerce(
            json!({ "known": { "type": "integer" } }),
            json!({ "other": "42" }),
        );
        assert_eq!(out, json!({ "other": "42" }));
    }

    #[test]
    fn passes_through_empty_and_non_object_args() {
        let props = json!({ "n": { "type": "integer" } });
        assert_eq!(coerce(props.clone(), json!({})), json!({}));
        assert_eq!(coerce(props, json!("scalar")), json!("scalar"));
    }

    #[test]
    fn passes_through_when_schema_has_no_properties() {
        let out = coerce_tool_args(&json!({ "type": "object" }), json!({ "n": "42" }));
        assert_eq!(out, json!({ "n": "42" }));
    }

    #[test]
    fn falls_back_to_single_element_list_for_malformed_json_array_string() {
        let out = coerce(
            json!({ "xs": { "type": "array", "items": { "type": "string" } } }),
            json!({ "xs": "[1,2" }),
        );
        assert_eq!(out, json!({ "xs": ["[1,2"] }));
    }

    #[test]
    fn preserves_explicit_null_for_array_schema() {
        let out = coerce(
            json!({ "xs": { "type": "array", "items": { "type": "string" } } }),
            json!({ "xs": null }),
        );
        assert_eq!(out, json!({ "xs": null }));
    }
}
