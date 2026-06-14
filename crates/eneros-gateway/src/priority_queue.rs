use std::collections::VecDeque;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::Notify;

use super::command::{Command, CommandPriority};

/// Priority-aware command queue.
///
/// Uses 4 internal `VecDeque` buckets (one per priority level).
/// Higher-priority commands are always dequeued before lower-priority ones.
/// Within the same priority level, FIFO order is preserved.
pub struct PriorityCommandQueue {
    /// [Low, Normal, High, Critical] — index matches `CommandPriority` discriminant
    queues: [VecDeque<Command>; 4],
    /// Total number of pending commands across all queues
    pending_count: usize,
    /// Notifier for async consumers waiting on `dequeue_async`
    notify: Arc<Notify>,
}

impl PriorityCommandQueue {
    /// Create a new empty priority command queue.
    pub fn new() -> Self {
        Self {
            queues: [
                VecDeque::new(),
                VecDeque::new(),
                VecDeque::new(),
                VecDeque::new(),
            ],
            pending_count: 0,
            notify: Arc::new(Notify::new()),
        }
    }

    /// Enqueue a command according to its priority.
    pub fn enqueue(&mut self, cmd: Command) {
        let idx = priority_index(&cmd.priority);
        self.queues[idx].push_back(cmd);
        self.pending_count += 1;
        self.notify.notify_one();
    }

    /// Dequeue the highest-priority command available.
    /// Returns `None` if the queue is empty.
    pub fn dequeue(&mut self) -> Option<Command> {
        // Iterate from Critical (3) down to Low (0)
        for i in (0..4).rev() {
            if let Some(cmd) = self.queues[i].pop_front() {
                self.pending_count -= 1;
                return Some(cmd);
            }
        }
        None
    }

    /// Peek at the highest-priority command without removing it.
    pub fn peek(&self) -> Option<&Command> {
        for i in (0..4).rev() {
            if let Some(cmd) = self.queues[i].front() {
                return Some(cmd);
            }
        }
        None
    }

    /// Total number of pending commands across all priority levels.
    pub fn len(&self) -> usize {
        self.pending_count
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.pending_count == 0
    }

    /// Number of pending commands at a specific priority level.
    pub fn len_by_priority(&self, level: CommandPriority) -> usize {
        self.queues[priority_index(&level)].len()
    }

    /// Get a reference to the internal `Notify` for async waiting.
    pub fn notify(&self) -> &Arc<Notify> {
        &self.notify
    }

    /// Clear all pending commands.
    pub fn clear(&mut self) {
        for q in &mut self.queues {
            q.clear();
        }
        self.pending_count = 0;
    }
}

impl Default for PriorityCommandQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe wrapper around `PriorityCommandQueue`.
pub struct SharedPriorityCommandQueue {
    inner: Mutex<PriorityCommandQueue>,
    notify: Arc<Notify>,
}

impl SharedPriorityCommandQueue {
    /// Create a new shared priority command queue.
    pub fn new() -> Self {
        let inner = PriorityCommandQueue::new();
        let notify = inner.notify().clone();
        Self {
            inner: Mutex::new(inner),
            notify,
        }
    }

    /// Enqueue a command (thread-safe).
    pub fn enqueue(&self, cmd: Command) {
        self.inner.lock().enqueue(cmd);
    }

    /// Dequeue the highest-priority command (thread-safe).
    pub fn dequeue(&self) -> Option<Command> {
        self.inner.lock().dequeue()
    }

    /// Peek at the highest-priority command (thread-safe).
    pub fn peek(&self) -> Option<Command> {
        self.inner.lock().peek().cloned()
    }

    /// Total number of pending commands.
    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.lock().is_empty()
    }

    /// Number of pending commands at a specific priority level.
    pub fn len_by_priority(&self, level: CommandPriority) -> usize {
        self.inner.lock().len_by_priority(level)
    }

    /// Asynchronously wait for and dequeue a command.
    /// If the queue is currently empty, waits until a command is enqueued.
    pub async fn dequeue_async(&self) -> Command {
        loop {
            if let Some(cmd) = self.dequeue() {
                return cmd;
            }
            self.notify.notified().await;
        }
    }

    /// Clear all pending commands.
    pub fn clear(&self) {
        self.inner.lock().clear();
    }
}

impl Default for SharedPriorityCommandQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// Map `CommandPriority` to array index.
/// Low=0, Normal=1, High=2, Critical=3
fn priority_index(priority: &CommandPriority) -> usize {
    match priority {
        CommandPriority::Low => 0,
        CommandPriority::Normal => 1,
        CommandPriority::High => 2,
        CommandPriority::Critical => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CommandType;

    fn make_cmd(priority: CommandPriority, id_suffix: &str) -> Command {
        Command::new(
            CommandType::SwitchOperation,
            1,
            priority,
            &format!("test-{}", id_suffix),
        )
    }

    #[test]
    fn test_enqueue_dequeue_priority_order() {
        let mut q = PriorityCommandQueue::new();

        // Enqueue in reverse priority order
        q.enqueue(make_cmd(CommandPriority::Low, "low"));
        q.enqueue(make_cmd(CommandPriority::Normal, "normal"));
        q.enqueue(make_cmd(CommandPriority::High, "high"));
        q.enqueue(make_cmd(CommandPriority::Critical, "critical"));

        // Should dequeue in priority order: Critical first
        assert_eq!(q.dequeue().unwrap().priority, CommandPriority::Critical);
        assert_eq!(q.dequeue().unwrap().priority, CommandPriority::High);
        assert_eq!(q.dequeue().unwrap().priority, CommandPriority::Normal);
        assert_eq!(q.dequeue().unwrap().priority, CommandPriority::Low);
        assert!(q.dequeue().is_none());
    }

    #[test]
    fn test_fifo_within_same_priority() {
        let mut q = PriorityCommandQueue::new();

        q.enqueue(make_cmd(CommandPriority::Normal, "first"));
        q.enqueue(make_cmd(CommandPriority::Normal, "second"));
        q.enqueue(make_cmd(CommandPriority::Normal, "third"));

        let first = q.dequeue().unwrap();
        let second = q.dequeue().unwrap();
        let third = q.dequeue().unwrap();

        assert_eq!(first.source, "test-first");
        assert_eq!(second.source, "test-second");
        assert_eq!(third.source, "test-third");
    }

    #[test]
    fn test_critical_preempts_normal() {
        let mut q = PriorityCommandQueue::new();

        q.enqueue(make_cmd(CommandPriority::Normal, "n1"));
        q.enqueue(make_cmd(CommandPriority::Normal, "n2"));
        q.enqueue(make_cmd(CommandPriority::Critical, "c1")); // inserted between normals

        // Critical should come out first
        assert_eq!(q.dequeue().unwrap().source, "test-c1");
        assert_eq!(q.dequeue().unwrap().source, "test-n1");
        assert_eq!(q.dequeue().unwrap().source, "test-n2");
    }

    #[test]
    fn test_peek_does_not_remove() {
        let mut q = PriorityCommandQueue::new();
        q.enqueue(make_cmd(CommandPriority::High, "h1"));

        assert_eq!(q.peek().unwrap().source, "test-h1");
        assert_eq!(q.len(), 1); // still there

        assert_eq!(q.dequeue().unwrap().source, "test-h1");
        assert_eq!(q.len(), 0);
    }

    #[test]
    fn test_len_by_priority() {
        let mut q = PriorityCommandQueue::new();
        q.enqueue(make_cmd(CommandPriority::Low, "l1"));
        q.enqueue(make_cmd(CommandPriority::Low, "l2"));
        q.enqueue(make_cmd(CommandPriority::Critical, "c1"));

        assert_eq!(q.len_by_priority(CommandPriority::Low), 2);
        assert_eq!(q.len_by_priority(CommandPriority::Normal), 0);
        assert_eq!(q.len_by_priority(CommandPriority::High), 0);
        assert_eq!(q.len_by_priority(CommandPriority::Critical), 1);
        assert_eq!(q.len(), 3);
    }

    #[test]
    fn test_empty_queue() {
        let mut q = PriorityCommandQueue::new();
        assert!(q.is_empty());
        assert_eq!(q.len(), 0);
        assert!(q.dequeue().is_none());
        assert!(q.peek().is_none());
    }

    #[test]
    fn test_clear() {
        let mut q = PriorityCommandQueue::new();
        q.enqueue(make_cmd(CommandPriority::Low, "l1"));
        q.enqueue(make_cmd(CommandPriority::Critical, "c1"));
        assert_eq!(q.len(), 2);

        q.clear();
        assert!(q.is_empty());
        assert!(q.dequeue().is_none());
    }

    #[test]
    fn test_shared_enqueue_dequeue() {
        let q = SharedPriorityCommandQueue::new();

        q.enqueue(make_cmd(CommandPriority::Low, "low"));
        q.enqueue(make_cmd(CommandPriority::Critical, "critical"));

        assert_eq!(q.dequeue().unwrap().priority, CommandPriority::Critical);
        assert_eq!(q.dequeue().unwrap().priority, CommandPriority::Low);
        assert!(q.dequeue().is_none());
    }

    #[tokio::test]
    async fn test_shared_dequeue_async() {
        let q = Arc::new(SharedPriorityCommandQueue::new());

        // Spawn a task that enqueues after a delay
        let q_clone = q.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            q_clone.enqueue(make_cmd(CommandPriority::Critical, "delayed"));
        });

        // Should block until the command arrives
        let cmd = q.dequeue_async().await;
        assert_eq!(cmd.priority, CommandPriority::Critical);
        assert_eq!(cmd.source, "test-delayed");
    }

    #[test]
    fn test_priority_index_mapping() {
        assert_eq!(priority_index(&CommandPriority::Low), 0);
        assert_eq!(priority_index(&CommandPriority::Normal), 1);
        assert_eq!(priority_index(&CommandPriority::High), 2);
        assert_eq!(priority_index(&CommandPriority::Critical), 3);
    }
}
