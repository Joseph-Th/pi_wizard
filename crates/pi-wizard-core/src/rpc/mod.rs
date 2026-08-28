mod command;
mod extension_ui;
mod framing;
mod gate;
mod io;
mod message;
mod pending;
mod response;

pub use command::{
    AttachmentError, ExtensionUiResponse, ImageContent, OutboundEncodeError, QueueMode, RpcCommand,
    RpcConcurrencyClass, RpcRequest, StreamingBehavior, ThinkingLevel,
    encode_extension_ui_response, encode_request,
};
pub use extension_ui::{
    ExtensionDialogKind, ExtensionDialogRequest, ExtensionFireAndForget, ExtensionNotifyType,
    ExtensionUiDisposition, ExtensionUiMethod, ExtensionUiParseError, ExtensionUiRequest,
    ExtensionUiRequestMeta, ExtensionWidgetPlacement,
};
pub use framing::{FrameError, JsonlDecoder};
pub use gate::{ActiveRpcCommand, RpcCommandGate, RpcGateError};
pub use io::{RpcReadError, RpcReader, RpcWriteError, RpcWriter};
pub use message::{
    AssistantMessageBlockKind, AssistantMessageUpdate, BashExecutionUpdate, InboundMessage,
    QueueUpdateCounts, RpcEvent, RpcEventKind, RpcEventPayloadError, RpcParseError, RpcResponse,
    RpcResponseOutcome, SessionInfoChanged, ToolCallStartMeta, ToolExecutionEnd,
    ToolExecutionStart, ToolExecutionUpdate, parse_frame,
};
pub use pending::{PendingRequest, PendingRequestError, PendingRequests};
pub use response::{
    BashCommandResult, ClearQueueResult, CommandSummary, CompactionResult, ModelSummary,
    RpcResponsePayloadError, RpcStateSnapshot, SessionContextUsage, SessionEntriesPage,
    SessionEntryEnvelope, SessionStats, SessionTokenUsage,
};
