/// Structure representing the entire patch graph.
/// A collection of nodes (Max objects) and edges (patch cords).
#[derive(Debug, Clone)]
pub struct PatchGraph {
    pub nodes: Vec<PatchNode>,
    pub edges: Vec<PatchEdge>,
}

/// Node purity classification.
/// Whether an object has internal state (stateful) or depends only on inputs (pure).
#[derive(Debug, Clone, PartialEq)]
pub enum NodePurity {
    /// Depends only on inputs (cycle~, +~, biquad~, etc.)
    Pure,
    /// Has internal state (pack, int, float, toggle, counter, etc.)
    Stateful,
    /// No information in objdb, or unclassifiable
    Unknown,
}

/// A single node (Max object) in the patch graph.
#[derive(Debug, Clone)]
pub struct PatchNode {
    pub id: String,
    pub object_name: String,
    pub args: Vec<String>,
    pub num_inlets: u32,
    pub num_outlets: u32,
    /// Whether this is a Signal object (name ends with `~`).
    /// Signal object fanouts don't need trigger insertion.
    pub is_signal: bool,
    /// flutmax wire name. Output as Max's varname attribute.
    /// None for inlets/outlets and auto-inserted triggers.
    pub varname: Option<String>,
    /// Whether each inlet is hot. hot_inlets[i] = true means inlet i is hot.
    /// Empty means unset (default: inlet 0 is hot, others are cold).
    pub hot_inlets: Vec<bool>,
    /// Purity classification of the object.
    pub purity: NodePurity,
    /// Attributes specified via `.attr()` chain. Vector of key-value pairs.
    /// Added as `@key value` to newobj text in codegen,
    /// or output as top-level fields in box JSON for UI objects.
    pub attrs: Vec<(String, String)>,
    /// Inline code for codebox. Used by v8.codebox / codebox (gen~).
    /// Output as the `code` field in .maxpat JSON.
    pub code: Option<String>,
}

/// A single edge (patch cord) in the patch graph.
#[derive(Debug, Clone)]
pub struct PatchEdge {
    pub source_id: String,
    pub source_outlet: u32,
    pub dest_id: String,
    pub dest_inlet: u32,
    /// Whether this is a feedback edge (cyclic edge between tapin~ and tapout~).
    /// Feedback edges are excluded from topological sort and trigger insertion.
    pub is_feedback: bool,
    /// Edge order for fanouts.
    /// Assigned 0, 1, 2... when multiple edges share the same (source_id, source_outlet).
    /// None for single edges.
    pub order: Option<u32>,
}

impl PatchGraph {
    /// Create an empty patch graph.
    pub fn new() -> Self {
        PatchGraph {
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }

    /// Add a node and return a reference to its ID.
    pub fn add_node(&mut self, node: PatchNode) -> &str {
        self.nodes.push(node);
        &self.nodes.last().unwrap().id
    }

    /// Add an edge.
    pub fn add_edge(&mut self, edge: PatchEdge) {
        self.edges.push(edge);
    }

    /// Find a node by ID.
    pub fn find_node(&self, id: &str) -> Option<&PatchNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Find a node by ID with mutable reference.
    pub fn find_node_mut(&mut self, id: &str) -> Option<&mut PatchNode> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }
}

impl Default for PatchGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_purity_equality() {
        assert_eq!(NodePurity::Pure, NodePurity::Pure);
        assert_eq!(NodePurity::Stateful, NodePurity::Stateful);
        assert_eq!(NodePurity::Unknown, NodePurity::Unknown);
        assert_ne!(NodePurity::Pure, NodePurity::Stateful);
        assert_ne!(NodePurity::Pure, NodePurity::Unknown);
    }

    #[test]
    fn test_patch_edge_order_default() {
        let edge = PatchEdge {
            source_id: "a".into(),
            source_outlet: 0,
            dest_id: "b".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        };
        assert_eq!(edge.order, None);
    }

    #[test]
    fn test_patch_edge_order_set() {
        let edge = PatchEdge {
            source_id: "a".into(),
            source_outlet: 0,
            dest_id: "b".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: Some(2),
        };
        assert_eq!(edge.order, Some(2));
    }

    #[test]
    fn test_patch_node_hot_inlets() {
        let node = PatchNode {
            id: "test".into(),
            object_name: "cycle~".into(),
            args: vec![],
            num_inlets: 2,
            num_outlets: 1,
            is_signal: true,
            varname: None,
            hot_inlets: vec![true, false],
            purity: NodePurity::Pure,
            attrs: vec![],
            code: None,
        };
        assert!(node.hot_inlets[0]);
        assert!(!node.hot_inlets[1]);
        assert_eq!(node.purity, NodePurity::Pure);
    }
}
