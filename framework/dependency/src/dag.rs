//! # Precompiled Topological DAG Dependency Graph
//!
//! Replaces Python FastAPI runtime reflection with a startup-compiled topological sort.
//! Verifies no circular references exist before listening on port 3000.

use crate::scope::DependencyScope;
use aether_core::error::AetherError;
use std::collections::HashMap;

pub type NodeId = usize;

#[derive(Clone, Debug)]
pub struct DependencyNode {
    pub id: NodeId,
    pub name: &'static str,
    pub scope: DependencyScope,
    pub dependencies: Vec<NodeId>,
}

pub struct DependencyGraph {
    nodes: HashMap<NodeId, DependencyNode>,
}

impl DependencyGraph {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
        }
    }

    pub fn add_node(&mut self, node: DependencyNode) {
        self.nodes.insert(node.id, node);
    }

    /// Compiles a topological execution plan. Errors out on cycles.
    ///
    /// Two properties the previous implementation lacked:
    /// - Deterministic output. Ready nodes were taken from `HashMap` iteration order and
    ///   LIFO-popped, so graphs with more than one root produced a different plan per run.
    ///   A min-heap keyed by node id gives the same plan in every process.
    /// - Honest diagnostics. A dependency on an unknown node id starved the dependent,
    ///   which then surfaced as "circular dependency" even though no cycle existed. Unknown
    ///   references are now rejected up front as a graph-construction error.
    pub fn compile_execution_plan(&self) -> Result<Vec<NodeId>, AetherError> {
        let mut ids: Vec<NodeId> = self.nodes.keys().copied().collect();
        ids.sort_unstable();

        for &id in &ids {
            let node = &self.nodes[&id];
            for &dep in &node.dependencies {
                if !self.nodes.contains_key(&dep) {
                    return Err(AetherError::ValidationFailed(format!(
                        "dependency graph: node '{}' ({}) references unknown node {}",
                        node.name, id, dep
                    )));
                }
            }
        }

        let mut in_degree: HashMap<NodeId, usize> = ids.iter().map(|&id| (id, 0)).collect();
        let mut adj: HashMap<NodeId, Vec<NodeId>> = ids.iter().map(|&id| (id, Vec::new())).collect();

        for &id in &ids {
            for &dep in &self.nodes[&id].dependencies {
                adj.get_mut(&dep).unwrap().push(id);
                *in_degree.get_mut(&id).unwrap() += 1;
            }
        }

        let mut ready: std::collections::BinaryHeap<std::cmp::Reverse<NodeId>> = in_degree
            .iter()
            .filter(|&(_, &deg)| deg == 0)
            .map(|(&id, _)| std::cmp::Reverse(id))
            .collect();

        let mut plan = Vec::with_capacity(self.nodes.len());

        while let Some(std::cmp::Reverse(curr)) = ready.pop() {
            plan.push(curr);

            for &neighbor in &adj[&curr] {
                let deg = in_degree.get_mut(&neighbor).unwrap();
                *deg -= 1;
                if *deg == 0 {
                    ready.push(std::cmp::Reverse(neighbor));
                }
            }
        }

        if plan.len() != self.nodes.len() {
            return Err(AetherError::DependencyCycleDetected(
                "Circular dependency detected in graph".into(),
            ));
        }

        Ok(plan)
    }
}

impl Default for DependencyGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topological_sort_and_cycle_check() {
        let mut graph = DependencyGraph::new();
        // 0: DB Pool (Singleton)
        // 1: Auth Service (Singleton, depends on 0)
        // 2: Current User (RequestScoped, depends on 1)
        graph.add_node(DependencyNode {
            id: 0,
            name: "DbPool",
            scope: DependencyScope::Singleton,
            dependencies: vec![],
        });
        graph.add_node(DependencyNode {
            id: 1,
            name: "AuthService",
            scope: DependencyScope::Singleton,
            dependencies: vec![0],
        });
        graph.add_node(DependencyNode {
            id: 2,
            name: "CurrentUser",
            scope: DependencyScope::RequestScoped,
            dependencies: vec![1],
        });

        let plan = graph.compile_execution_plan().expect("Compilation succeeds");
        assert_eq!(plan, vec![0, 1, 2]);
    }

    /// Regression: a dependency on an unknown node id starved the dependent and the
    /// unfinished-plan check misreported it as a circular dependency.
    #[test]
    fn test_dangling_dependency_is_not_reported_as_cycle() {
        let mut graph = DependencyGraph::new();
        graph.add_node(DependencyNode {
            id: 0,
            name: "DbPool",
            scope: DependencyScope::Singleton,
            dependencies: vec![99],
        });
        match graph.compile_execution_plan() {
            Err(AetherError::ValidationFailed(msg)) => {
                assert!(msg.contains("unknown node 99"), "message: {msg}");
            }
            other => panic!("expected validation failure, got {other:?}"),
        }
    }

    #[test]
    fn test_real_cycle_still_detected() {
        let mut graph = DependencyGraph::new();
        graph.add_node(DependencyNode {
            id: 0,
            name: "A",
            scope: DependencyScope::Singleton,
            dependencies: vec![1],
        });
        graph.add_node(DependencyNode {
            id: 1,
            name: "B",
            scope: DependencyScope::Singleton,
            dependencies: vec![0],
        });
        assert!(matches!(
            graph.compile_execution_plan(),
            Err(AetherError::DependencyCycleDetected(_))
        ));
    }

    /// Regression: with several ready nodes the plan used to depend on `HashMap`
    /// iteration order and differ between runs. Nodes are added in scrambled order;
    /// the plan must always be the deterministic lowest-ready-first sequence.
    #[test]
    fn test_plan_is_deterministic_with_multiple_roots() {
        let mut graph = DependencyGraph::new();
        for (id, deps) in [(4usize, vec![0usize]), (1, vec![]), (3, vec![]), (0, vec![]), (2, vec![])] {
            graph.add_node(DependencyNode {
                id,
                name: "N",
                scope: DependencyScope::Singleton,
                dependencies: deps,
            });
        }
        let plan = graph.compile_execution_plan().expect("acyclic");
        assert_eq!(plan, vec![0, 1, 2, 3, 4]);
    }
}
