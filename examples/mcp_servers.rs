// Copyright (c) 2026 Elias Bachaalany
// SPDX-License-Identifier: MIT

//! MCP servers example demonstrating MCP server configuration.

use copilot_sdk::{Client, McpLocalServerConfig, SessionConfig, SessionEventData};
use std::collections::HashMap;
use std::io::{self, Write};

#[tokio::main]
async fn main() -> copilot_sdk::Result<()> {
    println!("=== MCP Servers Example ===\n");

    let client = Client::builder().use_stdio(true).build()?;
    client.start().await?;

    // Configure filesystem MCP server
    let filesystem_server = McpLocalServerConfig {
        tools: vec!["*".to_string()],
        command: "npx".to_string(),
        args: vec![
            "-y".to_string(),
            "@anthropic/mcp-server-filesystem".to_string(),
            "/tmp".to_string(),
        ],
        server_type: Some("local".to_string()),
        timeout: Some(30000),
        env: None,
        cwd: None,
    };

    let mut mcp_servers: HashMap<String, serde_json::Value> = HashMap::new();
    mcp_servers.insert(
        "filesystem".to_string(),
        serde_json::to_value(&filesystem_server).unwrap(),
    );

    let config = SessionConfig {
        mcp_servers: Some(mcp_servers),
        ..Default::default()
    };

    println!("Configured MCP server: filesystem");
    println!("Note: Requires @anthropic/mcp-server-filesystem\n");

    let session = client.create_session(config).await?;
    let mut events = session.subscribe();

    session.send("List files in /tmp").await?;

    print!("Assistant: ");
    io::stdout().flush().unwrap();
    while let Ok(event) = events.recv().await {
        match &event.data {
            SessionEventData::AssistantMessageDelta(d) => {
                print!("{}", d.delta_content);
                io::stdout().flush().unwrap();
            }
            SessionEventData::ToolExecutionStart(t) => {
                println!("\n[MCP Tool: {}]", t.tool_name);
            }
            SessionEventData::SessionIdle(_) => {
                println!();
                break;
            }
            _ => {}
        }
    }

    client.stop().await;
    Ok(())
}
