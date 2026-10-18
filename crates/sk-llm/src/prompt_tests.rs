use super::{PROMPT_VERSION, RESPONSE_SCHEMA, SYSTEM_PROMPT};
use serde_json::Value;
use sk_core::model::Category;

/// Every `Category` variant. The exhaustive match in `all_listed` fails to
/// compile when a variant is added, forcing this list to be updated.
const ALL: [Category; 11] = [
    Category::GameSave,
    Category::GameConfig,
    Category::AppConfig,
    Category::AppData,
    Category::UserFiles,
    Category::DevEnvironment,
    Category::Credentials,
    Category::SystemSettings,
    Category::Reinstallable,
    Category::Cache,
    Category::Unknown,
];

fn all_listed(c: Category) -> bool {
    match c {
        Category::GameSave
        | Category::GameConfig
        | Category::AppConfig
        | Category::AppData
        | Category::UserFiles
        | Category::DevEnvironment
        | Category::Credentials
        | Category::SystemSettings
        | Category::Reinstallable
        | Category::Cache
        | Category::Unknown => ALL.contains(&c),
    }
}

fn schema() -> Value {
    serde_json::from_str(RESPONSE_SCHEMA).expect("response schema is valid JSON")
}

fn serde_name(c: Category) -> String {
    match serde_json::to_value(c).expect("category serializes") {
        Value::String(s) => s,
        other => panic!("category serialized as {other}"),
    }
}

#[test]
fn prompt_version_is_v1() {
    assert_eq!(PROMPT_VERSION, "v1");
}

#[test]
fn system_prompt_is_loaded() {
    assert!(SYSTEM_PROMPT.starts_with("You are a file-system analyst"));
    assert!(SYSTEM_PROMPT.contains("Output JSON only."));
    assert!(!SYSTEM_PROMPT.contains('\r'));
}

#[test]
fn schema_category_enum_matches_category() {
    let schema = schema();
    let names: Vec<&str> = schema["properties"]["results"]["items"]["properties"]["category"]
        ["enum"]
        .as_array()
        .expect("category enum is an array")
        .iter()
        .map(|v| v.as_str().expect("enum value is a string"))
        .collect();
    assert_eq!(names.len(), ALL.len());
    for c in ALL {
        assert!(all_listed(c));
        let name = serde_name(c);
        assert!(
            names.contains(&name.as_str()),
            "{name} missing from schema enum"
        );
        let back: Category =
            serde_json::from_value(Value::String(name.clone())).expect("enum value deserializes");
        assert_eq!(back, c);
    }
}

#[test]
fn prompt_lists_every_category() {
    for c in ALL {
        let name = serde_name(c);
        assert!(
            SYSTEM_PROMPT
                .lines()
                .any(|l| l.trim_start().starts_with(&name)),
            "{name} missing from system prompt"
        );
    }
}

fn walk(v: &Value, objects: &mut usize) {
    match v {
        Value::Object(map) => {
            for banned in ["minimum", "maximum", "maxLength", "minLength"] {
                assert!(!map.contains_key(banned), "schema contains {banned}");
            }
            if map.get("type").and_then(Value::as_str) == Some("object") {
                *objects += 1;
                assert_eq!(map.get("additionalProperties"), Some(&Value::Bool(false)));
            }
            map.values().for_each(|x| walk(x, objects));
        }
        Value::Array(items) => items.iter().for_each(|x| walk(x, objects)),
        _ => {}
    }
}

#[test]
fn schema_has_no_constraints_and_closed_objects() {
    let mut objects = 0;
    walk(&schema(), &mut objects);
    assert_eq!(objects, 2);
}

#[test]
fn schema_requires_every_property() {
    let schema = schema();
    let item = &schema["properties"]["results"]["items"];
    let mut required: Vec<&str> = item["required"]
        .as_array()
        .expect("required is an array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let mut props: Vec<&str> = item["properties"]
        .as_object()
        .expect("properties is an object")
        .keys()
        .map(String::as_str)
        .collect();
    required.sort_unstable();
    props.sort_unstable();
    assert_eq!(required, props);
}
