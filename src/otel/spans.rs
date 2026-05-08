use tracing::Span;

pub fn provider_span(provider: &str, model: &str) -> Span {
    tracing::info_span!("provider", provider = provider, model = model)
}

pub fn tool_span(tool: &str, call_id: &str) -> Span {
    tracing::info_span!("tool", tool = tool, call_id = call_id)
}

pub fn session_span(session_id: &str) -> Span {
    tracing::info_span!("session", session_id = session_id)
}

pub fn child_session_span(session_id: &str, parent_session_id: &str) -> Span {
    let span = tracing::info_span!(
        "session",
        session_id = session_id,
        parent_session_id = parent_session_id,
    );
    span
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
            let span = provider_span("openai", "gpt-4");
            assert_eq!(span.metadata().map(|m| m.name()), Some("provider"));

            let field_names: Vec<&str> = span
                .metadata()
                .map(|m| m.fields().iter().map(|f: Field| f.name()).collect())
                .unwrap_or_default();
            assert!(field_names.contains(&"provider"));
            assert!(field_names.contains(&"model"));
        });
    }

    #[test]
    fn test_tool_span_has_expected_fields() {
        let (sub, _ring) = test_subscriber();
        with_default(sub, || {
            let span = tool_span("read_file", "tc-001");
            assert_eq!(span.metadata().map(|m| m.name()), Some("tool"));

            let field_names: Vec<&str> = span
                .metadata()
                .map(|m| m.fields().iter().map(|f: Field| f.name()).collect())
                .unwrap_or_default();
            assert!(field_names.contains(&"tool"));
            assert!(field_names.contains(&"call_id"));
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
    fn test_child_session_span_has_parent_field() {
        let (sub, _ring) = test_subscriber();
        with_default(sub, || {
            let span = child_session_span("child-001", "parent-000");
            assert_eq!(span.metadata().map(|m| m.name()), Some("session"));

            let field_names: Vec<&str> = span
                .metadata()
                .map(|m| m.fields().iter().map(|f: Field| f.name()).collect())
                .unwrap_or_default();
            assert!(field_names.contains(&"session_id"));
            assert!(field_names.contains(&"parent_session_id"));
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
