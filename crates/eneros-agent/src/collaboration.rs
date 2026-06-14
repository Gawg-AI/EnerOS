use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

/// Role in the collaboration protocol
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CollaborationRole {
    /// Coordinates task distribution and monitors progress
    Coordinator,
    /// Executes assigned tasks and reports results
    Executor,
    /// Provides analysis and recommendations
    Advisor,
}

/// Status of a task assignment
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    /// Task has been assigned but not started
    Assigned,
    /// Task is in progress
    InProgress,
    /// Task completed successfully
    Completed,
    /// Task failed
    Failed,
}

/// A task assignment in the collaboration protocol
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskAssignment {
    /// Task ID
    pub id: String,
    /// Agent assigned to this task
    pub assignee_id: String,
    /// Task description
    pub description: String,
    /// Role expected for this task
    pub role: CollaborationRole,
    /// Current status
    pub status: TaskStatus,
    /// Creation timestamp
    pub created_at: DateTime<Utc>,
    /// Optional deadline
    pub deadline: Option<DateTime<Utc>>,
    /// Result description (when completed/failed)
    pub result: Option<String>,
}

impl TaskAssignment {
    /// Create a new task assignment
    pub fn new(assignee_id: &str, description: &str, role: CollaborationRole) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            assignee_id: assignee_id.to_string(),
            description: description.to_string(),
            role,
            status: TaskStatus::Assigned,
            created_at: Utc::now(),
            deadline: None,
            result: None,
        }
    }

    /// Set deadline
    pub fn with_deadline(mut self, deadline: DateTime<Utc>) -> Self {
        self.deadline = Some(deadline);
        self
    }

    /// Mark task as in progress
    pub fn start(&mut self) {
        self.status = TaskStatus::InProgress;
    }

    /// Mark task as completed
    pub fn complete(&mut self, result: &str) {
        self.status = TaskStatus::Completed;
        self.result = Some(result.to_string());
    }

    /// Mark task as failed
    pub fn fail(&mut self, reason: &str) {
        self.status = TaskStatus::Failed;
        self.result = Some(reason.to_string());
    }
}

/// Collaboration protocol for multi-agent coordination
pub struct CollaborationProtocol {
    /// Active tasks
    tasks: Vec<TaskAssignment>,
    /// Agent role assignments: agent_id -> role
    roles: std::collections::HashMap<String, CollaborationRole>,
}

impl CollaborationProtocol {
    /// Create a new collaboration protocol
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            roles: std::collections::HashMap::new(),
        }
    }

    /// Assign a role to an agent
    pub fn assign_role(&mut self, agent_id: &str, role: CollaborationRole) {
        self.roles.insert(agent_id.to_string(), role);
    }

    /// Get the role of an agent
    pub fn get_role(&self, agent_id: &str) -> Option<&CollaborationRole> {
        self.roles.get(agent_id)
    }

    /// Create a new task and assign it to an agent
    pub fn assign_task(&mut self, assignee_id: &str, description: &str, role: CollaborationRole) -> &TaskAssignment {
        let task = TaskAssignment::new(assignee_id, description, role);
        self.tasks.push(task);
        self.tasks.last().unwrap()
    }

    /// Get tasks for a specific agent
    pub fn tasks_for_agent(&self, agent_id: &str) -> Vec<&TaskAssignment> {
        self.tasks.iter().filter(|t| t.assignee_id == agent_id).collect()
    }

    /// Get pending tasks (Assigned or InProgress)
    pub fn pending_tasks(&self) -> Vec<&TaskAssignment> {
        self.tasks.iter().filter(|t| t.status != TaskStatus::Completed && t.status != TaskStatus::Failed).collect()
    }

    /// Get a mutable task by ID
    pub fn get_task_mut(&mut self, task_id: &str) -> Option<&mut TaskAssignment> {
        self.tasks.iter_mut().find(|t| t.id == task_id)
    }

    /// Get all tasks
    pub fn all_tasks(&self) -> &[TaskAssignment] {
        &self.tasks
    }

    /// Get agent count
    pub fn agent_count(&self) -> usize {
        self.roles.len()
    }
}

impl Default for CollaborationProtocol {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_assignment_creation() {
        let task = TaskAssignment::new("agent-1", "Analyze voltage", CollaborationRole::Advisor);
        assert_eq!(task.assignee_id, "agent-1");
        assert_eq!(task.description, "Analyze voltage");
        assert_eq!(task.role, CollaborationRole::Advisor);
        assert_eq!(task.status, TaskStatus::Assigned);
        assert!(task.deadline.is_none());
        assert!(task.result.is_none());
        assert!(!task.id.is_empty());
    }

    #[test]
    fn test_task_lifecycle_assigned_to_completed() {
        let mut task = TaskAssignment::new("agent-1", "Fix overload", CollaborationRole::Executor);
        assert_eq!(task.status, TaskStatus::Assigned);

        task.start();
        assert_eq!(task.status, TaskStatus::InProgress);

        task.complete("Overload resolved by shedding load");
        assert_eq!(task.status, TaskStatus::Completed);
        assert_eq!(task.result, Some("Overload resolved by shedding load".to_string()));
    }

    #[test]
    fn test_task_failure_path() {
        let mut task = TaskAssignment::new("agent-2", "Restore power", CollaborationRole::Executor);
        task.start();
        task.fail("Equipment unavailable");
        assert_eq!(task.status, TaskStatus::Failed);
        assert_eq!(task.result, Some("Equipment unavailable".to_string()));
    }

    #[test]
    fn test_collaboration_protocol_assign_roles_and_tasks() {
        let mut protocol = CollaborationProtocol::new();

        protocol.assign_role("coordinator-1", CollaborationRole::Coordinator);
        protocol.assign_role("executor-1", CollaborationRole::Executor);
        protocol.assign_role("advisor-1", CollaborationRole::Advisor);

        assert_eq!(protocol.agent_count(), 3);
        assert_eq!(protocol.get_role("coordinator-1"), Some(&CollaborationRole::Coordinator));
        assert_eq!(protocol.get_role("executor-1"), Some(&CollaborationRole::Executor));
        assert_eq!(protocol.get_role("advisor-1"), Some(&CollaborationRole::Advisor));
        assert_eq!(protocol.get_role("unknown"), None);

        let task1 = protocol.assign_task("executor-1", "Switch capacitor", CollaborationRole::Executor);
        let task1_id = task1.id.clone();

        let task2 = protocol.assign_task("advisor-1", "Analyze stability", CollaborationRole::Advisor);
        let task2_id = task2.id.clone();

        assert_eq!(protocol.all_tasks().len(), 2);

        let executor_tasks = protocol.tasks_for_agent("executor-1");
        assert_eq!(executor_tasks.len(), 1);
        assert_eq!(executor_tasks[0].description, "Switch capacitor");

        let advisor_tasks = protocol.tasks_for_agent("advisor-1");
        assert_eq!(advisor_tasks.len(), 1);
        assert_eq!(advisor_tasks[0].description, "Analyze stability");

        // Update task via get_task_mut
        let t = protocol.get_task_mut(&task1_id).unwrap();
        t.start();
        t.complete("Capacitor switched successfully");

        let t = protocol.get_task_mut(&task2_id).unwrap();
        t.start();

        assert_eq!(protocol.all_tasks()[0].status, TaskStatus::Completed);
        assert_eq!(protocol.all_tasks()[1].status, TaskStatus::InProgress);
    }

    #[test]
    fn test_pending_tasks_filtering() {
        let mut protocol = CollaborationProtocol::new();

        let t1 = protocol.assign_task("a1", "Task 1", CollaborationRole::Executor);
        let t1_id = t1.id.clone();
        let t2 = protocol.assign_task("a2", "Task 2", CollaborationRole::Advisor);
        let t2_id = t2.id.clone();
        let t3 = protocol.assign_task("a3", "Task 3", CollaborationRole::Coordinator);
        let t3_id = t3.id.clone();

        // All 3 are Assigned → all pending
        assert_eq!(protocol.pending_tasks().len(), 3);

        // Complete task 1
        protocol.get_task_mut(&t1_id).unwrap().start();
        protocol.get_task_mut(&t1_id).unwrap().complete("Done");
        assert_eq!(protocol.pending_tasks().len(), 2);

        // Fail task 2
        protocol.get_task_mut(&t2_id).unwrap().fail("Error");
        assert_eq!(protocol.pending_tasks().len(), 1);

        // Start task 3 → still pending (InProgress)
        protocol.get_task_mut(&t3_id).unwrap().start();
        assert_eq!(protocol.pending_tasks().len(), 1);
    }

    #[test]
    fn test_task_with_deadline() {
        let deadline = Utc::now() + chrono::Duration::hours(2);
        let task = TaskAssignment::new("agent-1", "Urgent task", CollaborationRole::Executor)
            .with_deadline(deadline);
        assert_eq!(task.deadline, Some(deadline));
    }
}
