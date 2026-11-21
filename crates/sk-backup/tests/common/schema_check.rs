//! Strict structural validation of JSON against the subset of JSON Schema that schemars
//! emits: `$ref`, `type`, `properties`, `required`, `enum`, `const`, `oneOf`, `anyOf`,
//! `items`, `additionalProperties`, string lengths.
//!
//! Unlike JSON Schema, a property missing from `properties` is an error unless
//! `additionalProperties` allows it: this catches schema mirrors that lag behind the types.

use serde_json::Value;

/// Validates `value` against `root`; returns the list of problems with JSON paths.
pub fn validate(root: &Value, value: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    check(root, root, value, "$", &mut errors);
    errors
}

fn resolve<'a>(root: &'a Value, schema: &'a Value) -> &'a Value {
    match schema.get("$ref").and_then(Value::as_str) {
        Some(reference) => {
            let pointer = reference.trim_start_matches('#');
            match root.pointer(pointer) {
                Some(target) => resolve(root, target),
                None => schema,
            }
        }
        None => schema,
    }
}

fn type_matches(name: &str, value: &Value) -> bool {
    match name {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        _ => false,
    }
}

fn check(root: &Value, schema: &Value, value: &Value, path: &str, errors: &mut Vec<String>) {
    if schema == &Value::Bool(true) {
        return;
    }
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        if root.pointer(reference.trim_start_matches('#')).is_none() {
            errors.push(format!("{path}: unresolved $ref {reference}"));
            return;
        }
    }
    let schema = resolve(root, schema);

    if let Some(ty) = schema.get("type") {
        let ok = match ty {
            Value::String(name) => type_matches(name, value),
            Value::Array(names) => names
                .iter()
                .filter_map(Value::as_str)
                .any(|name| type_matches(name, value)),
            _ => true,
        };
        if !ok {
            errors.push(format!("{path}: expected type {ty}, got {value}"));
            return;
        }
    }
    if let Some(expected) = schema.get("const") {
        if expected != value {
            errors.push(format!("{path}: expected {expected}, got {value}"));
        }
    }
    if let Some(Value::Array(options)) = schema.get("enum") {
        if !options.contains(value) {
            errors.push(format!("{path}: {value} not in {options:?}"));
        }
    }
    for (key, exactly_one) in [("oneOf", true), ("anyOf", false)] {
        if let Some(Value::Array(variants)) = schema.get(key) {
            let matching = variants
                .iter()
                .filter(|variant| {
                    let mut sub = Vec::new();
                    check(root, variant, value, path, &mut sub);
                    sub.is_empty()
                })
                .count();
            if matching == 0 || (exactly_one && matching > 1) {
                errors.push(format!(
                    "{path}: {matching} variants of {key} match {value}"
                ));
            }
        }
    }
    if let Some(text) = value.as_str() {
        let len = text.chars().count() as u64;
        if schema
            .get("minLength")
            .and_then(Value::as_u64)
            .is_some_and(|min| len < min)
            || schema
                .get("maxLength")
                .and_then(Value::as_u64)
                .is_some_and(|max| len > max)
        {
            errors.push(format!("{path}: length of {text:?} out of bounds"));
        }
    }
    if let (Some(items), Some(array)) = (schema.get("items"), value.as_array()) {
        for (i, item) in array.iter().enumerate() {
            check(root, items, item, &format!("{path}[{i}]"), errors);
        }
    }
    if let Some(object) = value.as_object() {
        check_object(root, schema, object, path, errors);
    }
}

fn check_object(
    root: &Value,
    schema: &Value,
    object: &serde_json::Map<String, Value>,
    path: &str,
    errors: &mut Vec<String>,
) {
    let properties = schema.get("properties").and_then(Value::as_object);
    if let Some(Value::Array(required)) = schema.get("required") {
        for name in required.iter().filter_map(Value::as_str) {
            if !object.contains_key(name) {
                errors.push(format!("{path}: missing required property {name}"));
            }
        }
    }
    let additional = schema.get("additionalProperties");
    // Only a schema that describes an object's shape is strict; combinators are checked per variant.
    let describes_object = properties.is_some() || additional.is_some();
    for (key, item) in object {
        let item_path = format!("{path}.{key}");
        match properties.and_then(|props| props.get(key)) {
            Some(sub) => check(root, sub, item, &item_path, errors),
            None => match additional {
                Some(Value::Bool(false)) => {
                    errors.push(format!("{path}: unknown property {key}"));
                }
                Some(sub) => check(root, sub, item, &item_path, errors),
                None if describes_object => {
                    errors.push(format!("{path}: property {key} is not in the schema"));
                }
                None => {}
            },
        }
    }
}
