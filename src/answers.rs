//! Answer types and the `systemone` result.

use std::collections::BTreeMap;

use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::questions::AnswerKey;
use crate::types::Usage;

/// A yes/no answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoulAnswer {
    /// Probability of yes, from 0 to 1.
    pub noul: f64,
}

/// A selected label with its probabilities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChoiceAnswer {
    /// The selected label.
    pub choice: String,
    /// Reported confidence in the selected label.
    pub confidence: f64,
    /// Probabilities keyed by label.
    #[serde(default)]
    pub probabilities: IndexMap<String, f64>,
}

impl ChoiceAnswer {
    /// The probability of a label.
    pub fn probability(&self, label: &str) -> Option<f64> {
        self.probabilities.get(label).copied()
    }
}

/// An expected score with its rubric and probabilities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoreAnswer {
    /// Expected score, which may fall between rubric levels.
    pub score: f64,
    /// Reported confidence in the score.
    pub confidence: f64,
    /// Rubric descriptions keyed by score.
    #[serde(default)]
    pub legend: BTreeMap<u32, Value>,
    /// Probabilities keyed by score.
    #[serde(default)]
    pub probabilities: BTreeMap<u32, f64>,
}

/// An answer, tagged by `type` on the wire.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    /// A yes/no answer.
    Noul(NoulAnswer),
    /// A selected label.
    Choice(ChoiceAnswer),
    /// An expected score.
    Score(ScoreAnswer),
    /// An answer type this crate does not know yet, kept as raw JSON.
    #[serde(untagged)]
    Unknown(Value),
}

impl<'de> Deserialize<'de> for Answer {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        use serde::de::Error as _;

        // Known types must parse strictly; only unknown types fall back to raw JSON.
        let value = Value::deserialize(deserializer)?;
        let parsed = match value.get("type").and_then(Value::as_str) {
            Some("noul") => serde_json::from_value(value).map(Answer::Noul),
            Some("choice") => serde_json::from_value(value).map(Answer::Choice),
            Some("score") => serde_json::from_value(value).map(Answer::Score),
            _ => return Ok(Answer::Unknown(value)),
        };
        parsed.map_err(D::Error::custom)
    }
}

/// An answer type that can be read back through an [`AnswerKey`].
pub trait AnswerKind: Sized {
    /// The wire `type` of this answer.
    const TYPE: &'static str;

    /// This answer, if `answer` has this type.
    fn from_answer(answer: &Answer) -> Option<&Self>;
}

impl AnswerKind for NoulAnswer {
    const TYPE: &'static str = "noul";

    fn from_answer(answer: &Answer) -> Option<&Self> {
        match answer {
            Answer::Noul(noul) => Some(noul),
            _ => None,
        }
    }
}

impl AnswerKind for ChoiceAnswer {
    const TYPE: &'static str = "choice";

    fn from_answer(answer: &Answer) -> Option<&Self> {
        match answer {
            Answer::Choice(choice) => Some(choice),
            _ => None,
        }
    }
}

impl AnswerKind for ScoreAnswer {
    const TYPE: &'static str = "score";

    fn from_answer(answer: &Answer) -> Option<&Self> {
        match answer {
            Answer::Score(score) => Some(score),
            _ => None,
        }
    }
}

/// Answers keyed by question name, with model and usage metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemOneResult {
    /// The model that answered the request.
    pub model: String,
    /// Answers in the order the server returned them.
    pub answers: IndexMap<String, Answer>,
    /// Token usage for the request.
    #[serde(default)]
    pub usage: Usage,
}

impl SystemOneResult {
    /// The typed answer for a key returned by [`Questions::add`](crate::Questions::add).
    pub fn answer<A: AnswerKind>(&self, key: &AnswerKey<A>) -> Result<&A> {
        self.answers
            .get(key.name())
            .and_then(A::from_answer)
            .ok_or_else(|| Error::MissingAnswer {
                name: key.name().to_owned(),
                expected: A::TYPE,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn quickstart_response() -> Value {
        json!({
            "model": "jev-latest",
            "answers": {
                "department": {
                    "type": "choice",
                    "choice": "technical",
                    "probabilities": {"billing": 0.159, "technical": 0.84, "sales": 0.001},
                    "confidence": 0.596
                },
                "frustration": {
                    "type": "score",
                    "score": 1.035,
                    "legend": {"0": "Calm", "1": "Frustrated", "2": "Angry"},
                    "confidence": 0.842
                },
                "is_urgent": {"type": "noul", "noul": 0.999}
            },
            "usage": {"input_tokens": 312, "output_tokens": 48}
        })
    }

    #[test]
    fn parses_the_documented_response() {
        let result: SystemOneResult = serde_json::from_value(quickstart_response()).unwrap();
        assert_eq!(result.model, "jev-latest");
        assert_eq!(result.usage.input_tokens, 312);

        let department = result
            .answer(&AnswerKey::<ChoiceAnswer>::new("department"))
            .unwrap();
        assert_eq!(department.choice, "technical");
        assert_eq!(department.probability("billing"), Some(0.159));
        let labels: Vec<_> = department.probabilities.keys().collect();
        assert_eq!(labels, ["billing", "technical", "sales"]);

        let frustration = result
            .answer(&AnswerKey::<ScoreAnswer>::new("frustration"))
            .unwrap();
        assert_eq!(frustration.legend[&2], json!("Angry"));
        assert!(frustration.probabilities.is_empty());

        let urgent = result
            .answer(&AnswerKey::<NoulAnswer>::new("is_urgent"))
            .unwrap();
        assert_eq!(urgent.noul, 0.999);
    }

    #[test]
    fn wrong_type_or_missing_name_is_an_error() {
        let result: SystemOneResult = serde_json::from_value(quickstart_response()).unwrap();
        let err = result
            .answer(&AnswerKey::<NoulAnswer>::new("department"))
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "The response has no noul answer named \"department\"."
        );
        assert!(
            result
                .answer(&AnswerKey::<NoulAnswer>::new("nope"))
                .is_err()
        );
    }

    #[test]
    fn unknown_answer_types_are_kept_raw() {
        let answer: Answer =
            serde_json::from_value(json!({"type": "rank", "order": [1, 2]})).unwrap();
        assert_eq!(
            answer,
            Answer::Unknown(json!({"type": "rank", "order": [1, 2]}))
        );
        assert_eq!(
            serde_json::to_value(&answer).unwrap(),
            json!({"type": "rank", "order": [1, 2]})
        );
    }

    #[test]
    fn malformed_known_answers_fail_instead_of_falling_back() {
        let bad = serde_json::from_value::<Answer>(json!({"type": "choice", "choice": "a"}));
        assert!(bad.is_err());
    }

    #[test]
    fn answers_serialize_with_their_type_tag() {
        let answer = Answer::Noul(NoulAnswer { noul: 0.5 });
        assert_eq!(
            serde_json::to_value(&answer).unwrap(),
            json!({"type": "noul", "noul": 0.5})
        );
        let score = Answer::Score(ScoreAnswer {
            score: 1.0,
            confidence: 0.9,
            legend: [(0, json!("low")), (1, json!("high"))].into(),
            probabilities: [(0, 0.1), (1, 0.9)].into(),
        });
        let json = serde_json::to_value(&score).unwrap();
        assert_eq!(json["legend"], json!({"0": "low", "1": "high"}));
        assert_eq!(serde_json::from_value::<Answer>(json).unwrap(), score);
    }
}
