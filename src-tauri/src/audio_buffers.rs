use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

pub(crate) static STREAM_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Generic ring buffer for audio data.
/// Uses VecDeque for O(1) push/pop instead of O(n) Vec::remove(0).
pub struct RingBuffer<T> {
    buffer: std::sync::Mutex<std::collections::VecDeque<T>>,
    capacity: usize,
}

impl<T: Clone> RingBuffer<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: std::sync::Mutex::new(std::collections::VecDeque::with_capacity(capacity)),
            capacity,
        }
    }

    pub fn push(&self, item: T) {
        if let Ok(mut g) = self.buffer.try_lock() {
            if g.len() >= self.capacity {
                g.pop_front();
            }
            g.push_back(item);
        }
    }

    pub fn extend<I: IntoIterator<Item = T>>(&self, iter: I) {
        if let Ok(mut g) = self.buffer.try_lock() {
            g.extend(iter);
            while g.len() > self.capacity {
                g.pop_front();
            }
        }
    }

    pub fn pop(&self) -> Option<T> {
        if let Ok(mut g) = self.buffer.try_lock() {
            g.pop_front() // O(1)
        } else {
            None
        }
    }

    pub fn pop_many(&self, count: usize) -> Vec<T> {
        if let Ok(mut g) = self.buffer.try_lock() {
            let actual_count = count.min(g.len());
            let mut result = Vec::with_capacity(actual_count);
            for _ in 0..actual_count {
                if let Some(item) = g.pop_front() {
                    result.push(item);
                }
            }
            result
        } else {
            Vec::new()
        }
    }

    pub fn drain(&self) -> Vec<T> {
        if let Ok(mut g) = self.buffer.lock() {
            g.drain(..).collect()
        } else {
            Vec::new()
        }
    }
}

pub(crate) type PassthroughBuffer = RingBuffer<(f32, f32)>;

pub(crate) fn create_passthrough_buffer(size: usize) -> Arc<PassthroughBuffer> {
    Arc::new(RingBuffer::new(size))
}
