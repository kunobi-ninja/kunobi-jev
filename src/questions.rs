//! Question types, builders and typed answer keys.

use std::fmt;
use std::marker::PhantomData;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::answers::{AnswerKind, ChoiceAnswer, NoulAnswer, ScoreAnswer};
use crate::error::{Error, Result};
use crate::types::Entry;

/// A question, tagged by `type` on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    /// A yes/no question.
    Noul(NoulQuestion),
    /// A choice between labels.
    Choice(ChoiceQuestion),
    /// A score from an ordered rubric.
    Score(ScoreQuestion),
}

/// A yes/no question. The answer is the probability of yes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NoulQuestion {
    /// The question as text, a JSON object or array, or null.
    #[serde(default)]
    pub instructions: Entry,
    /// Optional descriptions of the yes and no outcomes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criteria: Option<NoulCriteria>,
}

/// Descriptions of the yes and no outcomes of a [`NoulQuestion`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NoulCriteria {
    /// Description of the yes outcome.
    #[serde(rename = "true", default, skip_serializing_if = "Option::is_none")]
    pub when_true: Option<Entry>,
    /// Description of the no outcome.
    #[serde(rename = "false", default, skip_serializing_if = "Option::is_none")]
    pub when_false: Option<Entry>,
}

/// A question that selects one of several named labels.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChoiceQuestion {
    /// The question as text, a JSON object or array, or null.
    #[serde(default)]
    pub instructions: Entry,
    /// Labels mapped to descriptions; [`Entry::Null`] leaves a label undescribed.
    pub criteria: IndexMap<String, Entry>,
}

/// A question that assigns a score from an ordered rubric.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScoreQuestion {
    /// The question as text, a JSON object or array, or null.
    #[serde(default)]
    pub instructions: Entry,
    /// At least two level descriptions, indexed by score from zero.
    pub criteria: Vec<Entry>,
}

impl NoulQuestion {
    /// Describe the yes outcome.
    pub fn when_true(mut self, description: impl Into<Entry>) -> Self {
        self.criteria.get_or_insert_default().when_true = Some(description.into());
        self
    }

    /// Describe the no outcome.
    pub fn when_false(mut self, description: impl Into<Entry>) -> Self {
        self.criteria.get_or_insert_default().when_false = Some(description.into());
        self
    }
}

/// Create a yes/no question.
///
/// Pass [`Entry::Null`] to ask without instructions.
pub fn noul(instructions: impl Into<Entry>) -> NoulQuestion {
    NoulQuestion {
        instructions: instructions.into(),
        criteria: None,
    }
}

/// Create a choice between labels, each with a description.
///
/// ```
/// use kunobi_jev::choice;
///
/// let q = choice("Which team should handle this?", [
///     ("billing", "Payment or subscription issues"),
///     ("technical", "Bugs or integration problems"),
/// ]);
/// assert_eq!(q.criteria.len(), 2);
/// ```
pub fn choice<K, V>(
    instructions: impl Into<Entry>,
    criteria: impl IntoIterator<Item = (K, V)>,
) -> ChoiceQuestion
where
    K: Into<String>,
    V: Into<Entry>,
{
    ChoiceQuestion {
        instructions: instructions.into(),
        criteria: criteria
            .into_iter()
            .map(|(label, description)| (label.into(), description.into()))
            .collect(),
    }
}

/// Create a choice between undescribed labels.
pub fn choice_labels<K: Into<String>>(
    instructions: impl Into<Entry>,
    labels: impl IntoIterator<Item = K>,
) -> ChoiceQuestion {
    choice(
        instructions,
        labels.into_iter().map(|label| (label, Entry::Null)),
    )
}

/// Create a score question from level descriptions, lowest score first.
pub fn score<V: Into<Entry>>(
    instructions: impl Into<Entry>,
    levels: impl IntoIterator<Item = V>,
) -> ScoreQuestion {
    ScoreQuestion {
        instructions: instructions.into(),
        criteria: levels.into_iter().map(Into::into).collect(),
    }
}

impl From<NoulQuestion> for Question {
    fn from(question: NoulQuestion) -> Self {
        Question::Noul(question)
    }
}

impl From<ChoiceQuestion> for Question {
    fn from(question: ChoiceQuestion) -> Self {
        Question::Choice(question)
    }
}

impl From<ScoreQuestion> for Question {
    fn from(question: ScoreQuestion) -> Self {
        Question::Score(question)
    }
}

/// A question type with a known answer type.
pub trait QuestionKind: Into<Question> {
    /// The answer this question produces.
    type Answer: AnswerKind;
}

impl QuestionKind for NoulQuestion {
    type Answer = NoulAnswer;
}

impl QuestionKind for ChoiceQuestion {
    type Answer = ChoiceAnswer;
}

impl QuestionKind for ScoreQuestion {
    type Answer = ScoreAnswer;
}

/// A handle to a named question that reads back its typed answer.
///
/// Returned by [`Questions::add`]; pass it to
/// [`SystemOneResult::answer`](crate::SystemOneResult::answer).
pub struct AnswerKey<A> {
    name: String,
    kind: PhantomData<fn() -> A>,
}

impl<A> AnswerKey<A> {
    /// A key for an answer by name, for questions added without [`Questions::add`].
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: PhantomData,
        }
    }

    /// The question name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl<A> Clone for AnswerKey<A> {
    fn clone(&self) -> Self {
        Self::new(self.name.clone())
    }
}

impl<A: AnswerKind> fmt::Debug for AnswerKey<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnswerKey")
            .field("name", &self.name)
            .field("kind", &A::TYPE)
            .finish()
    }
}

/// Questions keyed by the names used to identify their answers, in insertion order.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Questions(IndexMap<String, Question>);

impl Questions {
    /// An empty set of questions.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a question and get a key for its typed answer.
    ///
    /// A question with the same name is replaced.
    pub fn add<Q: QuestionKind>(
        &mut self,
        name: impl Into<String>,
        question: Q,
    ) -> AnswerKey<Q::Answer> {
        let name = name.into();
        self.0.insert(name.clone(), question.into());
        AnswerKey::new(name)
    }

    /// Insert a question, returning the one it replaced.
    pub fn insert(
        &mut self,
        name: impl Into<String>,
        question: impl Into<Question>,
    ) -> Option<Question> {
        self.0.insert(name.into(), question.into())
    }

    /// The question with this name.
    pub fn get(&self, name: &str) -> Option<&Question> {
        self.0.get(name)
    }

    /// Number of questions.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are no questions.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Questions in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Question)> {
        self.0
            .iter()
            .map(|(name, question)| (name.as_str(), question))
    }

    /// Reject an empty set and score questions with fewer than two levels.
    pub fn validate(&self) -> Result<()> {
        if self.0.is_empty() {
            return Err(Error::InvalidRequest(
                "At least one question is required.".into(),
            ));
        }
        for (name, question) in &self.0 {
            if let Question::Score(score) = question
                && score.criteria.len() < 2
            {
                return Err(Error::InvalidRequest(format!(
                    "Score question \"{name}\" has {} criteria; at least two scores are required.",
                    score.criteria.len()
                )));
            }
        }
        Ok(())
    }
}

impl<K: Into<String>, Q: Into<Question>> FromIterator<(K, Q)> for Questions {
    fn from_iter<I: IntoIterator<Item = (K, Q)>>(iter: I) -> Self {
        Self(
            iter.into_iter()
                .map(|(name, question)| (name.into(), question.into()))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builders_serialize_to_the_wire_shape() {
        let mut questions = Questions::new();
        questions.add("urgent", noul("Does this message express urgency?"));
        questions.add("bare", noul(Entry::Null));
        questions.add(
            "described",
            noul("Refund?").when_true("asks for money back"),
        );
        questions.add(
            "department",
            choice(
                "Which team?",
                [("billing", Some("Payments")), ("other", None)],
            ),
        );
        questions.add("tone", choice_labels("Tone?", ["calm", "angry"]));
        questions.add(
            "frustration",
            score("How frustrated?", ["calm", "civil", "angry"]),
        );

        assert_eq!(
            serde_json::to_value(&questions).unwrap(),
            json!({
                "urgent": {"type": "noul", "instructions": "Does this message express urgency?"},
                "bare": {"type": "noul", "instructions": null},
                "described": {
                    "type": "noul",
                    "instructions": "Refund?",
                    "criteria": {"true": "asks for money back"}
                },
                "department": {
                    "type": "choice",
                    "instructions": "Which team?",
                    "criteria": {"billing": "Payments", "other": null}
                },
                "tone": {
                    "type": "choice",
                    "instructions": "Tone?",
                    "criteria": {"calm": null, "angry": null}
                },
                "frustration": {
                    "type": "score",
                    "instructions": "How frustrated?",
                    "criteria": ["calm", "civil", "angry"]
                }
            })
        );
    }

    #[test]
    fn preserves_insertion_order() {
        let questions: Questions = [("z", noul("z")), ("a", noul("a")), ("m", noul("m"))]
            .into_iter()
            .collect();
        let names: Vec<_> = questions.iter().map(|(name, _)| name).collect();
        assert_eq!(names, ["z", "a", "m"]);
        let wire = serde_json::to_string(&questions).unwrap();
        assert!(wire.find("\"z\"").unwrap() < wire.find("\"a\"").unwrap());
    }

    #[test]
    fn validation_rejects_empty_sets_and_short_rubrics() {
        let err = Questions::new().validate().unwrap_err();
        assert_eq!(err.to_string(), "At least one question is required.");

        let mut questions = Questions::new();
        questions.add("level", score("How?", ["only one"]));
        assert_eq!(
            questions.validate().unwrap_err().to_string(),
            "Score question \"level\" has 1 criteria; at least two scores are required."
        );

        let mut ok = Questions::new();
        ok.add("level", score("How?", ["low", "high"]));
        assert!(ok.validate().is_ok());
    }

    #[test]
    fn adding_a_duplicate_name_replaces_the_question() {
        let mut questions = Questions::new();
        questions.add("q", noul("first"));
        let key = questions.add("q", choice_labels("second", ["a", "b"]));
        assert_eq!(questions.len(), 1);
        assert!(matches!(
            questions.get(key.name()),
            Some(Question::Choice(_))
        ));
    }

    #[test]
    fn questions_round_trip_through_json() {
        let mut questions = Questions::new();
        questions.add("n", noul("x").when_false("no"));
        questions.add("s", score(Entry::Null, ["a", "b"]));
        let json = serde_json::to_value(&questions).unwrap();
        let back: Questions = serde_json::from_value(json).unwrap();
        assert_eq!(back, questions);
    }
}
