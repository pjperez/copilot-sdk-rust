// Copyright (c) 2026 Elias Bachaalany
// SPDX-License-Identifier: MIT

//! Session management for the Copilot SDK.
//!
//! A session represents a conversation with the Copilot CLI.

use crate::error::{CopilotError, Result};
use crate::events::{SessionEvent, SessionEventData};
use crate::session_fs::{SessionFsError, SessionFsErrorCode, SharedSessionFsProvider};
use crate::types::{
    AutoModeSwitchHandler, AutoModeSwitchRequest, AutoModeSwitchResponse, CommandContext,
    CommandDefinition, CommandResult, ElicitationContext, ElicitationHandler, ElicitationParams,
    ElicitationResult, ErrorOccurredHookInput, ExitPlanModeHandler, ExitPlanModeRequest,
    ExitPlanModeResult, MessageOptions, PermissionRequest, PermissionRequestResult,
    PostToolUseHookInput, PreToolUseHookInput, SectionTransformFn, SessionCapabilities,
    SessionEndHookInput, SessionHooks, SessionStartHookInput, SessionUiCapabilities, Tool,
    ToolResultObject, UserInputInvocation, UserInputRequest, UserInputResponse,
    UserPromptSubmittedHookInput,
};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, RwLock};

// =============================================================================
// Event Handler Types
// =============================================================================

/// Handler for session events.
pub type EventHandler = Arc<dyn Fn(&SessionEvent) + Send + Sync>;

/// Handler for permission requests.
pub type PermissionHandler =
    Arc<dyn Fn(&PermissionRequest) -> PermissionRequestResult + Send + Sync>;

/// Handler for tool invocations.
pub type ToolHandler = Arc<dyn Fn(&str, &Value) -> ToolResultObject + Send + Sync>;

/// Handler for user input requests.
pub type UserInputHandler =
    Arc<dyn Fn(&UserInputRequest, &UserInputInvocation) -> UserInputResponse + Send + Sync>;

/// Type alias for the invoke future.
pub type InvokeFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value>> + Send>>;

type InvokeFn = dyn Fn(&str, Option<Value>) -> InvokeFuture + Send + Sync;

/// Factory that produces a per-session FS provider.
///
/// Mirrors Python's `CreateSessionFsHandler`. Receives the session id and
/// returns a fresh [`SharedSessionFsProvider`]; the SDK invokes the factory
/// after `session.create` / `session.resume` succeeds.
#[derive(Clone)]
pub struct CreateSessionFsHandler(Arc<dyn Fn(&str) -> SharedSessionFsProvider + Send + Sync>);

impl CreateSessionFsHandler {
    /// Wrap a closure into a `CreateSessionFsHandler`.
    pub fn new<F>(factory: F) -> Self
    where
        F: Fn(&str) -> SharedSessionFsProvider + Send + Sync + 'static,
    {
        Self(Arc::new(factory))
    }

    /// Invoke the factory for the given session id.
    pub fn call(&self, session_id: &str) -> SharedSessionFsProvider {
        (self.0)(session_id)
    }
}

impl std::fmt::Debug for CreateSessionFsHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CreateSessionFsHandler(fn)")
    }
}

fn err_to_json(err: &SessionFsError) -> Value {
    serde_json::json!({
        "code": match err.code {
            SessionFsErrorCode::NoEnt => "ENOENT",
            SessionFsErrorCode::Unknown => "UNKNOWN",
        },
        "message": err.message,
    })
}

// =============================================================================
// Event Subscription
// =============================================================================

/// A subscription to session events.
///
/// Events are delivered via the broadcast channel receiver.
pub struct EventSubscription {
    pub receiver: broadcast::Receiver<SessionEvent>,
}

impl EventSubscription {
    /// Receive the next event.
    pub async fn recv(&mut self) -> std::result::Result<SessionEvent, broadcast::error::RecvError> {
        self.receiver.recv().await
    }
}

// =============================================================================
// Registered Tool
// =============================================================================

/// A tool registered with the session, including its handler.
#[derive(Clone)]
pub struct RegisteredTool {
    /// Tool definition.
    pub tool: Tool,
    /// Handler for tool invocations.
    pub handler: Option<ToolHandler>,
}

// =============================================================================
// Session
// =============================================================================

/// Shared session state.
struct SessionState {
    /// Registered tools.
    tools: HashMap<String, RegisteredTool>,
    /// Permission handler.
    permission_handler: Option<PermissionHandler>,
    /// User input handler.
    user_input_handler: Option<UserInputHandler>,
    /// Session hooks.
    hooks: Option<SessionHooks>,
    /// Callback-based event handlers.
    event_handlers: HashMap<u64, EventHandler>,
    /// Next handler ID.
    next_handler_id: AtomicU64,
    /// Registered slash commands.
    commands: HashMap<String, CommandDefinition>,
    /// Elicitation handler.
    elicitation_handler: Option<ElicitationHandler>,
    /// Exit-plan-mode handler.
    exit_plan_mode_handler: Option<ExitPlanModeHandler>,
    /// Auto-mode-switch handler.
    auto_mode_switch_handler: Option<AutoModeSwitchHandler>,
    /// Section-id → transform callback for the customize-mode system message.
    transform_callbacks: HashMap<String, SectionTransformFn>,
    /// Optional inbound session-FS provider supplied by the host.
    session_fs_provider: Option<SharedSessionFsProvider>,
}

/// A Copilot conversation session.
///
/// Sessions maintain conversation state, handle events, and manage tool execution.
///
/// # Example
///
/// ```no_run
/// use copilot_sdk::{Client, SessionConfig, SessionEventData};
///
/// #[tokio::main]
/// async fn main() -> copilot_sdk::Result<()> {
/// let client = Client::builder().build()?;
/// client.start().await?;
/// let session = client.create_session(SessionConfig::default()).await?;
///
/// // Subscribe to events
/// let mut events = session.subscribe();
///
/// // Send a message
/// session.send("Hello!").await?;
///
/// // Process events
/// while let Ok(event) = events.recv().await {
///     match &event.data {
///         SessionEventData::AssistantMessage(msg) => println!("{}", msg.content),
///         SessionEventData::SessionIdle(_) => break,
///         _ => {}
///     }
/// }
/// client.stop().await;
/// # Ok(())
/// # }
/// ```
pub struct Session {
    /// Session ID.
    session_id: String,
    /// Workspace path for infinite sessions.
    workspace_path: Option<String>,
    /// Event broadcaster.
    event_tx: broadcast::Sender<SessionEvent>,
    /// Session state.
    state: Arc<RwLock<SessionState>>,
    /// JSON-RPC invoke function (injected by Client).
    invoke_fn: Arc<InvokeFn>,
    /// Capabilities reported by the runtime for this session.
    capabilities: Arc<RwLock<Option<SessionCapabilities>>>,
}

impl Session {
    /// Create a new session.
    ///
    /// This is typically called by the Client when creating a session.
    pub fn new<F>(session_id: String, workspace_path: Option<String>, invoke_fn: F) -> Self
    where
        F: Fn(&str, Option<Value>) -> InvokeFuture + Send + Sync + 'static,
    {
        let (event_tx, _) = broadcast::channel(1024);

        Self {
            session_id,
            workspace_path,
            event_tx,
            state: Arc::new(RwLock::new(SessionState {
                tools: HashMap::new(),
                permission_handler: None,
                user_input_handler: None,
                hooks: None,
                event_handlers: HashMap::new(),
                next_handler_id: AtomicU64::new(1),
                commands: HashMap::new(),
                elicitation_handler: None,
                exit_plan_mode_handler: None,
                auto_mode_switch_handler: None,
                transform_callbacks: HashMap::new(),
                session_fs_provider: None,
            })),
            invoke_fn: Arc::new(invoke_fn),
            capabilities: Arc::new(RwLock::new(None)),
        }
    }

    // =========================================================================
    // Session Properties
    // =========================================================================

    /// Get the session ID.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Get the workspace path for infinite sessions.
    ///
    /// Contains checkpoints/, plan.md, and files/ subdirectories.
    /// Returns None if infinite sessions are disabled.
    pub fn workspace_path(&self) -> Option<&str> {
        self.workspace_path.as_deref()
    }

    /// Get capabilities reported by the runtime for this session.
    pub async fn capabilities(&self) -> Option<SessionCapabilities> {
        self.capabilities.read().await.clone()
    }

    /// Get UI capabilities reported by the runtime for this session.
    pub async fn ui_capabilities(&self) -> Option<SessionUiCapabilities> {
        self.capabilities()
            .await
            .map(|capabilities| capabilities.ui)
    }

    /// Update capabilities after create/resume responses.
    pub(crate) async fn set_capabilities(&self, capabilities: Option<SessionCapabilities>) {
        *self.capabilities.write().await = capabilities;
    }

    // =========================================================================
    // Event Handling
    // =========================================================================

    /// Subscribe to session events.
    ///
    /// Returns a receiver that will receive all session events.
    pub fn subscribe(&self) -> EventSubscription {
        EventSubscription {
            receiver: self.event_tx.subscribe(),
        }
    }

    /// Register a callback-based event handler.
    ///
    /// Returns an unsubscribe closure. Call it to remove the handler.
    /// Alternatively, use [`off`] with the internal handler ID.
    pub async fn on<F>(&self, handler: F) -> impl FnOnce()
    where
        F: Fn(&SessionEvent) + Send + Sync + 'static,
    {
        let mut state = self.state.write().await;
        let id = state.next_handler_id.fetch_add(1, Ordering::SeqCst);
        state.event_handlers.insert(id, Arc::new(handler));

        let state_ref = Arc::clone(&self.state);
        move || {
            tokio::spawn(async move {
                state_ref.write().await.event_handlers.remove(&id);
            });
        }
    }

    /// Unsubscribe a callback-based event handler.
    pub async fn off(&self, handler_id: u64) {
        let mut state = self.state.write().await;
        state.event_handlers.remove(&handler_id);
    }

    /// Dispatch an event to all subscribers.
    ///
    /// Broadcast request events (external_tool.requested, permission.requested) are handled
    /// internally before being forwarded to user handlers (protocol v3 model).
    ///
    /// This is called by the Client when events are received.
    pub async fn dispatch_event(&self, event: SessionEvent) {
        // Handle broadcast request events (protocol v3) before dispatching to user handlers.
        // Fire-and-forget: the response is sent asynchronously via RPC.
        self.handle_broadcast_event(&event).await;

        if let SessionEventData::CapabilitiesChanged(data) = &event.data {
            self.update_capabilities(data).await;
        }

        // Send to broadcast channel
        let _ = self.event_tx.send(event.clone());

        // Call registered handlers
        let state = self.state.read().await;
        for handler in state.event_handlers.values() {
            handler(&event);
        }
    }

    /// Handle broadcast request events by executing local handlers and responding via RPC.
    ///
    /// Implements the protocol v3 broadcast model where tool calls and permission requests
    /// are broadcast as session events to all clients.
    async fn handle_broadcast_event(&self, event: &SessionEvent) {
        match &event.data {
            SessionEventData::ExternalToolRequested(data) => {
                let request_id = match &data.request_id {
                    Some(id) => id.clone(),
                    None => return,
                };
                let tool_name = match &data.tool_name {
                    Some(name) => name.clone(),
                    None => return,
                };

                // Check if this session handles this tool
                if self.get_tool(&tool_name).await.is_none() {
                    return; // This client doesn't handle this tool; another client will.
                }

                let _tool_call_id = data.tool_call_id.clone().unwrap_or_default();
                let arguments = data.arguments.clone().unwrap_or(serde_json::json!({}));
                let session_id = self.session_id.clone();

                // Execute tool and respond via handlePendingToolCall RPC
                match self.invoke_tool(&tool_name, &arguments).await {
                    Ok(result) => {
                        // Always send tool results via the result object so the model
                        // sees the full error details in textResultForLlm. Sending errors
                        // via the top-level "error" field causes the CLI to show a generic
                        // "tool execution failed" message instead of the actionable error.
                        // Use to_value so skip_serializing_if on
                        // ToolResultObject is respected (avoids null fields
                        // that corrupt CLI session files).
                        let result_val = serde_json::to_value(&result)
                            .unwrap_or_else(|_| serde_json::json!({"textResultForLlm": "serialization error", "resultType": "error"}));
                        let params = serde_json::json!({
                            "sessionId": session_id,
                            "requestId": request_id,
                            "result": result_val,
                        });
                        let _ =
                            (self.invoke_fn)("session.tools.handlePendingToolCall", Some(params))
                                .await;
                    }
                    Err(e) => {
                        // SDK-level error (tool not found, no handler) — send the error
                        // message as textResultForLlm so the model can see what went wrong.
                        let error_msg = e.to_string();
                        let params = serde_json::json!({
                            "sessionId": session_id,
                            "requestId": request_id,
                            "result": {
                                "textResultForLlm": error_msg,
                                "resultType": "error",
                                "error": error_msg,
                            }
                        });
                        let _ =
                            (self.invoke_fn)("session.tools.handlePendingToolCall", Some(params))
                                .await;
                    }
                }
            }
            SessionEventData::PermissionRequested(data) => {
                let request_id = match &data.request_id {
                    Some(id) => id.clone(),
                    None => return,
                };
                let perm_data = match &data.permission_request {
                    Some(d) => d.clone(),
                    None => return,
                };

                let session_id = self.session_id.clone();

                // Build PermissionRequest from JSON
                use crate::types::PermissionRequest;
                let kind = perm_data
                    .get("kind")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let tool_call_id = perm_data
                    .get("toolCallId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let mut extension_data = std::collections::HashMap::new();
                if let Some(obj) = perm_data.as_object() {
                    for (key, value) in obj {
                        if key != "kind" && key != "toolCallId" {
                            extension_data.insert(key.clone(), value.clone());
                        }
                    }
                }

                let request = PermissionRequest {
                    kind,
                    tool_call_id,
                    extension_data,
                };

                let result = self.handle_permission_request(&request).await;

                let mut perm_result_inner = serde_json::json!({
                    "kind": result.kind,
                });
                if let Some(rules) = &result.rules {
                    perm_result_inner["rules"] = serde_json::json!(rules);
                }
                let perm_result = serde_json::json!({
                    "sessionId": session_id,
                    "requestId": request_id,
                    "result": perm_result_inner,
                });

                if let Err(err) = (self.invoke_fn)(
                    "session.permissions.handlePendingPermissionRequest",
                    Some(perm_result),
                )
                .await
                {
                    tracing::warn!(
                        "Failed to respond to permission request {} for session {}: {}",
                        request_id,
                        session_id,
                        err
                    );
                }
            }
            SessionEventData::CommandExecute(data) => {
                let request_id = match &data.request_id {
                    Some(id) => id.clone(),
                    None => return,
                };
                let command_name = match &data.command_name {
                    Some(name) => name.clone(),
                    None => return,
                };

                // Only respond if this client has registered the command;
                // otherwise another client may handle it.
                if self.get_command(&command_name).await.is_none() {
                    return;
                }

                let command = data.command.clone();
                let args = data.args.clone();
                let session_id = self.session_id.clone();

                let context = CommandContext {
                    session_id: session_id.clone(),
                    command: command.clone(),
                    command_name: Some(command_name.clone()),
                    args: args.clone(),
                    arguments: args,
                    raw_input: command,
                };

                let response_params =
                    match self.handle_command_execute(&command_name, &context).await {
                        Ok(_) => serde_json::json!({
                            "sessionId": session_id,
                            "requestId": request_id,
                        }),
                        Err(err) => serde_json::json!({
                            "sessionId": session_id,
                            "requestId": request_id,
                            "error": err.to_string(),
                        }),
                    };

                if let Err(err) = (self.invoke_fn)(
                    "session.commands.handlePendingCommand",
                    Some(response_params),
                )
                .await
                {
                    tracing::warn!(
                        "Failed to respond to command request {} for session {}: {}",
                        request_id,
                        session_id,
                        err
                    );
                }
            }
            _ => {} // Not a broadcast request event
        }
    }

    async fn update_capabilities(&self, data: &crate::events::CapabilitiesChangedData) {
        let mut capabilities = self.capabilities.write().await;
        let mut next = capabilities.clone().unwrap_or_default();

        if let Some(ui) = &data.ui {
            if let Some(elicitation) = ui.elicitation {
                next.ui.elicitation = elicitation;
            }
            if let Some(commands) = ui.commands {
                next.ui.commands = commands;
            }
        }

        *capabilities = Some(next);
    }

    // =========================================================================
    // Messaging
    // =========================================================================

    /// Send a message to the session.
    ///
    /// Returns the message ID.
    pub async fn send(&self, options: impl Into<MessageOptions>) -> Result<String> {
        let options = options.into();
        // Use serde_json::to_value so that #[serde(skip_serializing_if)]
        // attributes on MessageOptions are respected — otherwise None fields
        // serialize as null, which corrupts CLI session files on resume.
        let mut params = serde_json::to_value(&options).map_err(|e| {
            CopilotError::Protocol(format!("Failed to serialize MessageOptions: {}", e))
        })?;
        params["sessionId"] = serde_json::Value::String(self.session_id.clone());

        let result = (self.invoke_fn)("session.send", Some(params)).await?;

        result
            .get("messageId")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| CopilotError::Protocol("Missing messageId in response".into()))
    }

    /// Abort the current message processing.
    pub async fn abort(&self) -> Result<()> {
        let params = serde_json::json!({
            "sessionId": self.session_id,
        });

        (self.invoke_fn)("session.abort", Some(params)).await?;
        Ok(())
    }

    /// Get all messages in the session.
    pub async fn get_messages(&self) -> Result<Vec<SessionEvent>> {
        let params = serde_json::json!({
            "sessionId": self.session_id,
        });

        let result = (self.invoke_fn)("session.getMessages", Some(params)).await?;

        let events: Vec<SessionEvent> = result
            .get("events")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| SessionEvent::from_json(v).ok())
                    .collect()
            })
            .or_else(|| {
                result
                    .get("messages")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| SessionEvent::from_json(v).ok())
                            .collect()
                    })
            })
            .ok_or_else(|| {
                CopilotError::Protocol("Missing events in getMessages response".into())
            })?;

        Ok(events)
    }

    // =========================================================================
    // Tool Management
    // =========================================================================

    /// Register a tool with this session.
    pub async fn register_tool(&self, tool: Tool) {
        self.register_tool_with_handler(tool, None).await;
    }

    /// Register a tool with a handler.
    pub async fn register_tool_with_handler(&self, tool: Tool, handler: Option<ToolHandler>) {
        let mut state = self.state.write().await;
        let name = tool.name.clone();
        state.tools.insert(name, RegisteredTool { tool, handler });
    }

    /// Register multiple tools.
    pub async fn register_tools(&self, tools: Vec<Tool>) {
        let mut state = self.state.write().await;
        for tool in tools {
            let name = tool.name.clone();
            state.tools.insert(
                name,
                RegisteredTool {
                    tool,
                    handler: None,
                },
            );
        }
    }

    /// Get a registered tool by name.
    pub async fn get_tool(&self, name: &str) -> Option<Tool> {
        let state = self.state.read().await;
        state.tools.get(name).map(|rt| rt.tool.clone())
    }

    /// Get all registered tools.
    pub async fn get_tools(&self) -> Vec<Tool> {
        let state = self.state.read().await;
        state.tools.values().map(|rt| rt.tool.clone()).collect()
    }

    /// Invoke a tool handler.
    pub async fn invoke_tool(&self, name: &str, arguments: &Value) -> Result<ToolResultObject> {
        let state = self.state.read().await;
        let registered = state
            .tools
            .get(name)
            .ok_or_else(|| CopilotError::ToolNotFound(name.to_string()))?;

        let handler = registered
            .handler
            .as_ref()
            .ok_or_else(|| CopilotError::ToolError(format!("No handler for tool: {}", name)))?;

        Ok(handler(name, arguments))
    }

    // =========================================================================
    // Permission Handling
    // =========================================================================

    /// Register a permission handler.
    pub async fn register_permission_handler<F>(&self, handler: F)
    where
        F: Fn(&PermissionRequest) -> PermissionRequestResult + Send + Sync + 'static,
    {
        let mut state = self.state.write().await;
        state.permission_handler = Some(Arc::new(handler));
    }

    /// Handle a permission request.
    ///
    /// Delegates to the registered permission handler, or denies by default
    /// if no handler is set.
    pub async fn handle_permission_request(
        &self,
        request: &PermissionRequest,
    ) -> PermissionRequestResult {
        let state = self.state.read().await;

        if let Some(handler) = &state.permission_handler {
            handler(request)
        } else {
            // Default: deny all permissions
            PermissionRequestResult::denied()
        }
    }

    // =========================================================================
    // User Input Handling
    // =========================================================================

    /// Register a handler for user input requests from the server.
    pub async fn register_user_input_handler<F>(&self, handler: F)
    where
        F: Fn(&UserInputRequest, &UserInputInvocation) -> UserInputResponse + Send + Sync + 'static,
    {
        let mut state = self.state.write().await;
        state.user_input_handler = Some(Arc::new(handler));
    }

    /// Handle a user input request from the server.
    pub async fn handle_user_input_request(
        &self,
        request: &UserInputRequest,
    ) -> Result<UserInputResponse> {
        let state = self.state.read().await;
        if let Some(handler) = &state.user_input_handler {
            let invocation = UserInputInvocation {
                session_id: self.session_id.clone(),
            };
            Ok(handler(request, &invocation))
        } else {
            Err(CopilotError::Protocol(
                "No user input handler registered".into(),
            ))
        }
    }

    /// Check if a user input handler is registered.
    pub async fn has_user_input_handler(&self) -> bool {
        let state = self.state.read().await;
        state.user_input_handler.is_some()
    }

    // =========================================================================
    // Commands
    // =========================================================================

    /// Register a slash command.
    pub async fn register_command(&self, command: CommandDefinition) {
        let mut state = self.state.write().await;
        state.commands.insert(command.name.clone(), command);
    }

    /// Register multiple slash commands.
    pub async fn register_commands(&self, commands: Vec<CommandDefinition>) {
        let mut state = self.state.write().await;
        for cmd in commands {
            state.commands.insert(cmd.name.clone(), cmd);
        }
    }

    /// Get a registered command by name.
    pub async fn get_command(&self, name: &str) -> Option<CommandDefinition> {
        let state = self.state.read().await;
        state.commands.get(name).cloned()
    }

    /// Execute a registered command by name.
    pub async fn handle_command_execute(
        &self,
        command_name: &str,
        context: &CommandContext,
    ) -> Result<CommandResult> {
        let state = self.state.read().await;
        if let Some(cmd) = state.commands.get(command_name) {
            if let Some(handler) = &cmd.handler {
                Ok(handler(context))
            } else {
                Ok(CommandResult::default())
            }
        } else {
            Err(CopilotError::Protocol(format!(
                "Unknown command: {}",
                command_name
            )))
        }
    }

    // =========================================================================
    // Elicitation
    // =========================================================================

    /// Register an elicitation handler for UI dialogs.
    pub async fn register_elicitation_handler(&self, handler: ElicitationHandler) {
        let mut state = self.state.write().await;
        state.elicitation_handler = Some(handler);
    }

    /// Check if an elicitation handler is registered.
    pub async fn has_elicitation_handler(&self) -> bool {
        let state = self.state.read().await;
        state.elicitation_handler.is_some()
    }

    /// Handle an elicitation request from the CLI.
    pub async fn handle_elicitation_request(
        &self,
        params: &ElicitationParams,
    ) -> Result<ElicitationResult> {
        let state = self.state.read().await;
        if let Some(handler) = &state.elicitation_handler {
            let context = ElicitationContext {
                session_id: self.session_id.clone(),
                params: params.clone(),
            };
            Ok(handler(&context))
        } else {
            Err(CopilotError::Protocol(
                "No elicitation handler registered".into(),
            ))
        }
    }

    // =========================================================================
    // Exit Plan Mode
    // =========================================================================

    /// Register a handler for `exitPlanMode.request` callbacks.
    ///
    /// Set [`crate::types::SessionConfig::request_exit_plan_mode`] to
    /// `Some(true)` so the runtime knows to dispatch them to this client.
    pub async fn register_exit_plan_mode_handler(&self, handler: ExitPlanModeHandler) {
        let mut state = self.state.write().await;
        state.exit_plan_mode_handler = Some(handler);
    }

    /// Whether an exit-plan-mode handler is registered.
    pub async fn has_exit_plan_mode_handler(&self) -> bool {
        let state = self.state.read().await;
        state.exit_plan_mode_handler.is_some()
    }

    /// Dispatch an `exitPlanMode.request` to the registered handler.
    ///
    /// Returns the default `{ approved: true }` result when no handler is
    /// registered, mirroring the Python SDK.
    pub async fn handle_exit_plan_mode_request(
        &self,
        request: &ExitPlanModeRequest,
    ) -> ExitPlanModeResult {
        let state = self.state.read().await;
        match &state.exit_plan_mode_handler {
            Some(handler) => handler(request),
            None => ExitPlanModeResult::default(),
        }
    }

    // =========================================================================
    // Auto Mode Switch
    // =========================================================================

    /// Register a handler for `autoModeSwitch.request` callbacks.
    ///
    /// Set [`crate::types::SessionConfig::request_auto_mode_switch`] to
    /// `Some(true)` so the runtime knows to dispatch them to this client.
    pub async fn register_auto_mode_switch_handler(&self, handler: AutoModeSwitchHandler) {
        let mut state = self.state.write().await;
        state.auto_mode_switch_handler = Some(handler);
    }

    /// Whether an auto-mode-switch handler is registered.
    pub async fn has_auto_mode_switch_handler(&self) -> bool {
        let state = self.state.read().await;
        state.auto_mode_switch_handler.is_some()
    }

    /// Dispatch an `autoModeSwitch.request` to the registered handler.
    ///
    /// Returns [`AutoModeSwitchResponse::No`] when no handler is registered,
    /// mirroring the Python SDK default.
    pub async fn handle_auto_mode_switch_request(
        &self,
        request: &AutoModeSwitchRequest,
    ) -> AutoModeSwitchResponse {
        let state = self.state.read().await;
        match &state.auto_mode_switch_handler {
            Some(handler) => handler(request),
            None => AutoModeSwitchResponse::No,
        }
    }

    // =========================================================================
    // System Message Transform
    // =========================================================================

    /// Register the per-section transform callbacks for the customize-mode
    /// system message. Replaces any previously stored callbacks.
    pub async fn register_transform_callbacks(
        &self,
        callbacks: HashMap<String, SectionTransformFn>,
    ) {
        let mut state = self.state.write().await;
        state.transform_callbacks = callbacks;
    }

    /// Whether at least one section transform callback is registered.
    pub async fn has_transform_callbacks(&self) -> bool {
        let state = self.state.read().await;
        !state.transform_callbacks.is_empty()
    }

    /// Handle a `systemMessage.transform` callback from the runtime.
    ///
    /// The runtime sends `{ sectionId: { content: "..." }, ... }`; this method
    /// invokes the registered callback for each section and returns
    /// `{ sections: { sectionId: { content: "transformed" } } }`. Sections
    /// without a callback are echoed back unchanged.
    pub async fn handle_system_message_transform(&self, sections: &Value) -> Value {
        let state = self.state.read().await;
        let callbacks = &state.transform_callbacks;

        let mut result = serde_json::Map::new();
        if let Some(obj) = sections.as_object() {
            for (section_id, section_data) in obj {
                let content = section_data
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let new_content = match callbacks.get(section_id) {
                    Some(callback) => callback(&content),
                    None => content,
                };
                let mut entry = serde_json::Map::new();
                entry.insert("content".into(), Value::String(new_content));
                result.insert(section_id.clone(), Value::Object(entry));
            }
        }
        serde_json::json!({ "sections": Value::Object(result) })
    }

    // =========================================================================
    // Session FS Provider (inbound)
    // =========================================================================

    /// Register an inbound FS provider for this session. The runtime will
    /// dispatch `sessionFs.*` RPC calls to it.
    pub async fn register_session_fs_provider(&self, provider: SharedSessionFsProvider) {
        let mut state = self.state.write().await;
        state.session_fs_provider = Some(provider);
    }

    /// Whether an inbound session-FS provider is currently registered.
    pub async fn has_session_fs_provider(&self) -> bool {
        let state = self.state.read().await;
        state.session_fs_provider.is_some()
    }

    async fn session_fs_provider(&self) -> Option<SharedSessionFsProvider> {
        let state = self.state.read().await;
        state.session_fs_provider.clone()
    }

    /// Dispatch an inbound `sessionFs.*` RPC by method name. Returns the JSON
    /// response payload.
    pub async fn handle_session_fs_request(&self, method: &str, params: &Value) -> Result<Value> {
        let provider = match self.session_fs_provider().await {
            Some(p) => p,
            None => {
                return Err(CopilotError::Protocol(format!(
                    "No session_fs handler registered for session: {}",
                    self.session_id
                )));
            }
        };

        let path = params
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let mode = params
            .get("mode")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);

        match method {
            "sessionFs.readFile" => match provider.read_file(&path).await {
                Ok(content) => Ok(serde_json::json!({ "content": content })),
                Err(err) => Ok(serde_json::json!({
                    "content": "",
                    "error": err_to_json(&err),
                })),
            },
            "sessionFs.writeFile" => {
                let content = params
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                match provider.write_file(&path, &content, mode).await {
                    Ok(()) => Ok(Value::Null),
                    Err(err) => Ok(err_to_json(&err)),
                }
            }
            "sessionFs.appendFile" => {
                let content = params
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                match provider.append_file(&path, &content, mode).await {
                    Ok(()) => Ok(Value::Null),
                    Err(err) => Ok(err_to_json(&err)),
                }
            }
            "sessionFs.exists" => match provider.exists(&path).await {
                Ok(exists) => Ok(serde_json::json!({ "exists": exists })),
                Err(_) => Ok(serde_json::json!({ "exists": false })),
            },
            "sessionFs.stat" => match provider.stat(&path).await {
                Ok(info) => Ok(serde_json::to_value(info).unwrap_or(Value::Null)),
                Err(err) => Ok(serde_json::json!({
                    "isFile": false,
                    "isDirectory": false,
                    "size": 0,
                    "mtime": "",
                    "birthtime": "",
                    "error": err_to_json(&err),
                })),
            },
            "sessionFs.mkdir" => {
                let recursive = params
                    .get("recursive")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                match provider.mkdir(&path, recursive, mode).await {
                    Ok(()) => Ok(Value::Null),
                    Err(err) => Ok(err_to_json(&err)),
                }
            }
            "sessionFs.readdir" => match provider.readdir(&path).await {
                Ok(entries) => Ok(serde_json::json!({ "entries": entries })),
                Err(err) => Ok(serde_json::json!({
                    "entries": [],
                    "error": err_to_json(&err),
                })),
            },
            "sessionFs.readdirWithTypes" => match provider.readdir_with_types(&path).await {
                Ok(entries) => Ok(serde_json::json!({
                    "entries": serde_json::to_value(entries).unwrap_or(Value::Array(vec![])),
                })),
                Err(err) => Ok(serde_json::json!({
                    "entries": [],
                    "error": err_to_json(&err),
                })),
            },
            "sessionFs.rm" => {
                let recursive = params
                    .get("recursive")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let force = params
                    .get("force")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                match provider.rm(&path, recursive, force).await {
                    Ok(()) => Ok(Value::Null),
                    Err(err) => Ok(err_to_json(&err)),
                }
            }
            "sessionFs.rename" => {
                let src = params
                    .get("src")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let dest = params
                    .get("dest")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                match provider.rename(&src, &dest).await {
                    Ok(()) => Ok(Value::Null),
                    Err(err) => Ok(err_to_json(&err)),
                }
            }
            other => Err(CopilotError::Protocol(format!(
                "Unknown sessionFs method: {}",
                other
            ))),
        }
    }

    // =========================================================================
    // Hooks
    // =========================================================================

    /// Register session hooks.
    pub async fn register_hooks(&self, hooks: SessionHooks) {
        let mut state = self.state.write().await;
        state.hooks = Some(hooks);
    }

    /// Check if any hooks are registered.
    pub async fn has_hooks(&self) -> bool {
        let state = self.state.read().await;
        state.hooks.as_ref().is_some_and(|h| h.has_any())
    }

    /// Handle a `hooks.invoke` callback from the server.
    ///
    /// Dispatches to the appropriate hook handler based on `hook_type` and returns
    /// the serialized output JSON.
    pub async fn handle_hooks_invoke(&self, hook_type: &str, input: &Value) -> Result<Value> {
        let state = self.state.read().await;
        let hooks = match &state.hooks {
            Some(h) => h,
            None => return Ok(Value::Null),
        };

        match hook_type {
            "preToolUse" => {
                if let Some(handler) = &hooks.on_pre_tool_use {
                    let hook_input: PreToolUseHookInput = serde_json::from_value(input.clone())
                        .map_err(|e| {
                            CopilotError::Protocol(format!("Invalid preToolUse input: {}", e))
                        })?;
                    let output = handler(hook_input);
                    Ok(serde_json::to_value(output).unwrap_or(Value::Null))
                } else {
                    Ok(Value::Null)
                }
            }
            "postToolUse" => {
                if let Some(handler) = &hooks.on_post_tool_use {
                    let hook_input: PostToolUseHookInput = serde_json::from_value(input.clone())
                        .map_err(|e| {
                            CopilotError::Protocol(format!("Invalid postToolUse input: {}", e))
                        })?;
                    let output = handler(hook_input);
                    Ok(serde_json::to_value(output).unwrap_or(Value::Null))
                } else {
                    Ok(Value::Null)
                }
            }
            "userPromptSubmitted" => {
                if let Some(handler) = &hooks.on_user_prompt_submitted {
                    let hook_input: UserPromptSubmittedHookInput =
                        serde_json::from_value(input.clone()).map_err(|e| {
                            CopilotError::Protocol(format!(
                                "Invalid userPromptSubmitted input: {}",
                                e
                            ))
                        })?;
                    let output = handler(hook_input);
                    Ok(serde_json::to_value(output).unwrap_or(Value::Null))
                } else {
                    Ok(Value::Null)
                }
            }
            "sessionStart" => {
                if let Some(handler) = &hooks.on_session_start {
                    let hook_input: SessionStartHookInput = serde_json::from_value(input.clone())
                        .map_err(|e| {
                        CopilotError::Protocol(format!("Invalid sessionStart input: {}", e))
                    })?;
                    let output = handler(hook_input);
                    Ok(serde_json::to_value(output).unwrap_or(Value::Null))
                } else {
                    Ok(Value::Null)
                }
            }
            "sessionEnd" => {
                if let Some(handler) = &hooks.on_session_end {
                    let hook_input: SessionEndHookInput = serde_json::from_value(input.clone())
                        .map_err(|e| {
                            CopilotError::Protocol(format!("Invalid sessionEnd input: {}", e))
                        })?;
                    let output = handler(hook_input);
                    Ok(serde_json::to_value(output).unwrap_or(Value::Null))
                } else {
                    Ok(Value::Null)
                }
            }
            "errorOccurred" => {
                if let Some(handler) = &hooks.on_error_occurred {
                    let hook_input: ErrorOccurredHookInput = serde_json::from_value(input.clone())
                        .map_err(|e| {
                            CopilotError::Protocol(format!("Invalid errorOccurred input: {}", e))
                        })?;
                    let output = handler(hook_input);
                    Ok(serde_json::to_value(output).unwrap_or(Value::Null))
                } else {
                    Ok(Value::Null)
                }
            }
            _ => Ok(Value::Null),
        }
    }

    // =========================================================================
    // Lifecycle
    // =========================================================================

    /// Disconnect from the session (preferred over `destroy`).
    pub async fn disconnect(&self) -> Result<()> {
        let params = serde_json::json!({
            "sessionId": self.session_id,
        });

        (self.invoke_fn)("session.destroy", Some(params)).await?;
        Ok(())
    }

    /// Destroy the session.
    #[deprecated(since = "0.2.0", note = "Use `disconnect()` instead")]
    pub async fn destroy(&self) -> Result<()> {
        self.disconnect().await
    }

    // =========================================================================
    // Runtime Model Switching
    // =========================================================================

    /// Switch the model used by this session at runtime.
    ///
    /// Optionally provide a reasoning effort level and/or model capabilities override.
    pub async fn set_model(
        &self,
        model: impl Into<String>,
        reasoning_effort: Option<String>,
        model_capabilities: Option<crate::types::ModelCapabilitiesOverride>,
    ) -> Result<()> {
        let mut params = serde_json::json!({
            "sessionId": self.session_id,
            "model": model.into(),
        });

        if let Some(effort) = reasoning_effort {
            params["reasoningEffort"] = serde_json::Value::String(effort);
        }

        if let Some(caps) = model_capabilities {
            if let Ok(caps_val) = serde_json::to_value(caps) {
                params["modelCapabilities"] = caps_val;
            }
        }

        (self.invoke_fn)("session.setModel", Some(params)).await?;
        Ok(())
    }

    // =========================================================================
    // Session Logging
    // =========================================================================

    /// Send a log message to the CLI runtime.
    pub async fn log(
        &self,
        message: impl Into<String>,
        level: Option<&str>,
        ephemeral: Option<bool>,
    ) -> Result<()> {
        let mut params = serde_json::json!({
            "sessionId": self.session_id,
            "message": message.into(),
        });

        if let Some(level) = level {
            params["level"] = serde_json::Value::String(level.to_string());
        }

        if let Some(ephemeral) = ephemeral {
            params["ephemeral"] = serde_json::Value::Bool(ephemeral);
        }

        (self.invoke_fn)("session.log", Some(params)).await?;
        Ok(())
    }

    // =========================================================================
    // Session Filesystem
    // =========================================================================

    /// Set up a filesystem provider for this session.
    pub async fn fs_set_provider(
        &self,
        request: crate::types::SessionFsSetProviderRequest,
    ) -> Result<crate::types::SessionFsSetProviderResult> {
        let params = serde_json::to_value(&request)
            .map_err(|e| CopilotError::Protocol(format!("Failed to serialize request: {}", e)))?;
        let result = (self.invoke_fn)("session.fs.setProvider", Some(params)).await?;
        serde_json::from_value(result)
            .map_err(|e| CopilotError::Protocol(format!("Invalid setProvider response: {}", e)))
    }

    /// Read a file from the session filesystem.
    pub async fn fs_read_file(&self, path: &str) -> Result<String> {
        let params = serde_json::json!({
            "sessionId": self.session_id,
            "path": path,
        });
        let result = (self.invoke_fn)("session.fs.readFile", Some(params)).await?;
        let parsed: crate::types::SessionFsReadFileResult = serde_json::from_value(result)
            .map_err(|e| CopilotError::Protocol(format!("Invalid readFile response: {}", e)))?;
        Ok(parsed.content)
    }

    /// Write content to a file in the session filesystem.
    pub async fn fs_write_file(&self, path: &str, content: &str, mode: Option<u32>) -> Result<()> {
        let mut params = serde_json::json!({
            "sessionId": self.session_id,
            "path": path,
            "content": content,
        });
        if let Some(m) = mode {
            params["mode"] = serde_json::Value::Number(m.into());
        }
        (self.invoke_fn)("session.fs.writeFile", Some(params)).await?;
        Ok(())
    }

    /// Append content to a file in the session filesystem.
    pub async fn fs_append_file(&self, path: &str, content: &str, mode: Option<u32>) -> Result<()> {
        let mut params = serde_json::json!({
            "sessionId": self.session_id,
            "path": path,
            "content": content,
        });
        if let Some(m) = mode {
            params["mode"] = serde_json::Value::Number(m.into());
        }
        (self.invoke_fn)("session.fs.appendFile", Some(params)).await?;
        Ok(())
    }

    /// Check if a path exists in the session filesystem.
    pub async fn fs_exists(&self, path: &str) -> Result<bool> {
        let params = serde_json::json!({
            "sessionId": self.session_id,
            "path": path,
        });
        let result = (self.invoke_fn)("session.fs.exists", Some(params)).await?;
        let parsed: crate::types::SessionFsExistsResult = serde_json::from_value(result)
            .map_err(|e| CopilotError::Protocol(format!("Invalid exists response: {}", e)))?;
        Ok(parsed.exists)
    }

    /// Get file/directory stat information from the session filesystem.
    pub async fn fs_stat(&self, path: &str) -> Result<crate::types::SessionFsStatResult> {
        let params = serde_json::json!({
            "sessionId": self.session_id,
            "path": path,
        });
        let result = (self.invoke_fn)("session.fs.stat", Some(params)).await?;
        serde_json::from_value(result)
            .map_err(|e| CopilotError::Protocol(format!("Invalid stat response: {}", e)))
    }

    /// Create a directory in the session filesystem.
    pub async fn fs_mkdir(
        &self,
        path: &str,
        mode: Option<u32>,
        recursive: Option<bool>,
    ) -> Result<()> {
        let mut params = serde_json::json!({
            "sessionId": self.session_id,
            "path": path,
        });
        if let Some(m) = mode {
            params["mode"] = serde_json::Value::Number(m.into());
        }
        if let Some(r) = recursive {
            params["recursive"] = serde_json::Value::Bool(r);
        }
        (self.invoke_fn)("session.fs.mkdir", Some(params)).await?;
        Ok(())
    }

    /// Read directory entries (names only) from the session filesystem.
    pub async fn fs_readdir(&self, path: &str) -> Result<Vec<String>> {
        let params = serde_json::json!({
            "sessionId": self.session_id,
            "path": path,
        });
        let result = (self.invoke_fn)("session.fs.readdir", Some(params)).await?;
        let parsed: crate::types::SessionFsReaddirResult = serde_json::from_value(result)
            .map_err(|e| CopilotError::Protocol(format!("Invalid readdir response: {}", e)))?;
        Ok(parsed.entries)
    }

    /// Read directory entries with type information from the session filesystem.
    pub async fn fs_readdir_with_types(
        &self,
        path: &str,
    ) -> Result<Vec<crate::types::SessionFsDirEntry>> {
        let params = serde_json::json!({
            "sessionId": self.session_id,
            "path": path,
        });
        let result = (self.invoke_fn)("session.fs.readdirWithTypes", Some(params)).await?;
        let parsed: crate::types::SessionFsReaddirWithTypesResult = serde_json::from_value(result)
            .map_err(|e| {
                CopilotError::Protocol(format!("Invalid readdirWithTypes response: {}", e))
            })?;
        Ok(parsed.entries)
    }

    /// Remove a file or directory from the session filesystem.
    pub async fn fs_rm(
        &self,
        path: &str,
        force: Option<bool>,
        recursive: Option<bool>,
    ) -> Result<()> {
        let mut params = serde_json::json!({
            "sessionId": self.session_id,
            "path": path,
        });
        if let Some(f) = force {
            params["force"] = serde_json::Value::Bool(f);
        }
        if let Some(r) = recursive {
            params["recursive"] = serde_json::Value::Bool(r);
        }
        (self.invoke_fn)("session.fs.rm", Some(params)).await?;
        Ok(())
    }

    /// Rename a file or directory in the session filesystem.
    pub async fn fs_rename(&self, src: &str, dest: &str) -> Result<()> {
        let params = serde_json::json!({
            "sessionId": self.session_id,
            "src": src,
            "dest": dest,
        });
        (self.invoke_fn)("session.fs.rename", Some(params)).await?;
        Ok(())
    }
}

// =============================================================================
// Convenience methods for waiting on events
// =============================================================================

impl Session {
    /// Default timeout for waiting on session events (60 seconds).
    const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

    /// Wait for the session to become idle.
    ///
    /// Returns the last assistant message event, or None if no message was received.
    /// Uses the specified timeout, or 60 seconds if None.
    pub async fn wait_for_idle(&self, timeout: Option<Duration>) -> Result<Option<SessionEvent>> {
        let timeout = timeout.unwrap_or(Self::DEFAULT_TIMEOUT);
        let mut subscription = self.subscribe();
        let mut last_assistant_message: Option<SessionEvent> = None;

        let result = tokio::time::timeout(timeout, async {
            loop {
                match subscription.recv().await {
                    Ok(event) => match &event.data {
                        SessionEventData::AssistantMessage(_) => {
                            last_assistant_message = Some(event);
                        }
                        SessionEventData::AssistantMessageDelta(_) => {
                            // Deltas are intermediate; we track the full message
                        }
                        SessionEventData::SessionIdle(_) => {
                            break;
                        }
                        SessionEventData::SessionError(err) => {
                            return Err(CopilotError::Protocol(format!(
                                "Session error: {}",
                                err.message
                            )));
                        }
                        _ => {}
                    },
                    Err(broadcast::error::RecvError::Closed) => {
                        return Err(CopilotError::ConnectionClosed);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // Continue - we missed some events but can recover
                    }
                }
            }
            Ok(())
        })
        .await;

        match result {
            Ok(Ok(())) => Ok(last_assistant_message),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(CopilotError::Timeout(timeout)),
        }
    }

    /// Send a message and wait for the complete response.
    ///
    /// Returns the last `AssistantMessage` event, or `None` if session
    /// became idle without producing an assistant message.
    /// Uses the specified timeout, or 60 seconds if None.
    pub async fn send_and_wait(
        &self,
        options: impl Into<MessageOptions>,
        timeout: Option<Duration>,
    ) -> Result<Option<SessionEvent>> {
        self.send(options).await?;
        self.wait_for_idle(timeout).await
    }

    /// Send a message and wait for the response content as a string.
    ///
    /// Convenience method that collects all assistant message/delta content.
    /// Uses the specified timeout, or 60 seconds if None.
    pub async fn send_and_collect(
        &self,
        options: impl Into<MessageOptions>,
        timeout: Option<Duration>,
    ) -> Result<String> {
        let timeout = timeout.unwrap_or(Self::DEFAULT_TIMEOUT);
        self.send(options).await?;

        let mut subscription = self.subscribe();
        let mut content = String::new();

        let result = tokio::time::timeout(timeout, async {
            loop {
                match subscription.recv().await {
                    Ok(event) => match &event.data {
                        SessionEventData::AssistantMessage(msg) => {
                            content.push_str(&msg.content);
                        }
                        SessionEventData::AssistantMessageDelta(delta) => {
                            content.push_str(&delta.delta_content);
                        }
                        SessionEventData::SessionIdle(_) => {
                            break;
                        }
                        SessionEventData::SessionError(err) => {
                            return Err(CopilotError::Protocol(format!(
                                "Session error: {}",
                                err.message
                            )));
                        }
                        _ => {}
                    },
                    Err(broadcast::error::RecvError::Closed) => {
                        return Err(CopilotError::ConnectionClosed);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                }
            }
            Ok(())
        })
        .await;

        match result {
            Ok(Ok(())) => Ok(content),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(CopilotError::Timeout(timeout)),
        }
    }

    /// Return a [`SessionUi`] handle for showing interactive dialogs to the
    /// user via the `session.ui.elicitation` RPC.
    ///
    /// Mirrors Python's `session.ui` property. Only useful when the runtime
    /// has reported `capabilities.ui.elicitation == true`.
    pub fn ui(self: &Arc<Self>) -> SessionUi {
        SessionUi {
            session: Arc::clone(self),
        }
    }
}

// =============================================================================
// SessionUi (outbound interactive dialogs)
// =============================================================================

/// Handle to the interactive UI surface of a [`Session`].
///
/// Methods drive interactive dialogs back to the CLI host via the
/// `session.ui.elicitation` JSON-RPC method. Each helper builds the
/// appropriate JSON Schema internally; use [`SessionUi::elicitation`] for
/// fully custom forms.
#[derive(Clone)]
pub struct SessionUi {
    session: Arc<Session>,
}

impl SessionUi {
    /// Send a raw `session.ui.elicitation` request with a caller-supplied
    /// JSON Schema and message.
    ///
    /// Returns a [`crate::types::UiElicitationResult`] containing the user's
    /// `action` and (when accepted) submitted `content` map.
    pub async fn elicitation(
        &self,
        message: impl Into<String>,
        requested_schema: Value,
    ) -> Result<crate::types::UiElicitationResult> {
        let params = serde_json::json!({
            "sessionId": self.session.session_id(),
            "message": message.into(),
            "requestedSchema": requested_schema,
        });
        let response = (self.session.invoke_fn)("session.ui.elicitation", Some(params)).await?;
        serde_json::from_value(response)
            .map_err(|e| CopilotError::Protocol(format!("Invalid ui.elicitation response: {}", e)))
    }

    /// Show a confirmation dialog. Returns `true` when the user accepts the
    /// dialog and the `confirmed` field comes back `true`.
    pub async fn confirm(&self, message: impl Into<String>) -> Result<bool> {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "confirmed": { "type": "boolean", "default": true },
            },
            "required": ["confirmed"],
        });
        let result = self.elicitation(message, schema).await?;
        Ok(result.action == "accept"
            && result
                .content
                .as_ref()
                .and_then(|c| c.get("confirmed"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false))
    }

    /// Show a select dialog. Returns the chosen option, or `None` if the user
    /// declined or cancelled.
    pub async fn select(
        &self,
        message: impl Into<String>,
        options: &[impl AsRef<str>],
    ) -> Result<Option<String>> {
        let enum_values: Vec<Value> = options
            .iter()
            .map(|o| Value::String(o.as_ref().to_string()))
            .collect();
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "selection": { "type": "string", "enum": enum_values },
            },
            "required": ["selection"],
        });
        let result = self.elicitation(message, schema).await?;
        if result.action != "accept" {
            return Ok(None);
        }
        Ok(result
            .content
            .as_ref()
            .and_then(|c| c.get("selection"))
            .and_then(|v| v.as_str())
            .map(String::from))
    }

    /// Show a text-input dialog. Returns the entered text, or `None` if the
    /// user declined or cancelled.
    pub async fn input(
        &self,
        message: impl Into<String>,
        options: Option<&crate::types::InputOptions>,
    ) -> Result<Option<String>> {
        let mut field = serde_json::Map::new();
        field.insert("type".into(), Value::String("string".into()));
        if let Some(opts) = options {
            if let Some(v) = &opts.title {
                field.insert("title".into(), Value::String(v.clone()));
            }
            if let Some(v) = &opts.description {
                field.insert("description".into(), Value::String(v.clone()));
            }
            if let Some(v) = opts.min_length {
                field.insert("minLength".into(), Value::from(v));
            }
            if let Some(v) = opts.max_length {
                field.insert("maxLength".into(), Value::from(v));
            }
            if let Some(v) = &opts.format {
                field.insert("format".into(), Value::String(v.clone()));
            }
            if let Some(v) = &opts.default {
                field.insert("default".into(), Value::String(v.clone()));
            }
        }
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "value": Value::Object(field) },
            "required": ["value"],
        });
        let result = self.elicitation(message, schema).await?;
        if result.action != "accept" {
            return Ok(None);
        }
        Ok(result
            .content
            .as_ref()
            .and_then(|c| c.get("value"))
            .and_then(|v| v.as_str())
            .map(String::from))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn mock_invoke(_method: &str, _params: Option<Value>) -> InvokeFuture {
        Box::pin(async { Ok(serde_json::json!({"messageId": "test-msg-123"})) })
    }

    fn mock_invoke_with_events(method: &str, _params: Option<Value>) -> InvokeFuture {
        let method = method.to_string();
        Box::pin(async move {
            if method == "session.getMessages" {
                return Ok(serde_json::json!({
                    "events": [{
                        "id": "evt-1",
                        "timestamp": "2024-01-01T00:00:00Z",
                        "type": "session.idle",
                        "data": {}
                    }]
                }));
            }
            Ok(serde_json::json!({"messageId": "test-msg-123"}))
        })
    }

    #[tokio::test]
    async fn test_session_id() {
        let session = Session::new("test-session-123".to_string(), None, mock_invoke);
        assert_eq!(session.session_id(), "test-session-123");
    }

    #[tokio::test]
    async fn test_workspace_path() {
        let session = Session::new(
            "test".to_string(),
            Some("/tmp/workspace".to_string()),
            mock_invoke,
        );
        assert_eq!(session.workspace_path(), Some("/tmp/workspace"));
    }

    #[tokio::test]
    async fn test_session_capabilities_accessors() {
        let session = Session::new("test".to_string(), None, mock_invoke);
        assert!(session.capabilities().await.is_none());
        assert!(session.ui_capabilities().await.is_none());

        session
            .set_capabilities(Some(SessionCapabilities {
                ui: SessionUiCapabilities {
                    elicitation: true,
                    commands: true,
                },
            }))
            .await;

        let capabilities = session.capabilities().await.unwrap();
        assert!(capabilities.ui.elicitation);
        assert!(capabilities.ui.commands);

        let ui = session.ui_capabilities().await.unwrap();
        assert!(ui.elicitation);
        assert!(ui.commands);
    }

    #[tokio::test]
    async fn test_capabilities_changed_event_updates_session_capabilities() {
        let session = Session::new("test".to_string(), None, mock_invoke);
        let event = SessionEvent::from_json(&serde_json::json!({
            "id": "evt-capabilities",
            "timestamp": "2024-01-01T00:00:00Z",
            "type": "capabilities.changed",
            "data": {
                "ui": {
                    "elicitation": true
                }
            }
        }))
        .unwrap();

        session.dispatch_event(event).await;

        let ui = session.ui_capabilities().await.unwrap();
        assert!(ui.elicitation);
        assert!(!ui.commands);
    }

    #[tokio::test]
    async fn test_register_tool() {
        let session = Session::new("test".to_string(), None, mock_invoke);

        let tool = Tool::new("my_tool").description("A test tool");

        session.register_tool(tool.clone()).await;

        let retrieved = session.get_tool("my_tool").await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name, "my_tool");
    }

    #[tokio::test]
    async fn test_register_tool_with_handler() {
        let session = Session::new("test".to_string(), None, mock_invoke);

        let tool = Tool::new("echo").description("Echo tool");
        let handler: ToolHandler = Arc::new(|_name, args| {
            let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("empty");
            ToolResultObject::text(text)
        });

        session
            .register_tool_with_handler(tool, Some(handler))
            .await;

        let result = session
            .invoke_tool("echo", &serde_json::json!({"text": "hello"}))
            .await
            .unwrap();

        assert_eq!(result.text_result_for_llm, "hello");
    }

    #[tokio::test]
    async fn test_invoke_unknown_tool() {
        let session = Session::new("test".to_string(), None, mock_invoke);

        let result = session.invoke_tool("unknown", &serde_json::json!({})).await;

        assert!(matches!(result, Err(CopilotError::ToolNotFound(_))));
    }

    #[tokio::test]
    async fn test_event_subscription() {
        let session = Session::new("test".to_string(), None, mock_invoke);

        let mut sub1 = session.subscribe();
        let mut sub2 = session.subscribe();

        // Dispatch an event
        let event = SessionEvent::from_json(&serde_json::json!({
            "id": "evt-1",
            "timestamp": "2024-01-01T00:00:00Z",
            "type": "session.idle",
            "data": {}
        }))
        .unwrap();

        session.dispatch_event(event).await;

        // Both subscribers should receive it
        let received1 = sub1.recv().await.unwrap();
        let received2 = sub2.recv().await.unwrap();

        assert_eq!(received1.id, "evt-1");
        assert_eq!(received2.id, "evt-1");
    }

    #[tokio::test]
    async fn test_callback_handler() {
        let session = Session::new("test".to_string(), None, mock_invoke);
        let call_count = Arc::new(AtomicUsize::new(0));

        let count_clone = Arc::clone(&call_count);
        let unsubscribe = session
            .on(move |_event| {
                count_clone.fetch_add(1, Ordering::SeqCst);
            })
            .await;

        // Dispatch events
        let event = SessionEvent::from_json(&serde_json::json!({
            "id": "evt-callback-1",
            "timestamp": "2024-01-01T00:00:00Z",
            "type": "session.idle",
            "data": {}
        }))
        .unwrap();

        session.dispatch_event(event).await;

        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // Unsubscribe
        unsubscribe();
    }

    #[tokio::test]
    async fn test_permission_handler() {
        let session = Session::new("test".to_string(), None, mock_invoke);

        // Default handler denies
        let request = PermissionRequest {
            kind: "tool_execution".to_string(),
            tool_call_id: Some("call-123".to_string()),
            extension_data: HashMap::new(),
        };
        let result = session.handle_permission_request(&request).await;
        assert!(result.kind.contains("denied"));

        // Register custom handler that approves
        session
            .register_permission_handler(|_req| PermissionRequestResult::approved())
            .await;

        let result = session.handle_permission_request(&request).await;
        assert_eq!(result.kind, "approved");
    }

    #[tokio::test]
    async fn test_get_messages_with_events_field() {
        let session = Session::new("test".to_string(), None, mock_invoke_with_events);
        let messages = session.get_messages().await.unwrap();
        assert_eq!(messages.len(), 1);
        assert!(matches!(
            messages[0].data,
            crate::events::SessionEventData::SessionIdle(_)
        ));
    }

    #[tokio::test]
    async fn test_user_input_handler() {
        let session = Session::new("test".to_string(), None, mock_invoke);

        session
            .register_user_input_handler(|req, _inv| {
                assert_eq!(req.question, "What color?");
                UserInputResponse {
                    answer: "blue".into(),
                    was_freeform: Some(true),
                }
            })
            .await;

        let request = UserInputRequest {
            question: "What color?".into(),
            choices: Some(vec!["red".into(), "blue".into()]),
            allow_freeform: Some(true),
        };

        let response = session.handle_user_input_request(&request).await.unwrap();
        assert_eq!(response.answer, "blue");
        assert_eq!(response.was_freeform, Some(true));
    }

    #[tokio::test]
    async fn test_user_input_no_handler_errors() {
        let session = Session::new("test".to_string(), None, mock_invoke);

        let request = UserInputRequest {
            question: "?".into(),
            choices: None,
            allow_freeform: None,
        };

        let result = session.handle_user_input_request(&request).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_register_hooks() {
        let session = Session::new("test".to_string(), None, mock_invoke);

        assert!(!session.has_hooks().await);

        let hooks = crate::types::SessionHooks {
            on_pre_tool_use: Some(Arc::new(|input| {
                assert_eq!(input.tool_name, "my_tool");
                crate::types::PreToolUseHookOutput {
                    permission_decision: Some("allow".into()),
                    ..Default::default()
                }
            })),
            ..Default::default()
        };

        session.register_hooks(hooks).await;
        assert!(session.has_hooks().await);
    }

    #[tokio::test]
    async fn test_hooks_invoke_pre_tool_use() {
        let session = Session::new("test".to_string(), None, mock_invoke);

        let hooks = crate::types::SessionHooks {
            on_pre_tool_use: Some(Arc::new(|_input| crate::types::PreToolUseHookOutput {
                permission_decision: Some("allow".into()),
                additional_context: Some("extra context".into()),
                ..Default::default()
            })),
            ..Default::default()
        };

        session.register_hooks(hooks).await;

        let input = serde_json::json!({
            "timestamp": 1234567890,
            "cwd": "/tmp",
            "toolName": "test_tool",
            "toolArgs": {"key": "value"}
        });

        let result = session
            .handle_hooks_invoke("preToolUse", &input)
            .await
            .unwrap();
        assert_eq!(
            result.get("permissionDecision").and_then(|v| v.as_str()),
            Some("allow")
        );
        assert_eq!(
            result.get("additionalContext").and_then(|v| v.as_str()),
            Some("extra context")
        );
    }

    #[tokio::test]
    async fn test_hooks_invoke_no_handler_returns_null() {
        let session = Session::new("test".to_string(), None, mock_invoke);

        // No hooks registered at all
        let result = session
            .handle_hooks_invoke("preToolUse", &serde_json::json!({}))
            .await
            .unwrap();
        assert!(result.is_null());

        // Hooks registered but not for this type
        let hooks = crate::types::SessionHooks {
            on_session_start: Some(Arc::new(|_input| {
                crate::types::SessionStartHookOutput::default()
            })),
            ..Default::default()
        };
        session.register_hooks(hooks).await;

        let input = serde_json::json!({
            "timestamp": 1234567890,
            "cwd": "/tmp",
            "toolName": "test_tool",
            "toolArgs": {}
        });
        let result = session
            .handle_hooks_invoke("preToolUse", &input)
            .await
            .unwrap();
        assert!(result.is_null());
    }

    #[tokio::test]
    async fn test_hooks_invoke_unknown_type_returns_null() {
        let session = Session::new("test".to_string(), None, mock_invoke);

        let hooks = crate::types::SessionHooks {
            on_pre_tool_use: Some(Arc::new(|_| crate::types::PreToolUseHookOutput::default())),
            ..Default::default()
        };
        session.register_hooks(hooks).await;

        let result = session
            .handle_hooks_invoke("unknownHookType", &serde_json::json!({}))
            .await
            .unwrap();
        assert!(result.is_null());
    }

    #[tokio::test]
    async fn test_exit_plan_mode_default_when_unregistered() {
        let session = Session::new("s".to_string(), None, mock_invoke);
        let request = crate::types::ExitPlanModeRequest {
            session_id: Some("s".into()),
            summary: "done".into(),
            plan_content: None,
            actions: vec!["autopilot".into()],
            recommended_action: "autopilot".into(),
        };
        let result = session.handle_exit_plan_mode_request(&request).await;
        assert!(result.approved);
    }

    #[tokio::test]
    async fn test_exit_plan_mode_dispatch() {
        let session = Session::new("s".to_string(), None, mock_invoke);
        session
            .register_exit_plan_mode_handler(Arc::new(|req| crate::types::ExitPlanModeResult {
                approved: false,
                selected_action: Some(req.recommended_action.clone()),
                feedback: Some("not now".into()),
            }))
            .await;
        let request = crate::types::ExitPlanModeRequest {
            session_id: Some("s".into()),
            summary: "done".into(),
            plan_content: None,
            actions: vec!["autopilot".into(), "manual".into()],
            recommended_action: "autopilot".into(),
        };
        let result = session.handle_exit_plan_mode_request(&request).await;
        assert!(!result.approved);
        assert_eq!(result.selected_action.as_deref(), Some("autopilot"));
    }

    #[tokio::test]
    async fn test_auto_mode_switch_default_when_unregistered() {
        let session = Session::new("s".to_string(), None, mock_invoke);
        let req = crate::types::AutoModeSwitchRequest::default();
        let resp = session.handle_auto_mode_switch_request(&req).await;
        assert_eq!(resp, crate::types::AutoModeSwitchResponse::No);
    }

    #[tokio::test]
    async fn test_system_message_transform_callback() {
        let session = Session::new("s".to_string(), None, mock_invoke);
        let mut callbacks: HashMap<String, crate::types::SectionTransformFn> = HashMap::new();
        callbacks.insert(
            "tone".to_string(),
            Arc::new(|content: &str| format!("{} [transformed]", content)),
        );
        session.register_transform_callbacks(callbacks).await;

        let sections = serde_json::json!({
            "tone": { "content": "be friendly" },
            "identity": { "content": "you are an agent" },
        });
        let response = session.handle_system_message_transform(&sections).await;
        let out = response.get("sections").unwrap();
        assert_eq!(
            out.get("tone").unwrap().get("content").unwrap().as_str(),
            Some("be friendly [transformed]")
        );
        assert_eq!(
            out.get("identity")
                .unwrap()
                .get("content")
                .unwrap()
                .as_str(),
            Some("you are an agent")
        );
    }

    #[tokio::test]
    async fn test_command_context_back_compat_fields() {
        let ctx = crate::types::CommandContext {
            session_id: "s".to_string(),
            command: Some("/help".into()),
            command_name: Some("help".into()),
            args: Some("".into()),
            arguments: Some("".into()),
            raw_input: Some("/help".into()),
        };
        assert_eq!(ctx.command.as_deref(), Some("/help"));
        assert_eq!(ctx.raw_input.as_deref(), Some("/help"));
    }

    #[tokio::test]
    async fn test_section_overrides_lower_to_wire() {
        let overrides = vec![
            crate::types::SectionOverride {
                section: crate::types::SystemPromptSection::Tone,
                action: crate::types::SectionOverrideAction::Replace("be terse".into()),
            },
            crate::types::SectionOverride {
                section: crate::types::SystemPromptSection::Identity,
                action: crate::types::SectionOverrideAction::Transform(Arc::new(|s: &str| {
                    format!("{}!!", s)
                })),
            },
        ];

        let mut wire_count_static = 0;
        let mut wire_count_transform = 0;
        for ov in &overrides {
            let (wire, callback) = ov.to_wire();
            match wire.action.as_str() {
                "replace" => {
                    assert_eq!(wire.content.as_deref(), Some("be terse"));
                    assert!(callback.is_none());
                    wire_count_static += 1;
                }
                "transform" => {
                    assert!(wire.content.is_none());
                    let cb = callback.expect("transform must produce a callback");
                    assert_eq!(cb("hello"), "hello!!");
                    wire_count_transform += 1;
                }
                other => panic!("unexpected wire action {other}"),
            }
        }
        assert_eq!(wire_count_static, 1);
        assert_eq!(wire_count_transform, 1);
    }
}
