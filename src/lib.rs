// Copyright (c) 2026 Elias Bachaalany
// SPDX-License-Identifier: MIT

#![forbid(unsafe_code)]

//! # Copilot SDK for Rust
//!
//! A Rust SDK for interacting with the GitHub Copilot CLI.
//!
//! ## Quick Start
//!
//! ```no_run
//! use copilot_sdk::{Client, SessionConfig, SessionEventData};
//!
//! #[tokio::main]
//! async fn main() -> copilot_sdk::Result<()> {
//!     let client = Client::builder().build()?;
//!     client.start().await?;
//!
//!     let session = client.create_session(SessionConfig::default()).await?;
//!     let mut events = session.subscribe();
//!
//!     session.send("What is the capital of France?").await?;
//!
//!     while let Ok(event) = events.recv().await {
//!         match &event.data {
//!             SessionEventData::AssistantMessage(msg) => println!("{}", msg.content),
//!             SessionEventData::SessionIdle(_) => break,
//!             _ => {}
//!         }
//!     }
//!
//!     client.stop().await;
//!     Ok(())
//! }
//! ```

pub mod client;
pub mod error;
pub mod events;
pub mod jsonrpc;
pub mod process;
pub mod session;
pub mod tools;
pub mod transport;
pub mod types;

// Re-export tool utilities
pub use tools::{convert_mcp_call_tool_result, define_tool};

// Re-export main types at crate root for convenience
pub use error::{CopilotError, Result};
pub use types::{
    // Session lifecycle event type constants
    session_lifecycle_event_types,
    // Agent types
    AgentInfo,
    // Enums
    AttachmentType,
    // Config types
    AzureOptions,
    ClientOptions,
    // Command types
    CommandContext,
    CommandDefinition,
    CommandHandler,
    CommandResult,
    ConnectionState,
    CustomAgentConfig,
    // Elicitation types
    ElicitationContext,
    ElicitationHandler,
    ElicitationOption,
    ElicitationParams,
    ElicitationResult,
    // Hook types
    ErrorOccurredHandler,
    ErrorOccurredHookInput,
    ErrorOccurredHookOutput,
    // External server config
    ExternalServerConfig,
    // Fleet types
    FleetStartOptions,
    // Response types
    GetAuthStatusResponse,
    GetForegroundSessionResponse,
    GetStatusResponse,
    InfiniteSessionConfig,
    LogLevel,
    McpLocalServerConfig,
    McpRemoteServerConfig,
    McpServerConfig,
    MessageOptions,
    ModelBilling,
    ModelCapabilities,
    // Model override types
    ModelCapabilitiesOverride,
    ModelInfo,
    ModelLimits,
    ModelLimitsOverride,
    ModelPolicy,
    ModelSupports,
    ModelSupportsOverride,
    ModelVisionLimits,
    ModelVisionLimitsOverride,
    // Permission types
    PermissionRequest,
    PermissionRequestResult,
    PingResponse,
    // Plan types
    PlanData,
    PostToolUseHandler,
    PostToolUseHookInput,
    PostToolUseHookOutput,
    PreToolUseHandler,
    PreToolUseHookInput,
    PreToolUseHookOutput,
    ProviderConfig,
    // Quota types
    QuotaResult,
    QuotaSnapshot,
    // Reasoning effort
    ReasoningEffort,
    ResumeSessionConfig,
    // System prompt section types
    SectionOverride,
    SectionOverrideAction,
    // Selection types
    SelectionAttachment,
    SelectionPosition,
    SelectionRange,
    // Session capabilities
    SessionCapabilities,
    SessionConfig,
    SessionEndHandler,
    SessionEndHookInput,
    SessionEndHookOutput,
    // Session FS types
    SessionFsConventions,
    SessionFsDirEntry,
    SessionFsDirEntryType,
    SessionFsExistsResult,
    SessionFsReadFileResult,
    SessionFsReaddirResult,
    SessionFsReaddirWithTypesResult,
    SessionFsSetProviderRequest,
    SessionFsSetProviderResult,
    SessionFsStatResult,
    SessionHooks,
    // Session lifecycle types
    SessionLifecycleEvent,
    SessionLifecycleEventMetadata,
    // Session list filter
    SessionListFilter,
    SessionMetadata,
    // Session mode
    SessionMode,
    SessionStartHandler,
    SessionStartHookInput,
    SessionStartHookOutput,
    SessionUiCapabilities,
    SetForegroundSessionResponse,
    // Shell execution
    ShellExecOptions,
    ShellExecResult,
    ShellSignal,
    StopError,
    SystemMessageConfig,
    SystemMessageMode,
    // System prompt section enum
    SystemPromptSection,
    // Telemetry config
    TelemetryConfig,
    // Tool types
    Tool,
    ToolBinaryResult,
    ToolInfo,
    ToolInvocation,
    ToolResult,
    ToolResultObject,
    ToolResultType,
    ToolsListResult,
    // User input types
    UserInputInvocation,
    UserInputRequest,
    UserInputResponse,
    UserMessageAttachment,
    UserPromptSubmittedHandler,
    UserPromptSubmittedHookInput,
    UserPromptSubmittedHookOutput,
    // Workspace types
    WorkspaceFile,
    // Constants
    SDK_PROTOCOL_VERSION,
};

// Re-export event types
pub use events::{
    // Event data types
    AbortData,
    AssistantIntentData,
    AssistantMessageData,
    AssistantMessageDeltaData,
    AssistantReasoningData,
    AssistantReasoningDeltaData,
    AssistantTurnEndData,
    AssistantTurnStartData,
    AssistantUsageData,
    CommandCompleteData,
    CommandStartData,
    CompactionTokensUsed,
    CustomAgentCompletedData,
    CustomAgentFailedData,
    CustomAgentSelectedData,
    CustomAgentStartedData,
    ElicitationRequestData,
    ElicitationResponseData,
    HandoffSourceType,
    HookEndData,
    HookError,
    HookStartData,
    PendingMessagesModifiedData,
    // Main event types
    RawSessionEvent,
    RepositoryInfo,
    SessionCompactionCompleteData,
    SessionCompactionStartData,
    SessionErrorData,
    SessionEvent,
    SessionEventData,
    SessionHandoffData,
    SessionIdleData,
    SessionInfoData,
    SessionModelChangeData,
    SessionResumeData,
    SessionShutdownData,
    SessionSnapshotRewindData,
    SessionStartData,
    SessionTruncationData,
    SessionUsageInfoData,
    ShutdownCodeChanges,
    ShutdownType,
    SkillInvokedData,
    SystemMessageEventData,
    SystemMessageMetadata,
    SystemMessageRole,
    ToolExecutionCompleteData,
    ToolExecutionError,
    ToolExecutionPartialResultData,
    ToolExecutionProgressData,
    ToolExecutionStartData,
    ToolRequestItem,
    ToolResultContent,
    ToolUserRequestedData,
    UserMessageAttachmentItem,
    UserMessageData,
};

// Re-export transport types
pub use transport::{MessageFramer, StdioTransport, Transport};

// Re-export JSON-RPC types
pub use jsonrpc::{
    JsonRpcClient, JsonRpcError, JsonRpcId, JsonRpcRequest, JsonRpcResponse, NotificationHandler,
    RequestHandler,
};

// Re-export process types
pub use process::{
    find_copilot_cli, find_executable, find_node, is_node_script, CopilotProcess, ProcessOptions,
};

// Re-export session types
pub use session::{
    EventHandler, EventSubscription, InvokeFuture, PermissionHandler, RegisteredTool, Session,
    ToolHandler, UserInputHandler,
};

// Re-export client types
pub use client::{Client, ClientBuilder, LifecycleHandler};
