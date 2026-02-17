use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(i64),
    String(String),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
}

impl Value {
    pub fn is_scalar(&self) -> bool {
        matches!(
            self,
            Self::Null | Self::Bool(_) | Self::Number(_) | Self::String(_)
        )
    }

    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "boolean",
            Self::Number(_) => "number",
            Self::String(_) => "string",
            Self::Array(_) => "array",
            Self::Object(_) => "object",
        }
    }

    pub fn render_scalar(&self) -> Option<String> {
        match self {
            Self::Null => Some(String::new()),
            Self::Bool(value) => Some(value.to_string()),
            Self::Number(value) => Some(value.to_string()),
            Self::String(value) => Some(value.clone()),
            Self::Array(_) | Self::Object(_) => None,
        }
    }

    pub fn render_scalar_array(&self) -> Option<Vec<String>> {
        let Self::Array(values) = self else {
            return None;
        };

        values.iter().map(Self::render_scalar).collect()
    }

    pub fn to_json_string(&self) -> String {
        match self {
            Self::Null => "null".to_string(),
            Self::Bool(value) => value.to_string(),
            Self::Number(value) => value.to_string(),
            Self::String(value) => {
                let mut out = String::with_capacity(value.len() + 2);
                out.push('"');
                out.push_str(&escape_json(value));
                out.push('"');
                out
            }
            Self::Array(values) => {
                let mut out = String::from("[");
                for (idx, value) in values.iter().enumerate() {
                    if idx > 0 {
                        out.push(',');
                    }
                    out.push_str(&value.to_json_string());
                }
                out.push(']');
                out
            }
            Self::Object(map) => {
                let mut out = String::from("{");
                for (idx, (key, value)) in map.iter().enumerate() {
                    if idx > 0 {
                        out.push(',');
                    }
                    out.push('"');
                    out.push_str(&escape_json(key));
                    out.push_str("\":");
                    out.push_str(&value.to_json_string());
                }
                out.push('}');
                out
            }
        }
    }
}

fn escape_json(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            ch if ch.is_control() => {
                let code = ch as u32;
                out.push_str(&format!("\\u{code:04x}"));
            }
            _ => out.push(ch),
        }
    }
    out
}
