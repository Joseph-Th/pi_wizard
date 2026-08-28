use thiserror::Error;

/// Incremental LF-only JSONL decoder with a hard per-frame memory bound.
///
/// It deliberately does not use Unicode line semantics. U+2028 and U+2029 are
/// ordinary UTF-8 bytes and remain inside the JSON frame, matching Pi RPC.
#[derive(Debug)]
pub struct JsonlDecoder {
    max_frame_bytes: usize,
    buffer: Vec<u8>,
    discarding_oversized_frame: bool,
}

impl JsonlDecoder {
    #[must_use]
    pub fn new(max_frame_bytes: usize) -> Self {
        assert!(max_frame_bytes > 0, "JSONL frame limit must be non-zero");
        Self {
            max_frame_bytes,
            buffer: Vec::with_capacity(max_frame_bytes.min(16 * 1024)),
            discarding_oversized_frame: false,
        }
    }

    /// Feed arbitrary transport chunks and return every complete frame/error in order.
    pub fn push(&mut self, mut input: &[u8]) -> Vec<Result<Vec<u8>, FrameError>> {
        let mut output = Vec::new();

        while !input.is_empty() {
            if self.discarding_oversized_frame {
                if let Some(newline) = input.iter().position(|byte| *byte == b'\n') {
                    self.discarding_oversized_frame = false;
                    input = &input[newline + 1..];
                    continue;
                }
                break;
            }

            match input.iter().position(|byte| *byte == b'\n') {
                Some(newline) => {
                    let fragment = &input[..newline];
                    if self.buffer.len().saturating_add(fragment.len()) > self.max_frame_bytes {
                        self.buffer.clear();
                        output.push(Err(FrameError::TooLarge {
                            limit: self.max_frame_bytes,
                        }));
                    } else {
                        self.buffer.extend_from_slice(fragment);
                        let mut frame = std::mem::take(&mut self.buffer);
                        if frame.last() == Some(&b'\r') {
                            frame.pop();
                        }
                        output.push(Ok(frame));
                    }
                    input = &input[newline + 1..];
                }
                None => {
                    if self.buffer.len().saturating_add(input.len()) > self.max_frame_bytes {
                        self.buffer.clear();
                        self.discarding_oversized_frame = true;
                        output.push(Err(FrameError::TooLarge {
                            limit: self.max_frame_bytes,
                        }));
                    } else {
                        self.buffer.extend_from_slice(input);
                    }
                    break;
                }
            }
        }

        output
    }

    /// Flush a final unterminated frame at EOF.
    ///
    /// Pi normally emits LF-terminated records. Returning the final bytes lets the
    /// JSON parser classify a complete final record or report a precise malformed
    /// JSON error rather than silently discarding child output.
    pub fn finish(&mut self) -> Option<Result<Vec<u8>, FrameError>> {
        if self.discarding_oversized_frame {
            self.discarding_oversized_frame = false;
            self.buffer.clear();
            return None;
        }
        if self.buffer.is_empty() {
            return None;
        }

        let mut frame = std::mem::take(&mut self.buffer);
        if frame.last() == Some(&b'\r') {
            frame.pop();
        }
        Some(Ok(frame))
    }

    #[must_use]
    pub fn buffered_bytes(&self) -> usize {
        self.buffer.len()
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum FrameError {
    #[error("RPC JSONL frame exceeded {limit} bytes")]
    TooLarge { limit: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_split_and_multiple_frames() {
        let mut decoder = JsonlDecoder::new(64);

        assert!(decoder.push(br#"{"type":"agent_"#).is_empty());
        let frames = decoder.push(b"start\"}\n{\"type\":\"agent_settled\"}\n");

        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0], Ok(br#"{"type":"agent_start"}"#.to_vec()));
        assert_eq!(frames[1], Ok(br#"{"type":"agent_settled"}"#.to_vec()));
    }

    #[test]
    fn strips_optional_carriage_return_only_before_lf() {
        let mut decoder = JsonlDecoder::new(64);
        let frames = decoder.push(b"{}\r\n");

        assert_eq!(frames, vec![Ok(b"{}".to_vec())]);
    }

    #[test]
    fn unicode_line_separator_does_not_split_a_frame() {
        let mut decoder = JsonlDecoder::new(128);
        let frame = "{\"text\":\"before\u{2028}after\"}\n";
        let frames = decoder.push(frame.as_bytes());

        assert_eq!(frames.len(), 1);
        assert_eq!(
            String::from_utf8(frames[0].clone().expect("frame should decode"))
                .expect("fixture is UTF-8"),
            "{\"text\":\"before\u{2028}after\"}"
        );
    }

    #[test]
    fn oversized_frame_is_reported_once_and_decoder_recovers_after_lf() {
        let mut decoder = JsonlDecoder::new(8);

        let first = decoder.push(b"0123456789");
        assert_eq!(first, vec![Err(FrameError::TooLarge { limit: 8 })]);
        assert_eq!(decoder.buffered_bytes(), 0);

        let second = decoder.push(b"discarded\n{}\n");
        assert_eq!(second, vec![Ok(b"{}".to_vec())]);
        assert_eq!(decoder.buffered_bytes(), 0);
    }

    #[test]
    fn oversized_complete_frame_does_not_poison_following_frame() {
        let mut decoder = JsonlDecoder::new(4);
        let frames = decoder.push(b"12345\n{}\n");

        assert_eq!(
            frames,
            vec![Err(FrameError::TooLarge { limit: 4 }), Ok(b"{}".to_vec())]
        );
    }

    #[test]
    fn finish_returns_unterminated_final_frame() {
        let mut decoder = JsonlDecoder::new(16);
        assert!(decoder.push(b"{}").is_empty());

        assert_eq!(decoder.finish(), Some(Ok(b"{}".to_vec())));
        assert_eq!(decoder.finish(), None);
    }
}
