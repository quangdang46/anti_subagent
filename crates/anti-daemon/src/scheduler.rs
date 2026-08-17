//! Parallel scheduler — resource-aware, dependency-aware task scheduling.
//!
//! Schedules tasks across multiple peers with dependency tracking and
//! resource monitoring.

use std::collections::{HashMap, HashSet};

/// Task identifier.
pub type TaskId = String;

/// Dependency graph for tasks.
#[derive(Debug, Clone, Default)]
pub struct DagGraph {
    /// Task ID -> list of task IDs it depends on
    dependencies: HashMap<TaskId, Vec<TaskId>>,
    /// Task ID -> list of tasks that depend on it
    dependents: HashMap<TaskId, Vec<TaskId>>,
}

impl DagGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a task with its dependencies.
    pub fn add_task(&mut self, id: TaskId, deps: Vec<TaskId>) {
        for dep in &deps {
            self.dependents
                .entry(dep.clone())
                .or_default()
                .push(id.clone());
        }
        self.dependencies.insert(id, deps);
    }

    /// Get tasks with no unmet dependencies (ready to run).
    pub fn ready_tasks(&self, completed: &HashSet<TaskId>) -> Vec<TaskId> {
        self.dependencies
            .iter()
            .filter(|(id, deps)| {
                !completed.contains(id.as_str())
                    && deps.iter().all(|d| completed.contains(d.as_str()))
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Check for circular dependencies.
    pub fn has_cycle(&self) -> bool {
        let mut visited = HashSet::new();
        let mut stack = HashSet::new();

        fn dfs(
            node: &str,
            graph: &DagGraph,
            visited: &mut HashSet<String>,
            stack: &mut HashSet<String>,
        ) -> bool {
            if stack.contains(node) {
                return true;
            }
            if visited.contains(node) {
                return false;
            }
            visited.insert(node.to_string());
            stack.insert(node.to_string());
            if let Some(deps) = graph.dependencies.get(node) {
                for dep in deps {
                    if dfs(dep, graph, visited, stack) {
                        return true;
                    }
                }
            }
            stack.remove(node);
            false
        }

        for id in self.dependencies.keys() {
            if dfs(id, self, &mut visited, &mut stack) {
                return true;
            }
        }
        false
    }
}

/// Resource monitor — tracks system resource usage.
#[derive(Debug, Clone)]
pub struct ResourceMonitor {
    pub max_concurrent_peers: usize,
    pub current_peers: usize,
}

impl ResourceMonitor {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            max_concurrent_peers: max_concurrent,
            current_peers: 0,
        }
    }

    pub fn can_spawn(&self) -> bool {
        self.current_peers < self.max_concurrent_peers
    }

    pub fn peer_started(&mut self) {
        self.current_peers += 1;
    }

    pub fn peer_finished(&mut self) {
        self.current_peers = self.current_peers.saturating_sub(1);
    }
}

/// Schedule — output of the scheduler.
#[derive(Debug, Clone)]
pub struct Schedule {
    /// Tasks grouped by parallel execution waves
    pub parallel_groups: Vec<Vec<TaskId>>,
    /// Total tasks scheduled
    pub total_tasks: usize,
    /// Tasks that couldn't be scheduled (dependency issues)
    pub unscheduled: Vec<TaskId>,
}

/// Scheduler — builds and executes task schedules.
pub struct Scheduler {
    graph: DagGraph,
    resource_monitor: ResourceMonitor,
}

impl Scheduler {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            graph: DagGraph::new(),
            resource_monitor: ResourceMonitor::new(max_concurrent),
        }
    }

    /// Add a task with dependencies.
    pub fn add_task(&mut self, id: TaskId, deps: Vec<TaskId>) {
        self.graph.add_task(id, deps);
    }

    /// Build schedule — topological sort with parallel groups.
    pub fn build_schedule(&self) -> Schedule {
        let mut completed = HashSet::new();
        let mut parallel_groups = Vec::new();
        let mut unscheduled = Vec::new();

        loop {
            let ready = self.graph.ready_tasks(&completed);
            if ready.is_empty() {
                break;
            }

            // Take up to max_concurrent tasks for this wave
            let wave: Vec<TaskId> = ready
                .into_iter()
                .take(self.resource_monitor.max_concurrent_peers)
                .collect();

            if wave.is_empty() {
                break;
            }

            for id in &wave {
                completed.insert(id.clone());
            }

            parallel_groups.push(wave);
        }

        // Find unscheduled tasks (have dependencies that were never completed)
        for id in self.graph.dependencies.keys() {
            if !completed.contains(id.as_str()) {
                unscheduled.push(id.clone());
            }
        }

        let total_tasks = self.graph.dependencies.len();

        Schedule {
            parallel_groups,
            total_tasks,
            unscheduled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_tasks_in_same_group() {
        let mut scheduler = Scheduler::new(10);
        scheduler.add_task("a".into(), vec![]);
        scheduler.add_task("b".into(), vec![]);
        scheduler.add_task("c".into(), vec![]);

        let schedule = scheduler.build_schedule();
        assert_eq!(schedule.parallel_groups.len(), 1);
        assert_eq!(schedule.parallel_groups[0].len(), 3);
    }

    #[test]
    fn dependent_tasks_in_separate_groups() {
        let mut scheduler = Scheduler::new(10);
        scheduler.add_task("a".into(), vec![]);
        scheduler.add_task("b".into(), vec!["a".into()]);

        let schedule = scheduler.build_schedule();
        assert_eq!(schedule.parallel_groups.len(), 2);
        assert!(schedule.parallel_groups[0].contains(&"a".to_string()));
        assert!(schedule.parallel_groups[1].contains(&"b".to_string()));
    }

    #[test]
    fn max_concurrent_limits_parallelism() {
        let mut scheduler = Scheduler::new(2);
        scheduler.add_task("a".into(), vec![]);
        scheduler.add_task("b".into(), vec![]);
        scheduler.add_task("c".into(), vec![]);

        let schedule = scheduler.build_schedule();
        assert_eq!(schedule.parallel_groups[0].len(), 2);
        assert_eq!(schedule.parallel_groups[1].len(), 1);
    }

    #[test]
    fn cycle_detection() {
        let mut graph = DagGraph::new();
        graph.add_task("a".into(), vec!["b".into()]);
        graph.add_task("b".into(), vec!["a".into()]);

        assert!(graph.has_cycle());
    }

    #[test]
    fn no_cycle_in_dag() {
        let mut graph = DagGraph::new();
        graph.add_task("a".into(), vec![]);
        graph.add_task("b".into(), vec!["a".into()]);
        graph.add_task("c".into(), vec!["b".into()]);

        assert!(!graph.has_cycle());
    }
}
