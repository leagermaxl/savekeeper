//! System prompt, prompt version and response JSON schema (SPEC-08 §4.3, §4.5).

/// Version of the system prompt and response schema (SPEC-08 §4.3).
///
/// Part of the cache key (§4.7): any change of [`SYSTEM_PROMPT`] or
/// [`RESPONSE_SCHEMA`] must bump it, which invalidates the cache.
pub const PROMPT_VERSION: &str = "v1";

/// System prompt for folder classification, `prompts/classify_v1.txt` (SPEC-08 §4.3).
pub const SYSTEM_PROMPT: &str = include_str!("../prompts/classify_v1.txt");

/// JSON schema of the model response (SPEC-08 §4.5).
///
/// It has no numeric or string constraints (`minimum`, `maximum`, `maxLength`):
/// Anthropic structured outputs do not support them, so they are checked on the
/// client (§4.6). Every object has `additionalProperties: false`. The `category`
/// enum matches the `snake_case` serde names of
/// [`Category`](sk_core::model::Category).
pub const RESPONSE_SCHEMA: &str = r#"{
  "type": "object",
  "additionalProperties": false,
  "required": ["results"],
  "properties": {
    "results": {
      "type": "array",
      "items": {
        "type": "object",
        "additionalProperties": false,
        "required": ["id", "category", "app_guess", "importance", "confidence", "reason",
                     "suggested_include", "suggested_exclude"],
        "properties": {
          "id":         { "type": "string" },
          "category":   { "type": "string", "enum": ["game_save","game_config","app_config","app_data","user_files",
                                                     "dev_environment","credentials","system_settings",
                                                     "reinstallable","cache","unknown"] },
          "app_guess":  { "anyOf": [{ "type": "string" }, { "type": "null" }] },
          "importance": { "type": "number" },
          "confidence": { "type": "number" },
          "reason":     { "type": "string" },
          "suggested_include": { "type": "array", "items": { "type": "string" } },
          "suggested_exclude": { "type": "array", "items": { "type": "string" } }
        }
      }
    }
  }
}"#;

#[cfg(test)]
#[path = "prompt_tests.rs"]
mod tests;
