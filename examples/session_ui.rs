// Copyright (c) 2026 Elias Bachaalany
// SPDX-License-Identifier: MIT

//! Demonstrates the SessionUi handle for driving interactive dialogs back
//! to the CLI host (confirm/select/input/elicitation).
//!
//! Requires a CLI host that reports `capabilities.ui.elicitation == true`.

use copilot_sdk::{Client, InputOptions, Result, SessionConfig};

#[tokio::main]
async fn main() -> Result<()> {
    let client = Client::builder().build()?;
    client.start().await?;

    let session = client
        .create_session(SessionConfig {
            request_elicitation: Some(true),
            ..Default::default()
        })
        .await?;

    let ui = session.ui();

    if ui.confirm("Continue with the deployment?").await? {
        println!("Confirmed!");
    }

    if let Some(env) = ui
        .select("Pick an environment", &["staging", "production"])
        .await?
    {
        println!("Selected: {env}");
    }

    if let Some(name) = ui
        .input(
            "What's your name?",
            Some(&InputOptions {
                title: Some("Name".into()),
                description: Some("Used for the welcome banner".into()),
                min_length: Some(1),
                max_length: Some(80),
                ..Default::default()
            }),
        )
        .await?
    {
        println!("Hello, {name}!");
    }

    session.disconnect().await?;
    client.stop().await;
    Ok(())
}
