//! Answer types and the `systemone` result.

use std::collections::BTreeMap;

use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::labels::Labels;
use crate::questions::AnswerKey;
use crate::types::Usage;

/// A yes/no answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoulAnswer {
    /// Probability of yes, from 0 to 1.
    pub noul: f64,
}

impl NoulAnswer {
    /// Whether the probability of yes reaches `threshold`.
    pub fn is_yes(&self, threshold: f64) -> bool {
        self.noul >= threshold
    }
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

    /// The selected label, if confidence reaches `min_confidence`.
    ///
    /// Use it to act on clear answers and route the rest elsewhere.
    pub fn decide(&self, min_confidence: f64) -> Option<&str> {
        (self.confidence >= min_confidence).then_some(self.choice.as_str())
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

impl ScoreAnswer {
    /// The expected score, if confidence reaches `min_confidence`.
    pub fn decide(&self, min_confidence: f64) -> Option<f64> {
        (self.confidence >= min_confidence).then_some(self.score)
    }

    /// The level with the highest probability, or `None` when the response has no probabilities.
    ///
    /// Ties go to the lower level.
    pub fn most_likely_level(&self) -> Option<u32> {
        self.probabilities
            .iter()
            .fold(None, |best: Option<(u32, f64)>, (&level, &p)| match best {
                Some((_, best_p)) if best_p >= p => best,
                _ => Some((level, p)),
            })
            .map(|(level, _)| level)
    }
}

/// A choice answer whose labels are a [`Labels`] enum.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedChoiceAnswer<L> {
    /// The selected label.
    pub choice: L,
    /// Reported confidence in the selected label.
    pub confidence: f64,
    /// Probabilities by label, in the order the server returned them.
    pub probabilities: Vec<(L, f64)>,
}

impl<L: Labels> TypedChoiceAnswer<L> {
    /// The probability of a label, or `0.0` when the response did not include it.
    pub fn probability(&self, label: L) -> f64 {
        self.probabilities
            .iter()
            .find(|(candidate, _)| *candidate == label)
            .map_or(0.0, |(_, p)| *p)
    }

    /// The selected label, if confidence reaches `min_confidence`.
    pub fn decide(&self, min_confidence: f64) -> Option<L> {
        (self.confidence >= min_confidence).then_some(self.choice)
    }
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

impl Answer {
    /// The wire `type` of this answer.
    pub fn type_name(&self) -> &str {
        match self {
            Answer::Noul(_) => NoulAnswer::TYPE,
            Answer::Choice(_) => ChoiceAnswer::TYPE,
            Answer::Score(_) => ScoreAnswer::TYPE,
            Answer::Unknown(value) => value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
        }
    }
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

    /// Convert a response answer, or explain why it doesn't fit.
    fn from_answer(answer: &Answer) -> std::result::Result<Self, String>;
}

fn wrong_type(expected: &str, answer: &Answer) -> String {
    format!("expected a {expected} answer, got {}", answer.type_name())
}

impl AnswerKind for NoulAnswer {
    const TYPE: &'static str = "noul";

    fn from_answer(answer: &Answer) -> std::result::Result<Self, String> {
        match answer {
            Answer::Noul(noul) => Ok(noul.clone()),
            other => Err(wrong_type(Self::TYPE, other)),
        }
    }
}

impl AnswerKind for ChoiceAnswer {
    const TYPE: &'static str = "choice";

    fn from_answer(answer: &Answer) -> std::result::Result<Self, String> {
        match answer {
            Answer::Choice(choice) => Ok(choice.clone()),
            other => Err(wrong_type(Self::TYPE, other)),
        }
    }
}

impl AnswerKind for ScoreAnswer {
    const TYPE: &'static str = "score";

    fn from_answer(answer: &Answer) -> std::result::Result<Self, String> {
        match answer {
            Answer::Score(score) => Ok(score.clone()),
            other => Err(wrong_type(Self::TYPE, other)),
        }
    }
}

impl<L: Labels> AnswerKind for TypedChoiceAnswer<L> {
    const TYPE: &'static str = "choice";

    fn from_answer(answer: &Answer) -> std::result::Result<Self, String> {
        let Answer::Choice(answer) = answer else {
            return Err(wrong_type(Self::TYPE, answer));
        };
        let known = |label: &str| {
            L::from_label(label).ok_or_else(|| {
                let expected: Vec<_> = L::ALL.iter().map(|(_, label, _)| *label).collect();
                format!("label \"{label}\" is not one of {}", expected.join(", "))
            })
        };
        Ok(Self {
            choice: known(&answer.choice)?,
            confidence: answer.confidence,
            probabilities: answer
                .probabilities
                .iter()
                .map(|(label, p)| Ok((known(label)?, *p)))
                .collect::<std::result::Result<_, String>>()?,
        })
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
    ///
    /// Fails with [`Error::UnexpectedAnswer`] when the answer is missing, has another
    /// type, or uses a label the key's [`Labels`] enum doesn't know.
    pub fn answer<A: AnswerKind>(&self, key: &AnswerKey<A>) -> Result<A> {
        let unexpected = |reason: String| Error::UnexpectedAnswer {
            name: key.name().to_owned(),
            reason,
        };
        let answer = self
            .answers
            .get(key.name())
            .ok_or_else(|| unexpected(format!("no {} answer in the response", A::TYPE)))?;
        A::from_answer(answer).map_err(unexpected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    crate::labels! {
        enum Department {
            Billing = "billing",
            Technical = "technical",
            Sales = "sales",
        }
    }

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

    fn result() -> SystemOneResult {
        serde_json::from_value(quickstart_response()).unwrap()
    }

    #[test]
    fn parses_the_documented_response() {
        let result = result();
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
        assert_eq!(frustration.most_likely_level(), None);

        let urgent = result
            .answer(&AnswerKey::<NoulAnswer>::new("is_urgent"))
            .unwrap();
        assert_eq!(urgent.noul, 0.999);
    }

    #[test]
    fn typed_choices_map_labels_to_the_enum() {
        let department = result()
            .answer(&AnswerKey::<TypedChoiceAnswer<Department>>::new(
                "department",
            ))
            .unwrap();
        assert_eq!(department.choice, Department::Technical);
        assert_eq!(department.probability(Department::Billing), 0.159);
        let order: Vec<_> = department.probabilities.iter().map(|(l, _)| *l).collect();
        assert_eq!(
            order,
            [
                Department::Billing,
                Department::Technical,
                Department::Sales
            ]
        );
    }

    #[test]
    fn typed_choices_reject_unknown_labels() {
        crate::labels! {
            enum Narrow {
                Billing = "billing",
                Technical = "technical",
            }
        }
        let err = result()
            .answer(&AnswerKey::<TypedChoiceAnswer<Narrow>>::new("department"))
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Unexpected answer \"department\": label \"sales\" is not one of billing, technical."
        );
    }

    #[test]
    fn wrong_type_or_missing_name_is_an_error() {
        let result = result();
        let err = result
            .answer(&AnswerKey::<NoulAnswer>::new("department"))
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Unexpected answer \"department\": expected a noul answer, got choice."
        );
        let err = result
            .answer(&AnswerKey::<NoulAnswer>::new("nope"))
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Unexpected answer \"nope\": no noul answer in the response."
        );
    }

    #[test]
    fn confidence_helpers_gate_on_thresholds() {
        let result = result();
        let department = result
            .answer(&AnswerKey::<TypedChoiceAnswer<Department>>::new(
                "department",
            ))
            .unwrap();
        assert_eq!(department.decide(0.5), Some(Department::Technical));
        assert_eq!(department.decide(0.596), Some(Department::Technical));
        assert_eq!(department.decide(0.6), None);

        let raw = result
            .answer(&AnswerKey::<ChoiceAnswer>::new("department"))
            .unwrap();
        assert_eq!(raw.decide(0.5), Some("technical"));
        assert_eq!(raw.decide(0.9), None);

        let score = result
            .answer(&AnswerKey::<ScoreAnswer>::new("frustration"))
            .unwrap();
        assert_eq!(score.decide(0.8), Some(1.035));
        assert_eq!(score.decide(0.9), None);

        let urgent = result
            .answer(&AnswerKey::<NoulAnswer>::new("is_urgent"))
            .unwrap();
        assert!(urgent.is_yes(0.999));
        assert!(!urgent.is_yes(0.9991));
    }

    #[test]
    fn most_likely_level_prefers_the_highest_then_the_lowest_on_ties() {
        let score = |probabilities: &[(u32, f64)]| ScoreAnswer {
            score: 0.0,
            confidence: 0.0,
            legend: BTreeMap::new(),
            probabilities: probabilities.iter().copied().collect(),
        };
        assert_eq!(
            score(&[(0, 0.2), (1, 0.7), (2, 0.1)]).most_likely_level(),
            Some(1)
        );
        assert_eq!(
            score(&[(0, 0.4), (1, 0.2), (2, 0.4)]).most_likely_level(),
            Some(0)
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
        assert_eq!(answer.type_name(), "rank");
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
