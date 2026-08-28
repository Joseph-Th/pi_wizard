use std::collections::HashMap;
use std::time::Instant;

use thiserror::Error;

use super::{RpcRequest, RpcResponse};
use crate::{RequestId, RuntimeLimits};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingRequest {
    pub id: RequestId,
    pub command: &'static str,
    pub expires_at: Option<Instant>,
}

/// Bounded correlation owner for app-generated RPC requests awaiting responses.
#[derive(Debug)]
pub struct PendingRequests {
    max_pending: usize,
    by_id: HashMap<String, PendingRequest>,
}

impl PendingRequests {
    #[must_use]
    pub fn new(max_pending: usize) -> Self {
        assert!(max_pending > 0, "pending request limit must be non-zero");
        Self {
            max_pending,
            by_id: HashMap::new(),
        }
    }

    #[must_use]
    pub fn from_limits(limits: RuntimeLimits) -> Self {
        Self::new(limits.max_pending_rpc_requests_per_run)
    }

    pub fn register(
        &mut self,
        request: &RpcRequest,
        expires_at: Option<Instant>,
    ) -> Result<(), PendingRequestError> {
        if self.by_id.contains_key(request.id.as_str()) {
            return Err(PendingRequestError::DuplicateId {
                id: request.id.as_str().to_owned(),
            });
        }
        if self.by_id.len() >= self.max_pending {
            return Err(PendingRequestError::Limit {
                limit: self.max_pending,
            });
        }

        self.by_id.insert(
            request.id.as_str().to_owned(),
            PendingRequest {
                id: request.id.clone(),
                command: request.command.wire_type(),
                expires_at,
            },
        );
        Ok(())
    }

    pub fn complete(
        &mut self,
        response: &RpcResponse,
    ) -> Result<PendingRequest, PendingRequestError> {
        let id = response
            .id
            .as_deref()
            .ok_or(PendingRequestError::MissingResponseId)?;
        let pending = self
            .by_id
            .get(id)
            .ok_or_else(|| PendingRequestError::UnknownResponseId { id: id.to_owned() })?;
        if pending.command != response.command {
            return Err(PendingRequestError::ResponseCommandMismatch {
                id: id.to_owned(),
                expected: pending.command,
                actual: response.command.clone(),
            });
        }
        self.by_id
            .remove(id)
            .ok_or_else(|| PendingRequestError::UnknownResponseId { id: id.to_owned() })
    }

    pub fn cancel(&mut self, id: &RequestId) -> Option<PendingRequest> {
        self.by_id.remove(id.as_str())
    }

    pub fn expire(&mut self, now: Instant) -> Vec<PendingRequest> {
        let expired_ids: Vec<String> = self
            .by_id
            .iter()
            .filter(|(_, request)| request.expires_at.is_some_and(|deadline| deadline <= now))
            .map(|(id, _)| id.clone())
            .collect();

        expired_ids
            .into_iter()
            .filter_map(|id| self.by_id.remove(&id))
            .collect()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum PendingRequestError {
    #[error("pending RPC request limit {limit} reached")]
    Limit { limit: usize },
    #[error("RPC request id {id} is already pending")]
    DuplicateId { id: String },
    #[error("correlated RPC response is missing id")]
    MissingResponseId,
    #[error("RPC response id {id} does not match a pending request")]
    UnknownResponseId { id: String },
    #[error("RPC response {id} reported command {actual}, expected {expected}")]
    ResponseCommandMismatch {
        id: String,
        expected: &'static str,
        actual: String,
    },
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::rpc::RpcCommand;

    fn response(id: Option<&str>) -> RpcResponse {
        RpcResponse {
            id: id.map(str::to_owned),
            command: "get_state".to_owned(),
            success: true,
            data: None,
            error: None,
            extra: Default::default(),
        }
    }

    #[test]
    fn request_is_removed_only_by_matching_response_id() {
        let mut pending = PendingRequests::new(2);
        let request = RpcRequest::with_id(RequestId::from_wire("req-1"), RpcCommand::GetState);
        pending.register(&request, None).expect("register request");

        assert_eq!(
            pending.complete(&response(Some("other"))),
            Err(PendingRequestError::UnknownResponseId {
                id: "other".to_owned()
            })
        );
        assert_eq!(pending.len(), 1);

        let completed = pending
            .complete(&response(Some("req-1")))
            .expect("matching response");
        assert_eq!(completed.id, request.id);
        assert_eq!(completed.command, "get_state");
        assert!(pending.is_empty());
    }

    #[test]
    fn registry_rejects_unbounded_growth() {
        let mut pending = PendingRequests::new(1);
        let first = RpcRequest::with_id(RequestId::from_wire("one"), RpcCommand::GetState);
        let second = RpcRequest::with_id(RequestId::from_wire("two"), RpcCommand::GetCommands);
        pending.register(&first, None).expect("first request");

        assert_eq!(
            pending.register(&second, None),
            Err(PendingRequestError::Limit { limit: 1 })
        );
    }

    #[test]
    fn centralized_runtime_limit_configures_registry_capacity() {
        let limits = RuntimeLimits {
            max_pending_rpc_requests_per_run: 1,
            ..RuntimeLimits::default()
        };
        let mut pending = PendingRequests::from_limits(limits);
        let first = RpcRequest::with_id(RequestId::from_wire("one"), RpcCommand::GetState);
        let second = RpcRequest::with_id(RequestId::from_wire("two"), RpcCommand::GetCommands);
        pending.register(&first, None).expect("first request");

        assert_eq!(
            pending.register(&second, None),
            Err(PendingRequestError::Limit { limit: 1 })
        );
    }

    #[test]
    fn response_without_id_cannot_implicitly_complete_oldest_request() {
        let mut pending = PendingRequests::new(1);
        let request = RpcRequest::with_id(RequestId::from_wire("one"), RpcCommand::GetState);
        pending.register(&request, None).expect("request");

        assert_eq!(
            pending.complete(&response(None)),
            Err(PendingRequestError::MissingResponseId)
        );
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn mismatched_response_command_does_not_retire_request() {
        let mut pending = PendingRequests::new(1);
        let request = RpcRequest::with_id(RequestId::from_wire("one"), RpcCommand::GetState);
        pending.register(&request, None).expect("request");
        let mut wrong = response(Some("one"));
        wrong.command = "get_messages".to_owned();

        assert_eq!(
            pending.complete(&wrong),
            Err(PendingRequestError::ResponseCommandMismatch {
                id: "one".to_owned(),
                expected: "get_state",
                actual: "get_messages".to_owned(),
            })
        );
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn expired_requests_release_bounded_correlation_capacity() {
        let mut pending = PendingRequests::new(1);
        let now = Instant::now();
        let first = RpcRequest::with_id(RequestId::from_wire("one"), RpcCommand::GetState);
        pending
            .register(&first, Some(now + Duration::from_secs(1)))
            .expect("first request");

        assert!(pending.expire(now).is_empty());
        let expired = pending.expire(now + Duration::from_secs(1));
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].id, first.id);
        assert!(pending.is_empty());

        pending
            .register(
                &RpcRequest::with_id(RequestId::from_wire("two"), RpcCommand::GetCommands),
                None,
            )
            .expect("capacity released");
    }
}
