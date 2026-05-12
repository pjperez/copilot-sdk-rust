// Copyright (c) 2026 Elias Bachaalany
// SPDX-License-Identifier: MIT

//! Demonstrates registering an `exitPlanMode` and `autoModeSwitch` handler
//! on a session. The runtime calls these RPCs when the agent wants to leave
//! plan mode or switch into auto mode after a rate limit.

use copilot_sdk::{AutoModeSwitchResponse, Client, ExitPlanModeResult, Result, SessionConfig};
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    let client = Client::builder().build()?;
    client.start().await?;

    let session = client
        .create_session(SessionConfig {
            request_exit_plan_mode: Some(true),
            request_auto_mode_switch: Some(true),
            ..Default::default()
        })
        .await?;

    session
        .register_exit_plan_mode_handler(Arc::new(|req| {
            println!("Plan summary: {}", req.summary);
            ExitPlanModeResult {
                approved: true,
                selected_action: Some(req.recommended_action.clone()),
                feedback: None,
            }
        }))
        .await;

    session
        .register_auto_mode_switch_handler(Arc::new(|req| {
            println!(
                "Rate-limit retry-after = {:?}, errorCode = {:?}",
                req.retry_after_seconds, req.error_code
            );
            AutoModeSwitchResponse::YesAlways
        }))
        .await;

    session
        .send("Plan a trivial change and ask me to confirm before applying it.")
        .await?;

    session.disconnect().await?;
    client.stop().await;
    Ok(())
}
