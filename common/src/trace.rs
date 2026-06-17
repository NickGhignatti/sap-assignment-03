//! # Lightweight distributed tracing
//!
//! A single correlation id — the **trace id** — is minted at the system entry
//! point (the Order Service HTTP API) and propagated end-to-end across the
//! services through a Kafka message header (`x-trace-id`). Every consumer
//! extracts it and enters a `tracing` span carrying `trace_id`, so the
//! structured logs of one order can be followed across Order → Delivery →
//! Drone.
//!
//! This is the Kafka-native counterpart of the trace-context propagation that
//! Micrometer Observation provided over AMQP in assignment 02: the broker has
//! no notion of a trace, so we carry the id ourselves in a message header and
//! re-attach it on every downstream publish.

use rdkafka::message::{BorrowedHeaders, Header, Headers, OwnedHeaders};

/// Kafka header key that carries the end-to-end trace id.
pub const TRACE_ID_HEADER: &str = "x-trace-id";

/// Mint a fresh trace id. Called once per order, at the HTTP entry point.
pub fn new_trace_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Build a Kafka header set carrying `trace_id`, ready to attach to a produced
/// record via `FutureRecord::headers(...)`.
pub fn trace_headers(trace_id: &str) -> OwnedHeaders {
    OwnedHeaders::new().insert(Header {
        key: TRACE_ID_HEADER,
        value: Some(trace_id),
    })
}

/// Extract the trace id from a consumed message's headers, if present.
pub fn extract_trace_id(headers: Option<&BorrowedHeaders>) -> Option<String> {
    headers.and_then(|h| find_trace_id(h))
}

/// Extract the trace id from a consumed message's headers, minting a fresh one
/// when the header is absent (e.g. a message produced by an older client).
/// Never returns empty, so every span gets a usable id.
pub fn trace_id_or_new(headers: Option<&BorrowedHeaders>) -> String {
    extract_trace_id(headers).unwrap_or_else(new_trace_id)
}

/// Scan any `Headers` implementation for the trace-id header. Generic so it can
/// be unit-tested against `OwnedHeaders` (a `BorrowedHeaders` only exists tied
/// to a live consumed message).
fn find_trace_id<H: Headers>(headers: &H) -> Option<String> {
    for i in 0..headers.count() {
        let header = headers.get(i);
        if header.key == TRACE_ID_HEADER {
            return header
                .value
                .and_then(|bytes| String::from_utf8(bytes.to_vec()).ok());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_trace_id_is_nonempty_and_unique() {
        let a = new_trace_id();
        let b = new_trace_id();
        assert!(!a.is_empty());
        assert_ne!(a, b);
    }

    #[test]
    fn trace_headers_carry_the_id() {
        let headers = trace_headers("trace-123");
        assert_eq!(headers.count(), 1);
        let header = headers.get(0);
        assert_eq!(header.key, TRACE_ID_HEADER);
        assert_eq!(header.value, Some(&b"trace-123"[..]));
    }

    #[test]
    fn trace_id_round_trips_through_headers() {
        let headers = trace_headers("trace-456");
        assert_eq!(find_trace_id(&headers), Some("trace-456".to_string()));
    }

    #[test]
    fn missing_header_yields_none() {
        let empty = OwnedHeaders::new();
        assert_eq!(find_trace_id(&empty), None);
    }
}
