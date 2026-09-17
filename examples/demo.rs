//! Run with `cargo run --example demo`. Needs TYPESAFE_API_KEY in the environment.

use kunobi_jev::{Client, Entry, Error, Questions, SystemOneRequest, choice_labels, noul, score};
use serde_json::json;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = Client::new()?;

    let models = client.models().list().await?;
    let names: Vec<_> = models.iter().map(|m| m.name.as_str()).collect();
    println!("Available models: {}", names.join(", "));

    let ticket = Entry::try_from(json!({
        "subject": "Charged twice this month",
        "body": "Hi, I see two charges of $49 on my card for August. I only have one account. Please fix this ASAP, I'm pretty frustrated."
    }))?;

    let mut questions = Questions::new();
    let is_billing = questions.add("isBilling", noul("Is this ticket about billing?"));
    let sentiment = questions.add(
        "sentiment",
        choice_labels(
            "What is the customer's tone?",
            ["calm", "frustrated", "angry"],
        ),
    );
    let urgency = questions.add(
        "urgency",
        score(
            "How urgent is this ticket?",
            ["can wait", "this week", "today", "right now"],
        ),
    );
    let refund_risk = questions.add(
        "refundRisk",
        score(
            "How likely is the customer to demand a refund?",
            ["unlikely", "possible", "likely"],
        ),
    );

    let result = match client
        .system_one(SystemOneRequest::new(ticket, questions))
        .await
    {
        Ok(result) => result,
        Err(Error::Api(err)) => {
            eprintln!(
                "API error {} (request {}): {:?}",
                err.status(),
                err.request_id().unwrap_or("unknown"),
                err.body()
            );
            std::process::exit(1);
        }
        Err(err) => return Err(err.into()),
    };

    let sentiment = result.answer(&sentiment)?;
    let urgency = result.answer(&urgency)?;
    let refund_risk = result.answer(&refund_risk)?;
    println!("billing?     {:.2}", result.answer(&is_billing)?.noul);
    println!(
        "tone         {} ({:.2})",
        sentiment.choice,
        sentiment.probability(&sentiment.choice).unwrap_or_default()
    );
    println!(
        "urgency      {:.2} on a 0-3 scale: {:?}",
        urgency.score, urgency.legend
    );
    println!(
        "refund risk  {:.2} ({:.2} confidence)",
        refund_risk.score, refund_risk.confidence
    );
    println!(
        "tokens       {} in / {} out",
        result.usage.input_tokens, result.usage.output_tokens
    );
    Ok(())
}
