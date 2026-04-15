use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Severity level for in-editor messages.
/// Mirrors tracing levels but is independent of the tracing crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MessageLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl std::fmt::Display for MessageLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageLevel::Trace => write!(f, "TRACE"),
            MessageLevel::Debug => write!(f, "DEBUG"),
            MessageLevel::Info => write!(f, "INFO"),
            MessageLevel::Warn => write!(f, "WARN"),
            MessageLevel::Error => write!(f, "ERROR"),
        }
    }
}

/// A single entry in the editor's *Messages* buffer.
///
/// Emacs lesson: `*Messages*` is one of the most-used debugging tools.
/// Ours is structured (level, target, timestamp) so the renderer can
/// style it and the AI agent can read it programmatically.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub level: MessageLevel,
    pub target: String,
    pub message: String,
    /// Monotonic sequence number for ordering.
    pub seq: u64,
}

/// Thread-safe ring buffer of log entries.
///
/// Shared between the tracing layer (which writes) and the editor/renderer
/// (which reads). The Arc<Mutex<>> is justified because the tracing layer
/// runs on multiple threads (AI task, main task) and must be Send+Sync.
///
/// Max capacity prevents unbounded memory growth. Old entries are evicted.
pub struct MessageLog {
    inner: Arc<Mutex<MessageLogInner>>,
}

struct MessageLogInner {
    entries: VecDeque<LogEntry>,
    max_entries: usize,
    next_seq: u64,
}

impl MessageLog {
    pub fn new(max_entries: usize) -> Self {
        MessageLog {
            inner: Arc::new(Mutex::new(MessageLogInner {
                entries: VecDeque::with_capacity(max_entries),
                max_entries,
                next_seq: 0,
            })),
        }
    }

    /// Push a new log entry. Evicts oldest if at capacity.
    pub fn push(&self, level: MessageLevel, target: impl Into<String>, message: impl Into<String>) {
        let mut inner = self.inner.lock().unwrap();
        let seq = inner.next_seq;
        inner.next_seq += 1;
        if inner.entries.len() >= inner.max_entries {
            inner.entries.pop_front();
        }
        inner.entries.push_back(LogEntry {
            level,
            target: target.into(),
            message: message.into(),
            seq,
        });
    }

    /// Get a snapshot of all entries (for rendering).
    pub fn entries(&self) -> Vec<LogEntry> {
        let inner = self.inner.lock().unwrap();
        inner.entries.iter().cloned().collect()
    }

    /// Get entries at or above a minimum level.
    pub fn entries_filtered(&self, min_level: MessageLevel) -> Vec<LogEntry> {
        let inner = self.inner.lock().unwrap();
        inner
            .entries
            .iter()
            .filter(|e| e.level >= min_level)
            .cloned()
            .collect()
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        let inner = self.inner.lock().unwrap();
        inner.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clone the Arc handle for sharing between threads.
    pub fn handle(&self) -> MessageLogHandle {
        MessageLogHandle {
            inner: self.inner.clone(),
        }
    }
}

/// Cheap cloneable handle to the message log. Send + Sync.
/// Used by the tracing layer to write from any thread.
#[derive(Clone)]
pub struct MessageLogHandle {
    inner: Arc<Mutex<MessageLogInner>>,
}

impl MessageLogHandle {
    pub fn push(&self, level: MessageLevel, target: impl Into<String>, message: impl Into<String>) {
        let mut inner = self.inner.lock().unwrap();
        let seq = inner.next_seq;
        inner.next_seq += 1;
        if inner.entries.len() >= inner.max_entries {
            inner.entries.pop_front();
        }
        inner.entries.push_back(LogEntry {
            level,
            target: target.into(),
            message: message.into(),
            seq,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_read_entries() {
        let log = MessageLog::new(100);
        log.push(MessageLevel::Info, "test", "hello");
        log.push(MessageLevel::Error, "test", "oh no");

        let entries = log.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].message, "hello");
        assert_eq!(entries[0].level, MessageLevel::Info);
        assert_eq!(entries[1].message, "oh no");
        assert_eq!(entries[1].level, MessageLevel::Error);
        assert_eq!(entries[0].seq, 0);
        assert_eq!(entries[1].seq, 1);
    }

    #[test]
    fn ring_buffer_evicts_oldest() {
        let log = MessageLog::new(3);
        log.push(MessageLevel::Info, "t", "a");
        log.push(MessageLevel::Info, "t", "b");
        log.push(MessageLevel::Info, "t", "c");
        log.push(MessageLevel::Info, "t", "d");

        let entries = log.entries();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].message, "b");
        assert_eq!(entries[2].message, "d");
    }

    #[test]
    fn filtered_entries() {
        let log = MessageLog::new(100);
        log.push(MessageLevel::Debug, "t", "debug msg");
        log.push(MessageLevel::Info, "t", "info msg");
        log.push(MessageLevel::Warn, "t", "warn msg");
        log.push(MessageLevel::Error, "t", "error msg");

        let warnings = log.entries_filtered(MessageLevel::Warn);
        assert_eq!(warnings.len(), 2);
        assert_eq!(warnings[0].message, "warn msg");
        assert_eq!(warnings[1].message, "error msg");
    }

    #[test]
    fn handle_is_send_sync() {
        let log = MessageLog::new(100);
        let handle = log.handle();
        // Prove it's Send + Sync
        fn assert_send_sync<T: Send + Sync>(_: &T) {}
        assert_send_sync(&handle);

        handle.push(MessageLevel::Info, "thread", "from handle");
        let entries = log.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].message, "from handle");
    }
}
