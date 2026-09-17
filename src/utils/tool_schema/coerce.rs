//! Tool-argument type coercion: repair string-typed values the model emitted
//! against a tool's JSON Schema.
//!
//! Small local models emit `"42"` for integers, `"true"` for booleans,
//! JSON-encoded strings for arrays/objects (also nested inside containers),
//! and bare scalars where an array is expected. Coercion is schema-guided and
//! conservative: the original value is kept whenever a repair is not unambiguous.

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
        if let Some(fixed) = coerce_property(&key, value, prop) {
            map.insert(key, fixed);
        }
    }

    Value::Object(map)
}

/// Repair one property value against its schema; `None` means "leave as is".
fn coerce_property(key: &str, value: &Value, prop: &Value) -> Option<Value> {
    let expected = prop.get("type");
    let wants_array = type_names(expected).contains(&"array");

    // A bare, non-array value where an array is expected. Strings go through
    // `coerce_value` first, so a JSON-encoded array parses and a nullable
    // "null" becomes null rather than ["null"]. An explicit null is preserved:
    // the tool's own default handling decides between "omit" and "empty list".
    if wants_array && !value.is_null() && !value.is_array() {
        if let Value::String(s) = value {
            if let Some(coerced) = coerce_value(s, expected, prop) {
                return Some(coerced);
            }
            if s.trim_start().starts_with('[') {
                tracing::warn!(
                    property = key,
                    "tool arg looks like a JSON array string but could not be parsed; \
                     wrapping it as a single-element list"
                );
            }
        }
        tracing::debug!(property = key, "wrapped bare tool arg in a list");
        return Some(Value::Array(vec![value.clone()]));
    }

    let Value::String(s) = value else {
        // Native container: still normalize JSON-encoded elements and sub-fields.
        let normalize = (wants_array && value.is_array())
            || (type_names(expected).contains(&"object") && value.is_object());
        return normalize
            .then(|| normalize_json_strings(value, prop))
            .flatten();
    };

    if expected.is_none() && !schema_allows_null(prop) {
        return None;
    }

    let coerced = coerce_value(s, expected, prop)?;
    Some(normalize_json_strings(&coerced, prop).unwrap_or(coerced))
}

/// The JSON type names a `type` entry declares (`"x"` or `["x", "y"]`).
fn type_names(expected: Option<&Value>) -> Vec<&str> {
    match expected {
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    }
}

/// True when `schema` permits JSON type `kind` via `type` or any combinator branch.
fn schema_accepts_kind(schema: &Value, kind: &str) -> bool {
    let Some(obj) = schema.as_object() else {
        return false;
    };
    if type_names(obj.get("type")).contains(&kind) {
        return true;
    }
    ["anyOf", "oneOf", "allOf"].iter().any(|union_key| {
        obj.get(*union_key)
            .and_then(Value::as_array)
            .is_some_and(|branches| branches.iter().any(|b| schema_accepts_kind(b, kind)))
    })
}

/// Recursively parse JSON-encoded strings where the schema expects array/object.
///
/// Schema-guided: a string is only parsed when its schema position expects a
/// container, so legitimate JSON-looking `type: string` fields survive.
/// `None` means nothing changed.
fn normalize_json_strings(value: &Value, schema: &Value) -> Option<Value> {
    if !schema.is_object() {
        return None;
    }

    let mut current = None;
    if let Value::String(s) = value {
        let trimmed = s.trim();
        let expects_array = schema_accepts_kind(schema, "array");
        let expects_object = schema_accepts_kind(schema, "object");
        let plausible = (expects_array && trimmed.starts_with('['))
            || (expects_object && trimmed.starts_with('{'));
        if !plausible {
            return None;
        }
        let parsed: Value = serde_json::from_str(trimmed).ok()?;
        let matches =
            (parsed.is_array() && expects_array) || (parsed.is_object() && expects_object);
        if !matches {
            return None;
        }
        current = Some(parsed);
    }

    let target = current.as_ref().unwrap_or(value);
    let deeper = match target {
        Value::Array(items) => {
            let item_schema = schema.get("items")?;
            let mut changed = false;
            let out: Vec<Value> = items
                .iter()
                .map(|item| match normalize_json_strings(item, item_schema) {
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
            for (k, prop_schema) in props {
                if let Some(field) = fields.get(k) {
                    if let Some(fixed) = normalize_json_strings(field, prop_schema) {
                        out.insert(k.clone(), fixed);
                        changed = true;
                    }
                }
            }
            changed.then_some(Value::Object(out))
        }
        _ => None,
    };

    deeper.or(current)
}

/// Coerce string `s` to `expected` (a name or a union list); `None` on failure.
fn coerce_value(s: &str, expected: Option<&Value>, schema: &Value) -> Option<Value> {
    if schema_allows_null(schema) && s.trim().eq_ignore_ascii_case("null") {
        return Some(Value::Null);
    }
    type_names(expected).into_iter().find_map(|t| match t {
        "integer" => coerce_number(s, true),
        "number" => coerce_number(s, false),
        "boolean" => coerce_boolean(s),
        "array" => coerce_json(s, Value::is_array),
        "object" => coerce_json(s, Value::is_object),
        _ => None,
    })
}

/// True when a JSON Schema fragment explicitly permits null.
fn schema_allows_null(schema: &Value) -> bool {
    let Some(obj) = schema.as_object() else {
        return false;
    };
    if type_names(obj.get("type")).contains(&"null") {
        return true;
    }
    if obj.get("nullable") == Some(&Value::Bool(true)) {
        return true;
    }
    ["anyOf", "oneOf"].iter().any(|union_key| {
        obj.get(*union_key)
            .and_then(Value::as_array)
            .is_some_and(|variants| {
                variants
                    .iter()
                    .any(|v| type_names(v.get("type")).contains(&"null"))
            })
    })
}

/// `serde_json::from_str` when the parsed value is the container kind wanted.
fn coerce_json(s: &str, wanted: fn(&Value) -> bool) -> Option<Value> {
    let parsed: Value = serde_json::from_str(s).ok()?;
    wanted(&parsed).then_some(parsed)
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
        let out = coerce(
            json!({ "v": { "type": ["integer", "string"] } }),
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

    // ---- regression guards: must NOT touch -----------------------------

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
