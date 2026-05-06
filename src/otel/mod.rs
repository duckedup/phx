//! Telemetry module: in-memory ring buffer for the TUI observer view,
//! `tracing` integration, and span helpers.
//!
//! The optional OTLP HTTP exporter (`exporter.rs`) lives behind the `otlp`
//! feature flag and is not required for compilation.

pub mod ring;
pub mod spans;

use ring::{RingBuffer, RingLayer};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Configuration for telemetry initialisation.
pub struct TelemetryInit {
    /// OTLP collector endpoint (unused until the `otlp` feature is wired up).
    #[allow(dead_code)]
    pub otlp_endpoint: Option<String>,
    /// `service.name` resource attribute.
    #[allow(dead_code)]
    pub service_name: String,
    /// Maximum number of events kept in the in-memory ring buffer.
    pub ring_capacity: usize,
}

/// Handle returned by [`init`]. Holds the shared ring buffer so callers can
/// take snapshots or subscribe to live events. Dropping the handle is a
/// no-op; telemetry keeps flowing as long as the global subscriber is
/// installed.
pub struct TelemetryHandle {
    ring: RingBuffer,
}

impl TelemetryHandle {
    /// Access the underlying ring buffer (e.g. from the TUI).
    pub fn ring(&self) -> &RingBuffer {
        &self.ring
    }

    /// Shutdown hook. Currently a no-op — the OTLP exporter (when wired up)
    /// would flush its batch here.
    pub fn shutdown(&self) {
        // Future: signal the exporter task to flush + stop.
    }
}

/// Initialise the global tracing subscriber with:
///
/// 1. An [`EnvFilter`] sourced from `RUST_LOG` (default: `info`).
/// 2. A `fmt` layer that writes human-readable logs to stderr.
/// 3. A [`RingLayer`] that captures events into an in-memory ring buffer.
///
/// Returns a [`TelemetryHandle`] whose [`RingBuffer`] can be shared with the
/// TUI observer view.
///
/// # Panics
///
/// Panics if a global subscriber has already been set (i.e. `init` was called
/// twice in the same process).
pub fn init(cfg: TelemetryInit) -> TelemetryHandle {
    let ring = RingBuffer::new(cfg.ring_capacity);
    let ring_layer = RingLayer::new(ring.clone());

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(true)
        .with_ansi(true);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .with(ring_layer)
        .init();

    TelemetryHandle { ring }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::otel::ring::EventLevel;

    #[test]
    fn test_telemetry_init_struct() {
        let cfg = TelemetryInit {
            otlp_endpoint: Some("http://localhost:4318".into()),
            service_name: "test-service".into(),
            ring_capacity: 128,
        };
        assert_eq!(cfg.ring_capacity, 128);
        assert_eq!(cfg.service_name, "test-service");
        assert_eq!(cfg.otlp_endpoint.as_deref(), Some("http://localhost:4318"));
    }

    #[test]
    fn test_telemetry_handle_ring_access() {
        let ring = RingBuffer::new(16);
        let handle = TelemetryHandle { ring };
        assert!(handle.ring().is_empty());
        handle.shutdown(); // should not panic
    }

    /// Integration test: build the full subscriber stack (without installing
    /// it globally) and verify events flow through the ring layer.
    #[test]
    fn test_full_stack_events_captured() {
        use tracing::subscriber::with_default;
        use tracing_subscriber::layer::SubscriberExt;

        let ring = RingBuffer::new(32);
        let ring_layer = RingLayer::new(ring.clone());

        let subscriber = tracing_subscriber::registry().with(ring_layer);

        with_default(subscriber, || {
            tracing::info!(key = "val", "hello from test");
            tracing::warn!("a warning");
        });

        let snap = ring.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].level, EventLevel::Info);
        assert_eq!(snap[1].level, EventLevel::Warn);

        // Check that the info event captured the "message" and "key" fields.
        let fields = &snap[0].fields;
        assert_eq!(fields.get("key").and_then(|v| v.as_str()), Some("val"));
        assert!(fields.get("message").is_some());
    }
}
