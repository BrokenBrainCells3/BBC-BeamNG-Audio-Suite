use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

// ============================================
// LOG THROTTLE - Print console logs only once per 10 seconds
// ============================================
static LAST_LOG_TIME: AtomicU64 = AtomicU64::new(0);
const LOG_INTERVAL_MS: u64 = 10000;

pub(crate) fn should_log() -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let last = LAST_LOG_TIME.load(Ordering::Relaxed);
    if now - last >= LOG_INTERVAL_MS {
        LAST_LOG_TIME
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    } else {
        false
    }
}

pub(crate) fn log_throttled(msg: &str) {
    if should_log() {
        eprintln!("{}", msg);
    }
}

// Fix #4: Event queue for non-blocking emits (using lazy_static pattern with std)
pub(crate) struct EventQueue {
    events: Mutex<Vec<(String, serde_json::Value)>>,
}

impl EventQueue {
    fn new() -> Self {
        Self { events: Mutex::new(Vec::new()) }
    }
    
    pub fn push(&self, event: String, value: serde_json::Value) {
        if let Ok(mut queue) = self.events.lock() {
            queue.push((event, value));
        }
    }
    
    pub fn drain(&self) -> Vec<(String, serde_json::Value)> {
        if let Ok(mut queue) = self.events.lock() {
            std::mem::take(&mut *queue)
        } else {
            Vec::new()
        }
    }
}

// Global event queue instance using std::sync::OnceLock (Rust 1.70+)
static EVENT_QUEUE: OnceLock<Arc<EventQueue>> = OnceLock::new();

pub(crate) fn get_event_queue() -> &'static Arc<EventQueue> {
    EVENT_QUEUE.get_or_init(|| Arc::new(EventQueue::new()))
}

// Fix #4: Non-blocking event emit helper
// Queues events instead of emitting directly from audio callback
pub(crate) fn emit_band_event(event_queue: &Arc<EventQueue>, name: &str, value: f32) {
    event_queue.push(name.to_string(), serde_json::json!(value));
}
