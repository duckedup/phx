use std::collections::VecDeque;
use std::sync::Arc;
use std::time::SystemTime;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::Level;
use tracing::field::{Field, Visit};

// ---------------------------------------------------------------------------
// Event type stored in the ring
// ---------------------------------------------------------------------------

/// A captured tracing event, serializable for the TUI observer view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub ts: SystemTime,
    pub level: EventLevel,
    pub target: String,
    pub fields: serde_json::Value,
}

/// Mirrors [`tracing::Level`] but is `Serialize`/`Deserialize`-friendly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum EventLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl From<&Level> for EventLevel {
    fn from(level: &Level) -> Self {
        match *level {
            Level::TRACE => Self::Trace,
            Level::DEBUG => Self::Debug,
            Level::INFO => Self::Info,
            Level::WARN => Self::Warn,
            Level::ERROR => Self::Error,
        }
    }
}

// ---------------------------------------------------------------------------
// Ring buffer
// ---------------------------------------------------------------------------

/// In-memory ring buffer that stores the most recent `capacity` events.
///
/// Thread-safe via [`parking_lot::RwLock`]. Events are also broadcast to
/// live subscribers (e.g. the TUI observer view) via a
/// [`tokio::sync::broadcast`] channel.
#[derive(Debug, Clone)]
pub struct RingBuffer {
    inner: Arc<RingInner>,
}

#[derive(Debug)]
struct RingInner {
    buf: RwLock<VecDeque<Event>>,
    capacity: usize,
    tx: broadcast::Sender<Event>,
}

impl RingBuffer {
    /// Create a new ring buffer with the given maximum capacity.
    ///
    /// The broadcast channel is sized to `capacity` as well; slow receivers
    /// that fall behind will see `RecvError::Lagged`.
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity.max(1));
        Self {
            inner: Arc::new(RingInner {
                buf: RwLock::new(VecDeque::with_capacity(capacity)),
                capacity,
                tx,
            }),
        }
    }

    /// Push an event into the ring, evicting the oldest if at capacity.
    ///
    /// The event is also broadcast to all active subscribers.
    pub fn push(&self, event: Event) {
        // Broadcast first (clones the event). We ignore the error – it just
        // means there are no active receivers right now.
        let _ = self.inner.tx.send(event.clone());

        let mut buf = self.inner.buf.write();
        if buf.len() >= self.inner.capacity {
            buf.pop_front();
        }
        buf.push_back(event);
    }

    /// Return a snapshot of all events currently in the ring (oldest first).
    pub fn snapshot(&self) -> Vec<Event> {
        let buf = self.inner.buf.read();
        buf.iter().cloned().collect()
    }

    /// Subscribe to live events. Returns a broadcast receiver.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.inner.tx.subscribe()
    }

    /// Current number of events stored.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.inner.buf.read().len()
    }

    /// Whether the ring is empty.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.inner.buf.read().is_empty()
    }
}

// ---------------------------------------------------------------------------
// Custom tracing Layer that captures events into the ring
// ---------------------------------------------------------------------------

/// A [`tracing_subscriber::Layer`] that captures every event into a
/// [`RingBuffer`].
#[derive(Debug, Clone)]
pub struct RingLayer {
    ring: RingBuffer,
}

impl RingLayer {
    pub fn new(ring: RingBuffer) -> Self {
        Self { ring }
    }
}

/// Visitor that collects event fields into a `serde_json::Map`.
struct JsonVisitor {
    map: serde_json::Map<String, serde_json::Value>,
}

impl JsonVisitor {
    fn new() -> Self {
        Self {
            map: serde_json::Map::new(),
        }
    }
}

impl Visit for JsonVisitor {
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.map
            .insert(field.name().to_string(), serde_json::Value::from(value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.map
            .insert(field.name().to_string(), serde_json::Value::from(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.map
            .insert(field.name().to_string(), serde_json::Value::from(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.map
            .insert(field.name().to_string(), serde_json::Value::from(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.map
            .insert(field.name().to_string(), serde_json::Value::from(value));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.map.insert(
            field.name().to_string(),
            serde_json::Value::from(format!("{:?}", value)),
        );
    }
}

impl<S> tracing_subscriber::Layer<S> for RingLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let metadata = event.metadata();

        let mut visitor = JsonVisitor::new();
        event.record(&mut visitor);

        let ring_event = Event {
            ts: SystemTime::now(),
            level: EventLevel::from(metadata.level()),
            target: metadata.target().to_string(),
            fields: serde_json::Value::Object(visitor.map),
        };

        self.ring.push(ring_event);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg_attr(miri, ignore)]
    #[test]
    fn test_ring_capacity_eviction() {
        let ring = RingBuffer::new(3);

        for i in 0..5 {
            ring.push(Event {
                ts: SystemTime::now(),
                level: EventLevel::Info,
                target: format!("event_{}", i),
                fields: serde_json::Value::Null,
            });
        }

        let snap = ring.snapshot();
        assert_eq!(snap.len(), 3);
        // Oldest two (event_0, event_1) should have been evicted
        assert_eq!(snap[0].target, "event_2");
        assert_eq!(snap[1].target, "event_3");
        assert_eq!(snap[2].target, "event_4");
    }

    #[test]
    fn test_ring_empty() {
        let ring = RingBuffer::new(10);
        assert!(ring.is_empty());
        assert_eq!(ring.len(), 0);
        assert!(ring.snapshot().is_empty());
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn test_ring_under_capacity() {
        let ring = RingBuffer::new(10);
        ring.push(Event {
            ts: SystemTime::now(),
            level: EventLevel::Debug,
            target: "test".into(),
            fields: serde_json::json!({"msg": "hello"}),
        });
        assert_eq!(ring.len(), 1);
        let snap = ring.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].target, "test");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn test_subscribe_receives_events() {
        let ring = RingBuffer::new(16);
        let mut rx = ring.subscribe();

        ring.push(Event {
            ts: SystemTime::now(),
            level: EventLevel::Warn,
            target: "live".into(),
            fields: serde_json::Value::Null,
        });

        let received = rx.recv().await.expect("should receive event");
        assert_eq!(received.target, "live");
        assert_eq!(received.level, EventLevel::Warn);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn test_subscribe_multiple_events() {
        let ring = RingBuffer::new(16);
        let mut rx = ring.subscribe();

        for i in 0..3 {
            ring.push(Event {
                ts: SystemTime::now(),
                level: EventLevel::Info,
                target: format!("evt_{}", i),
                fields: serde_json::Value::Null,
            });
        }

        for i in 0..3 {
            let received = rx.recv().await.expect("should receive event");
            assert_eq!(received.target, format!("evt_{}", i));
        }
    }

    #[test]
    fn test_event_level_from_tracing_level() {
        assert_eq!(EventLevel::from(&Level::TRACE), EventLevel::Trace);
        assert_eq!(EventLevel::from(&Level::DEBUG), EventLevel::Debug);
        assert_eq!(EventLevel::from(&Level::INFO), EventLevel::Info);
        assert_eq!(EventLevel::from(&Level::WARN), EventLevel::Warn);
        assert_eq!(EventLevel::from(&Level::ERROR), EventLevel::Error);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn test_capacity_one() {
        let ring = RingBuffer::new(1);
        ring.push(Event {
            ts: SystemTime::now(),
            level: EventLevel::Info,
            target: "a".into(),
            fields: serde_json::Value::Null,
        });
        ring.push(Event {
            ts: SystemTime::now(),
            level: EventLevel::Info,
            target: "b".into(),
            fields: serde_json::Value::Null,
        });
        let snap = ring.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].target, "b");
    }
}
