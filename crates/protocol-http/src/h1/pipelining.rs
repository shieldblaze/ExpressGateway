//! HTTP pipelining support with serial response ordering.
//!
//! HTTP/1.1 pipelining allows clients to send multiple requests on the same
//! connection without waiting for responses. However, responses must be sent
//! back in the same order as the requests. This module provides a queue that
//! ensures correct response ordering.

use std::collections::VecDeque;

use bytes::Bytes;
use http::Response;
use http_body_util::Full;

/// A queued request awaiting its response.
#[derive(Debug)]
pub struct PendingRequest {
    /// Unique sequence number for ordering.
    pub sequence: u64,
    /// The completed response, if available.
    pub response: Option<Response<Full<Bytes>>>,
}

/// Queue for managing HTTP pipelined requests and ensuring serial response
/// ordering.
///
/// Requests are assigned monotonically increasing sequence numbers. Responses
/// may arrive out of order (from concurrent backend processing) but are only
/// dequeued in sequence order.
pub struct PipelineQueue {
    /// Next sequence number to assign.
    next_sequence: u64,
    /// Next sequence number that should be sent to the client.
    next_send: u64,
    /// Whether a request is currently in-flight (actively being processed).
    request_in_flight: bool,
    /// Pending requests ordered by sequence number.
    pending: VecDeque<PendingRequest>,
    /// Maximum number of pending requests.
    max_pending: usize,
}

impl PipelineQueue {
    /// Create a new pipeline queue.
    ///
    /// `max_pending` limits how many requests can be queued before the
    /// connection stops accepting new pipelined requests.
    pub fn new(max_pending: usize) -> Self {
        Self {
            next_sequence: 0,
            next_send: 0,
            request_in_flight: false,
            pending: VecDeque::with_capacity(max_pending.min(64)),
            max_pending,
        }
    }

    /// Enqueue a new pipelined request.
    ///
    /// Returns `Some(sequence)` if the request was enqueued, or `None` if the
    /// queue is full.
    pub fn enqueue(&mut self) -> Option<u64> {
        if self.pending.len() >= self.max_pending {
            return None;
        }

        let seq = self.next_sequence;
        self.next_sequence += 1;
        self.pending.push_back(PendingRequest {
            sequence: seq,
            response: None,
        });
        Some(seq)
    }

    /// Attach a response to a pending request by sequence number.
    ///
    /// Returns `true` if the response was attached, `false` if the sequence
    /// number was not found in the queue.
    pub fn complete(&mut self, sequence: u64, response: Response<Full<Bytes>>) -> bool {
        for entry in &mut self.pending {
            if entry.sequence == sequence {
                entry.response = Some(response);
                return true;
            }
        }
        false
    }

    /// Try to dequeue the next response that should be sent to the client.
    ///
    /// Returns the response if the next-in-order request has its response ready.
    /// Responses are only returned in strict sequence order.
    pub fn try_dequeue(&mut self) -> Option<Response<Full<Bytes>>> {
        if let Some(front) = self.pending.front()
            && front.sequence == self.next_send
            && front.response.is_some()
        {
            let entry = self.pending.pop_front().unwrap();
            self.next_send += 1;
            return entry.response;
        }
        None
    }

    /// Whether a request is currently being actively processed.
    pub fn is_in_flight(&self) -> bool {
        self.request_in_flight
    }

    /// Mark that a request is now in flight.
    pub fn set_in_flight(&mut self, in_flight: bool) {
        self.request_in_flight = in_flight;
    }

    /// Number of pending requests in the queue.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Whether the queue is full.
    pub fn is_full(&self) -> bool {
        self.pending.len() >= self.max_pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_response(body: &str) -> Response<Full<Bytes>> {
        Response::builder()
            .status(200)
            .body(Full::new(Bytes::from(body.to_owned())))
            .unwrap()
    }

    #[test]
    fn enqueue_and_dequeue_in_order() {
        let mut q = PipelineQueue::new(10);

        let s0 = q.enqueue().unwrap();
        let s1 = q.enqueue().unwrap();
        assert_eq!(s0, 0);
        assert_eq!(s1, 1);

        // Complete s0, dequeue it.
        q.complete(s0, make_response("resp0"));
        let resp = q.try_dequeue().unwrap();
        assert_eq!(resp.status(), 200);

        // Complete s1, dequeue it.
        q.complete(s1, make_response("resp1"));
        let resp = q.try_dequeue().unwrap();
        assert_eq!(resp.status(), 200);

        assert!(q.is_empty());
    }

    #[test]
    fn out_of_order_completion_waits() {
        let mut q = PipelineQueue::new(10);

        let s0 = q.enqueue().unwrap();
        let s1 = q.enqueue().unwrap();

        // Complete s1 first. Should not dequeue because s0 is not ready.
        q.complete(s1, make_response("resp1"));
        assert!(q.try_dequeue().is_none());

        // Now complete s0. Should dequeue both in order.
        q.complete(s0, make_response("resp0"));
        assert!(q.try_dequeue().is_some()); // s0
        assert!(q.try_dequeue().is_some()); // s1
        assert!(q.try_dequeue().is_none());
    }

    #[test]
    fn queue_limit_enforced() {
        let mut q = PipelineQueue::new(2);

        assert!(q.enqueue().is_some());
        assert!(q.enqueue().is_some());
        assert!(q.enqueue().is_none()); // Full.
        assert!(q.is_full());
    }

    #[test]
    fn in_flight_flag() {
        let mut q = PipelineQueue::new(10);
        assert!(!q.is_in_flight());
        q.set_in_flight(true);
        assert!(q.is_in_flight());
        q.set_in_flight(false);
        assert!(!q.is_in_flight());
    }

    #[test]
    fn pending_count() {
        let mut q = PipelineQueue::new(10);
        assert_eq!(q.pending_count(), 0);
        q.enqueue();
        assert_eq!(q.pending_count(), 1);
        q.enqueue();
        assert_eq!(q.pending_count(), 2);
    }

    #[test]
    fn complete_unknown_sequence_returns_false() {
        let mut q = PipelineQueue::new(10);
        q.enqueue(); // seq 0
        assert!(!q.complete(999, make_response("nope")));
    }
}
