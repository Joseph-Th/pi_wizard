use std::collections::VecDeque;
use std::io;

use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::RuntimeLimits;

use super::{
    ExtensionUiResponse, FrameError, InboundMessage, JsonlDecoder, OutboundEncodeError,
    RpcParseError, RpcRequest, encode_extension_ui_response, encode_request, parse_frame,
};

const READ_CHUNK_BYTES: usize = 16 * 1024;

/// Async reader for Pi's stdout protocol stream.
///
/// Framing is delegated to [`JsonlDecoder`], so chunk boundaries, Unicode line
/// separators, and oversized records have one implementation and one test surface.
pub struct RpcReader<R> {
    inner: R,
    decoder: JsonlDecoder,
    pending: VecDeque<Result<InboundMessage, RpcReadError>>,
    eof: bool,
}

impl<R> RpcReader<R>
where
    R: AsyncRead + Unpin,
{
    #[must_use]
    pub fn new(inner: R, max_frame_bytes: usize) -> Self {
        Self {
            inner,
            decoder: JsonlDecoder::new(max_frame_bytes),
            pending: VecDeque::new(),
            eof: false,
        }
    }

    pub async fn next_message(&mut self) -> Option<Result<InboundMessage, RpcReadError>> {
        loop {
            if let Some(message) = self.pending.pop_front() {
                return Some(message);
            }
            if self.eof {
                return None;
            }

            let mut chunk = [0_u8; READ_CHUNK_BYTES];
            match self.inner.read(&mut chunk).await {
                Ok(0) => {
                    self.eof = true;
                    if let Some(frame) = self.decoder.finish() {
                        return Some(decode_frame(frame));
                    }
                }
                Ok(read) => {
                    self.pending.extend(
                        self.decoder
                            .push(&chunk[..read])
                            .into_iter()
                            .map(decode_frame),
                    );
                }
                Err(source) => {
                    self.eof = true;
                    return Some(Err(RpcReadError::Io(source)));
                }
            }
        }
    }

    #[must_use]
    pub fn into_inner(self) -> R {
        self.inner
    }
}

fn decode_frame(frame: Result<Vec<u8>, FrameError>) -> Result<InboundMessage, RpcReadError> {
    let bytes = frame?;
    Ok(parse_frame(&bytes)?)
}

/// Async writer for Pi stdin with enforced outbound payload ceilings.
pub struct RpcWriter<W> {
    inner: W,
    limits: RuntimeLimits,
}

impl<W> RpcWriter<W>
where
    W: AsyncWrite + Unpin,
{
    #[must_use]
    pub fn new(inner: W, limits: RuntimeLimits) -> Self {
        Self { inner, limits }
    }

    pub async fn send_request(&mut self, request: &RpcRequest) -> Result<(), RpcWriteError> {
        let encoded = encode_request(request, self.limits)?;
        self.write_encoded(&encoded).await
    }

    pub async fn send_extension_ui_response(
        &mut self,
        response: &ExtensionUiResponse,
    ) -> Result<(), RpcWriteError> {
        let encoded = encode_extension_ui_response(response, self.limits.max_outbound_rpc_bytes)?;
        self.write_encoded(&encoded).await
    }

    async fn write_encoded(&mut self, encoded: &[u8]) -> Result<(), RpcWriteError> {
        self.inner.write_all(encoded).await?;
        self.inner.flush().await?;
        Ok(())
    }

    #[must_use]
    pub fn into_inner(self) -> W {
        self.inner
    }
}

#[derive(Debug, Error)]
pub enum RpcReadError {
    #[error("failed reading Pi RPC stdout: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error(transparent)]
    Parse(#[from] RpcParseError),
}

#[derive(Debug, Error)]
pub enum RpcWriteError {
    #[error("failed writing Pi RPC stdin: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Encode(#[from] OutboundEncodeError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RequestId;
    use crate::rpc::{RpcCommand, RpcEventKind};
    use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

    #[tokio::test]
    async fn reader_handles_transport_chunking_without_line_reader_semantics() {
        let (mut producer, consumer) = duplex(128);
        let writer = tokio::spawn(async move {
            producer
                .write_all(b"{\"type\":\"agent_start\"}\n{\"type\":\"future")
                .await
                .expect("write first chunk");
            producer
                .write_all(b"_event\",\"value\":1}\n")
                .await
                .expect("write second chunk");
        });
        let mut reader = RpcReader::new(consumer, 1024);

        let first = reader
            .next_message()
            .await
            .expect("first message")
            .expect("valid first message");
        let second = reader
            .next_message()
            .await
            .expect("second message")
            .expect("valid second message");
        writer.await.expect("producer task");

        let InboundMessage::Event(first) = first else {
            panic!("expected first event");
        };
        let InboundMessage::Event(second) = second else {
            panic!("expected second event");
        };
        assert_eq!(first.kind, RpcEventKind::AgentStart);
        assert_eq!(
            second.kind,
            RpcEventKind::Unknown("future_event".to_owned())
        );
    }

    #[tokio::test]
    async fn reader_surfaces_oversized_frame_then_recovers() {
        let (mut producer, consumer) = duplex(128);
        producer
            .write_all(b"012345678901234567890123456789\n{\"type\":\"agent_start\"}\n")
            .await
            .expect("write fixture");
        drop(producer);
        let mut reader = RpcReader::new(consumer, 24);

        assert!(matches!(
            reader.next_message().await,
            Some(Err(RpcReadError::Frame(FrameError::TooLarge { limit: 24 })))
        ));
        let next = reader
            .next_message()
            .await
            .expect("recovered frame")
            .expect("recovered message");
        let InboundMessage::Event(event) = next else {
            panic!("expected event");
        };
        assert_eq!(event.kind, RpcEventKind::AgentStart);
    }

    #[tokio::test]
    async fn writer_emits_one_lf_terminated_request() {
        let (producer, mut consumer) = duplex(256);
        let mut writer = RpcWriter::new(producer, RuntimeLimits::default());
        let request = RpcRequest::with_id(RequestId::from_wire("req-1"), RpcCommand::GetState);

        writer.send_request(&request).await.expect("write request");
        drop(writer);
        let mut bytes = Vec::new();
        consumer
            .read_to_end(&mut bytes)
            .await
            .expect("read request");

        assert_eq!(bytes.last(), Some(&b'\n'));
        assert_eq!(bytes.iter().filter(|byte| **byte == b'\n').count(), 1);
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).expect("written bytes are JSON");
        assert_eq!(value["id"], "req-1");
        assert_eq!(value["type"], "get_state");
    }
}
