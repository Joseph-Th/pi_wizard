use std::collections::VecDeque;

use thiserror::Error;

use crate::{RequestId, RuntimeLimits};

const FRAME_ACCOUNTING_OVERHEAD_BYTES: usize = 32;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiCoalesceKey {
    AssistantBlock(usize),
    ToolPreview(String),
    DirectBash(RequestId),
}

impl UiCoalesceKey {
    fn byte_len(&self) -> usize {
        match self {
            Self::AssistantBlock(_) => std::mem::size_of::<usize>(),
            Self::ToolPreview(id) => id.len(),
            Self::DirectBash(id) => id.as_str().len(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiBacklogFrame {
    /// A transition/error/request ownership frame that must not be discarded to
    /// make room for display-only stream progress.
    Semantic(Vec<u8>),
    /// A display-only frame where only the newest value for a key matters.
    Coalescible {
        key: UiCoalesceKey,
        payload: Vec<u8>,
    },
}

impl UiBacklogFrame {
    fn cost(&self) -> usize {
        let payload = match self {
            Self::Semantic(payload) | Self::Coalescible { payload, .. } => payload.len(),
        };
        let key = match self {
            Self::Semantic(_) => 0,
            Self::Coalescible { key, .. } => key.byte_len(),
        };
        FRAME_ACCOUNTING_OVERHEAD_BYTES
            .saturating_add(key)
            .saturating_add(payload)
    }

    fn is_coalescible(&self) -> bool {
        matches!(self, Self::Coalescible { .. })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UiBacklogPush {
    Queued,
    Coalesced,
    DroppedDisplayFrame,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UiBacklogStats {
    pub resident_bytes: usize,
    pub frame_count: usize,
    pub coalesced_frames: u64,
    pub dropped_display_frames: u64,
}

/// Byte-bounded per-run queue between normalized backend state and renderer
/// delivery.
///
/// Semantic transitions are never evicted to preserve token/tool progress. If
/// semantic frames alone exhaust the cap, the caller gets an explicit error
/// and should force renderer rehydration rather than silently losing state.
#[derive(Debug)]
pub struct UiEventBacklog {
    max_bytes: usize,
    resident_bytes: usize,
    frames: VecDeque<UiBacklogFrame>,
    coalesced_frames: u64,
    dropped_display_frames: u64,
}

impl UiEventBacklog {
    #[must_use]
    pub fn new(max_bytes: usize) -> Self {
        assert!(max_bytes > 0, "UI backlog byte limit must be non-zero");
        Self {
            max_bytes,
            resident_bytes: 0,
            frames: VecDeque::new(),
            coalesced_frames: 0,
            dropped_display_frames: 0,
        }
    }

    #[must_use]
    pub fn from_limits(limits: RuntimeLimits) -> Self {
        Self::new(limits.max_ui_backlog_bytes_per_run)
    }

    pub fn push_semantic(&mut self, payload: Vec<u8>) -> Result<UiBacklogPush, UiBacklogError> {
        let frame = UiBacklogFrame::Semantic(payload);
        let cost = frame.cost();
        if cost > self.max_bytes {
            return Err(UiBacklogError::FrameTooLarge {
                actual: cost,
                limit: self.max_bytes,
            });
        }

        self.evict_display_frames_until(cost);
        if self.resident_bytes.saturating_add(cost) > self.max_bytes {
            return Err(UiBacklogError::SemanticCapacityExhausted {
                resident: self.resident_bytes,
                incoming: cost,
                limit: self.max_bytes,
            });
        }

        self.resident_bytes = self.resident_bytes.saturating_add(cost);
        self.frames.push_back(frame);
        Ok(UiBacklogPush::Queued)
    }

    pub fn push_coalescible(&mut self, key: UiCoalesceKey, payload: Vec<u8>) -> UiBacklogPush {
        let had_previous = self.remove_matching_key(&key);
        if had_previous {
            self.coalesced_frames = self.coalesced_frames.saturating_add(1);
        }

        let frame = UiBacklogFrame::Coalescible { key, payload };
        let cost = frame.cost();
        if cost > self.max_bytes {
            self.dropped_display_frames = self.dropped_display_frames.saturating_add(1);
            return UiBacklogPush::DroppedDisplayFrame;
        }

        self.evict_display_frames_until(cost);
        if self.resident_bytes.saturating_add(cost) > self.max_bytes {
            self.dropped_display_frames = self.dropped_display_frames.saturating_add(1);
            return UiBacklogPush::DroppedDisplayFrame;
        }

        self.resident_bytes = self.resident_bytes.saturating_add(cost);
        self.frames.push_back(frame);
        if had_previous {
            UiBacklogPush::Coalesced
        } else {
            UiBacklogPush::Queued
        }
    }

    pub fn pop_front(&mut self) -> Option<UiBacklogFrame> {
        let frame = self.frames.pop_front()?;
        self.resident_bytes = self.resident_bytes.saturating_sub(frame.cost());
        Some(frame)
    }

    pub fn clear(&mut self) {
        self.frames.clear();
        self.resident_bytes = 0;
    }

    #[must_use]
    pub fn stats(&self) -> UiBacklogStats {
        UiBacklogStats {
            resident_bytes: self.resident_bytes,
            frame_count: self.frames.len(),
            coalesced_frames: self.coalesced_frames,
            dropped_display_frames: self.dropped_display_frames,
        }
    }

    fn remove_matching_key(&mut self, key: &UiCoalesceKey) -> bool {
        let Some(index) = self.frames.iter().position(|frame| {
            matches!(
                frame,
                UiBacklogFrame::Coalescible {
                    key: existing,
                    ..
                } if existing == key
            )
        }) else {
            return false;
        };

        let removed = self
            .frames
            .remove(index)
            .expect("position came from the same queue");
        self.resident_bytes = self.resident_bytes.saturating_sub(removed.cost());
        true
    }

    fn evict_display_frames_until(&mut self, incoming_cost: usize) {
        while self.resident_bytes.saturating_add(incoming_cost) > self.max_bytes {
            let Some(index) = self.frames.iter().position(UiBacklogFrame::is_coalescible) else {
                return;
            };
            let removed = self
                .frames
                .remove(index)
                .expect("position came from the same queue");
            self.resident_bytes = self.resident_bytes.saturating_sub(removed.cost());
            self.dropped_display_frames = self.dropped_display_frames.saturating_add(1);
        }
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum UiBacklogError {
    #[error("one semantic UI frame costs {actual} bytes, exceeding backlog limit {limit}")]
    FrameTooLarge { actual: usize, limit: usize },
    #[error(
        "semantic UI backlog exhausted: resident {resident} + incoming {incoming} exceeds limit {limit}"
    )]
    SemanticCapacityExhausted {
        resident: usize,
        incoming: usize,
        limit: usize,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_stream_key_is_replaced_and_moves_behind_newer_semantic_state() {
        let mut backlog = UiEventBacklog::new(1024);
        assert_eq!(
            backlog.push_coalescible(UiCoalesceKey::AssistantBlock(0), b"old".to_vec()),
            UiBacklogPush::Queued
        );
        backlog
            .push_semantic(b"semantic".to_vec())
            .expect("semantic");
        assert_eq!(
            backlog.push_coalescible(UiCoalesceKey::AssistantBlock(0), b"new".to_vec()),
            UiBacklogPush::Coalesced
        );

        assert_eq!(
            backlog.pop_front(),
            Some(UiBacklogFrame::Semantic(b"semantic".to_vec()))
        );
        assert_eq!(
            backlog.pop_front(),
            Some(UiBacklogFrame::Coalescible {
                key: UiCoalesceKey::AssistantBlock(0),
                payload: b"new".to_vec(),
            })
        );
        assert_eq!(backlog.stats().coalesced_frames, 1);
    }

    #[test]
    fn display_pressure_evicts_display_frames_but_never_semantic_transition() {
        let semantic_cost = FRAME_ACCOUNTING_OVERHEAD_BYTES + 8;
        let display_cost = FRAME_ACCOUNTING_OVERHEAD_BYTES + std::mem::size_of::<usize>() + 8;
        let mut backlog = UiEventBacklog::new(semantic_cost + display_cost);
        backlog
            .push_semantic(b"semantic".to_vec())
            .expect("semantic");
        backlog.push_coalescible(UiCoalesceKey::AssistantBlock(0), b"display0".to_vec());

        assert_eq!(
            backlog.push_coalescible(UiCoalesceKey::AssistantBlock(1), b"display1".to_vec()),
            UiBacklogPush::Queued
        );
        assert_eq!(
            backlog.pop_front(),
            Some(UiBacklogFrame::Semantic(b"semantic".to_vec()))
        );
        assert_eq!(backlog.stats().dropped_display_frames, 1);
    }

    #[test]
    fn semantic_only_overflow_is_explicit_rehydration_condition() {
        let cost = FRAME_ACCOUNTING_OVERHEAD_BYTES + 1;
        let mut backlog = UiEventBacklog::new(cost);
        backlog.push_semantic(vec![1]).expect("first semantic");

        assert_eq!(
            backlog.push_semantic(vec![2]),
            Err(UiBacklogError::SemanticCapacityExhausted {
                resident: cost,
                incoming: cost,
                limit: cost,
            })
        );
        assert_eq!(backlog.stats().frame_count, 1);
    }

    #[test]
    fn accounting_overhead_bounds_empty_frame_count() {
        let mut backlog = UiEventBacklog::new(FRAME_ACCOUNTING_OVERHEAD_BYTES * 2);
        backlog.push_semantic(Vec::new()).expect("first empty");
        backlog.push_semantic(Vec::new()).expect("second empty");
        assert!(matches!(
            backlog.push_semantic(Vec::new()),
            Err(UiBacklogError::SemanticCapacityExhausted { .. })
        ));
    }

    #[test]
    fn oversized_display_frame_is_dropped_without_disturbing_semantic_queue() {
        let mut backlog = UiEventBacklog::new(64);
        backlog
            .push_semantic(b"important".to_vec())
            .expect("semantic");
        assert_eq!(
            backlog.push_coalescible(UiCoalesceKey::ToolPreview("call".to_owned()), vec![0; 128],),
            UiBacklogPush::DroppedDisplayFrame
        );
        assert_eq!(backlog.stats().frame_count, 1);
    }
}
