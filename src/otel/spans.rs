use tracing::Span;

/// Create a span for an LLM provider call.
///
/// Fields: `provider`, `model`.
pub fn provider_span(provider: &str, model: &str) -> Span {
    tracing::info_span!("provider", provider = provider, model = model,)
}

/// Create a span for a tool invocation.
///
/// Fields: `tool`.
pub fn tool_span(tool: &str) -> Span {
    tracing::info_span!("tool", tool = tool,)
}

/// Create a span for a session lifecycle.
///
/// Fields: `session_id`.
pub fn session_span(session_id: &str) -> Span {
    tracing::info_span!("session", session_id = session_id,)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing::field::Field;
    use tracing::subscriber::with_default;
    use tracing_subscriber::layer::SubscriberExt;

    use crate::otel::ring::{RingBuffer, RingLayer};

    /// Helper: install a ring-backed subscriber and return the ring so we can
    /// inspect captured events.
    fn test_subscriber() -> (impl tracing::Subscriber, RingBuffer) {
        let ring = RingBuffer::new(64);
        let layer = RingLayer::new(ring.clone());
        let subscriber = tracing_subscriber::registry().with(layer);
        (subscriber, ring)
    }

    #[test]
    fn test_provider_span_has_expected_fields() {
        let (sub, _ring) = test_subscriber();

        with_default(sub, || {
            let span = provider_span("anthropic", "claude-4");
            let _guard = span.enter();

            // Verify the span was created with the right metadata
            span.record("provider", "anthropic");

            // Access span fields via the extensions
            span.with_subscriber(|(id, dispatch)| {
                // Verify span exists in the subscriber
                let _ = id;
                let _ = dispatch;
            });
        });

        // Verify the span has the right name and fields by inspecting metadata
        let span = provider_span("openai", "gpt-4");
        assert_eq!(span.metadata().map(|m| m.name()), Some("provider"));

        // Verify field names exist in metadata
        let field_names: Vec<&str> = span
            .metadata()
            .map(|m| m.fields().iter().map(|f: Field| f.name()).collect())
            .unwrap_or_default();
        assert!(field_names.contains(&"provider"));
        assert!(field_names.contains(&"model"));
    }

    #[test]
    fn test_tool_span_has_expected_fields() {
        let (sub, _ring) = test_subscriber();

        with_default(sub, || {
            let span = tool_span("read_file");
            assert_eq!(span.metadata().map(|m| m.name()), Some("tool"));

            let field_names: Vec<&str> = span
                .metadata()
                .map(|m| m.fields().iter().map(|f: Field| f.name()).collect())
                .unwrap_or_default();
            assert!(field_names.contains(&"tool"));
        });
    }

    #[test]
    fn test_session_span_has_expected_fields() {
        let (sub, _ring) = test_subscriber();

        with_default(sub, || {
            let span = session_span("sess-abc-123");
            assert_eq!(span.metadata().map(|m| m.name()), Some("session"));

            let field_names: Vec<&str> = span
                .metadata()
                .map(|m| m.fields().iter().map(|f: Field| f.name()).collect())
                .unwrap_or_default();
            assert!(field_names.contains(&"session_id"));
        });
    }

    #[test]
    fn test_span_inside_subscriber_captures_events() {
        let (sub, ring) = test_subscriber();

        with_default(sub, || {
            let span = session_span("sess-001");
            let _guard = span.enter();
            tracing::info!("session started");
        });

        let events = ring.snapshot();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].level, crate::otel::ring::EventLevel::Info);
    }
}
