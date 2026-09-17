//! Values shared by requests and responses.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::Error;

/// Text, a JSON object, a JSON array, or `null`.
///
/// State, instructions and criteria descriptions all take this shape. Numbers
/// and booleans are not valid on their own; wrap them in an object.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Entry {
    /// No value. For a criterion, this leaves the label undescribed.
    #[default]
    Null,
    /// Plain text.
    Text(String),
    /// A JSON object.
    Object(Map<String, Value>),
    /// A JSON array.
    Array(Vec<Value>),
}

impl Entry {
    /// Serialize any value into an entry.
    ///
    /// Fails when the value serializes to a number or a boolean.
    pub fn from_serialize<T: Serialize + ?Sized>(value: &T) -> Result<Self, Error> {
        let value = serde_json::to_value(value)
            .map_err(|err| Error::InvalidRequest(format!("Could not serialize entry: {err}")))?;
        Self::try_from(value)
    }

    /// Whether this entry is `null`.
    pub fn is_null(&self) -> bool {
        matches!(self, Entry::Null)
    }
}

impl From<&str> for Entry {
    fn from(text: &str) -> Self {
        Entry::Text(text.to_owned())
    }
}

impl From<String> for Entry {
    fn from(text: String) -> Self {
        Entry::Text(text)
    }
}

impl From<&String> for Entry {
    fn from(text: &String) -> Self {
        Entry::Text(text.clone())
    }
}

impl From<Map<String, Value>> for Entry {
    fn from(object: Map<String, Value>) -> Self {
        Entry::Object(object)
    }
}

impl From<Vec<Value>> for Entry {
    fn from(array: Vec<Value>) -> Self {
        Entry::Array(array)
    }
}

impl<T: Into<Entry>> From<Option<T>> for Entry {
    fn from(value: Option<T>) -> Self {
        value.map_or(Entry::Null, Into::into)
    }
}

impl TryFrom<Value> for Entry {
    type Error = Error;

    fn try_from(value: Value) -> Result<Self, Error> {
        match value {
            Value::Null => Ok(Entry::Null),
            Value::String(text) => Ok(Entry::Text(text)),
            Value::Object(object) => Ok(Entry::Object(object)),
            Value::Array(array) => Ok(Entry::Array(array)),
            other => Err(Error::InvalidRequest(format!(
                "An entry must be text, a JSON object, a JSON array, or null, got {other}."
            ))),
        }
    }
}

impl From<Entry> for Value {
    fn from(entry: Entry) -> Self {
        match entry {
            Entry::Null => Value::Null,
            Entry::Text(text) => Value::String(text),
            Entry::Object(object) => Value::Object(object),
            Entry::Array(array) => Value::Array(array),
        }
    }
}

/// Token usage for a request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Number of input tokens used.
    #[serde(default)]
    pub input_tokens: u64,
    /// Number of output tokens used.
    #[serde(default)]
    pub output_tokens: u64,
}

/// Metadata for an available model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCard {
    /// Model name to pass as `model`, such as `jev-latest`.
    pub name: String,
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
    /// Release date as the API reports it.
    #[serde(default)]
    pub release_date: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn entries_serialize_to_their_json_shape() {
        assert_eq!(serde_json::to_value(Entry::Null).unwrap(), json!(null));
        assert_eq!(
            serde_json::to_value(Entry::from("hi")).unwrap(),
            json!("hi")
        );
        let object = Entry::try_from(json!({"a": 1})).unwrap();
        assert_eq!(serde_json::to_value(object).unwrap(), json!({"a": 1}));
        let array = Entry::try_from(json!([1, "b"])).unwrap();
        assert_eq!(serde_json::to_value(array).unwrap(), json!([1, "b"]));
    }

    #[test]
    fn entries_reject_bare_numbers_and_booleans() {
        assert!(Entry::try_from(json!(1)).is_err());
        assert!(Entry::try_from(json!(true)).is_err());
        assert!(Entry::from_serialize(&3.5).is_err());
    }

    #[test]
    fn entries_accept_serializable_structs() {
        #[derive(Serialize)]
        struct Ticket {
            subject: &'static str,
        }
        let entry = Entry::from_serialize(&Ticket { subject: "refund" }).unwrap();
        assert_eq!(Value::from(entry), json!({"subject": "refund"}));
    }

    #[test]
    fn object_entries_keep_key_order() {
        let entry = Entry::try_from(json!({"zeta": 1, "alpha": 2, "mid": 3})).unwrap();
        assert_eq!(
            serde_json::to_string(&entry).unwrap(),
            r#"{"zeta":1,"alpha":2,"mid":3}"#
        );
    }

    #[test]
    fn options_map_none_to_null() {
        assert_eq!(Entry::from(None::<&str>), Entry::Null);
        assert_eq!(Entry::from(Some("x")), Entry::Text("x".into()));
    }

    #[test]
    fn entries_deserialize_from_json() {
        let entries: Vec<Entry> =
            serde_json::from_value(json!([null, "t", {"k": 1}, [1]])).unwrap();
        assert!(entries[0].is_null());
        assert_eq!(entries[1], Entry::Text("t".into()));
        assert!(matches!(entries[2], Entry::Object(_)));
        assert!(matches!(entries[3], Entry::Array(_)));
    }
}
