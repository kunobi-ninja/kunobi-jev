//! The `systemone` request.

use serde::Serialize;
use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::questions::Questions;
use crate::types::Entry;

/// Top-level request fields that extra properties may not shadow.
const RESERVED_FIELDS: [&str; 3] = ["state", "questions", "model"];

/// State and named questions for [`Client::system_one`](crate::Client::system_one).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SystemOneRequest {
    /// Text, a JSON object or array, or null to evaluate.
    pub state: Entry,
    /// Non-empty questions keyed by the names used to identify their answers.
    pub questions: Questions,
    /// Model override; `None` uses the client's default model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Additional top-level properties, forwarded as given.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl SystemOneRequest {
    /// A request with the client's default model.
    pub fn new(state: impl Into<Entry>, questions: Questions) -> Self {
        Self {
            state: state.into(),
            questions,
            model: None,
            extra: Map::new(),
        }
    }

    /// Use a specific model for this request.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Forward an additional top-level property.
    pub fn extra(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.extra.insert(key.into(), value.into());
        self
    }

    /// Validate the request and serialize it with the model resolved.
    pub(crate) fn to_body(&self, default_model: &str) -> Result<Vec<u8>> {
        self.questions.validate()?;
        if let Some(key) = RESERVED_FIELDS
            .iter()
            .find(|field| self.extra.contains_key(**field))
        {
            return Err(Error::InvalidRequest(format!(
                "Extra property \"{key}\" conflicts with a request field; set it directly."
            )));
        }

        #[derive(Serialize)]
        struct Payload<'a> {
            state: &'a Entry,
            questions: &'a Questions,
            model: &'a str,
            #[serde(flatten)]
            extra: &'a Map<String, Value>,
        }

        serde_json::to_vec(&Payload {
            state: &self.state,
            questions: &self.questions,
            model: self.model.as_deref().unwrap_or(default_model),
            extra: &self.extra,
        })
        .map_err(|err| Error::InvalidRequest(format!("Could not serialize the request: {err}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::questions::noul;
    use serde_json::json;

    fn questions() -> Questions {
        let mut questions = Questions::new();
        questions.add("urgent", noul("Urgent?"));
        questions
    }

    fn body(request: &SystemOneRequest) -> Value {
        serde_json::from_slice(&request.to_body("jev-latest").unwrap()).unwrap()
    }

    #[test]
    fn resolves_the_default_model() {
        let request = SystemOneRequest::new("Help ASAP", questions());
        assert_eq!(
            body(&request),
            json!({
                "state": "Help ASAP",
                "questions": {"urgent": {"type": "noul", "instructions": "Urgent?"}},
                "model": "jev-latest"
            })
        );
    }

    #[test]
    fn model_override_and_extra_properties_are_sent() {
        let request = SystemOneRequest::new(Entry::Null, questions())
            .model("jev-2")
            .extra("seed", 7)
            .extra("tag", Value::Null);
        let sent = body(&request);
        assert_eq!(sent["model"], "jev-2");
        assert_eq!(sent["seed"], 7);
        assert_eq!(sent["tag"], Value::Null);
        assert_eq!(sent["state"], Value::Null);
    }

    #[test]
    fn extra_properties_cannot_shadow_request_fields() {
        for field in RESERVED_FIELDS {
            let request = SystemOneRequest::new("s", questions()).extra(field, "x");
            let err = request.to_body("m").unwrap_err();
            assert!(err.to_string().contains(field), "{err}");
        }
    }

    #[test]
    fn invalid_questions_are_rejected_before_sending() {
        let request = SystemOneRequest::new("s", Questions::new());
        assert!(matches!(
            request.to_body("m"),
            Err(Error::InvalidRequest(_))
        ));
    }
}
