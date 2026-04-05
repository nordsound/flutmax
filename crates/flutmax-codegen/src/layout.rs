/// Sugiyama (layered graph drawing) auto-layout algorithm.
///
/// 4-phase approach:
/// 1. Layer assignment (longest path from sources)
/// 2. Build layer structure
/// 3. Crossing reduction (barycenter heuristic)
/// 4. Coordinate assignment

use std::collections::{HashMap, VecDeque};

use flutmax_sema::graph::{PatchGraph, PatchNode};

/// Layout result: node positions and patcher dimensions.
pub struct LayoutResult {
    /// node_id -> (x, y) position
    pub positions: HashMap<String, (f64, f64)>,
    /// Computed patcher dimensions (width, height)
    pub patcher_size: (f64, f64),
}

// ─── Layout constants ───

const LAYER_SPACING: f64 = 70.0;
const NODE_SPACING: f64 = 130.0;
const MARGIN_X: f64 = 50.0;
const MARGIN_Y: f64 = 50.0;

/// Compute a Sugiyama layered layout for the given patch graph.
pub fn sugiyama_layout(graph: &PatchGraph) -> LayoutResult {
    if graph.nodes.is_empty() {
        return LayoutResult {
            positions: HashMap::new(),
            patcher_size: (200.0, 200.0),
        };
    }

    // Phase 1: Assign layers (longest path from sources)
    let node_layers = assign_layers(graph);

    // Phase 2: Build layer structure
    let mut layers = build_layers(&node_layers, graph);

    // Phase 3: Reduce crossings (barycenter heuristic)
    reduce_crossings(&mut layers, graph);

    // Phase 4: Assign coordinates
    assign_coordinates(&layers)
}

// ─── Phase 1: Layer Assignment ───

fn assign_layers(graph: &PatchGraph) -> HashMap<String, usize> {
    // Build adjacency (forward edges only, skip feedback)
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();

    for node in &graph.nodes {
        in_degree.entry(node.id.as_str()).or_insert(0);
        adjacency.entry(node.id.as_str()).or_insert_with(Vec::new);
    }

    for edge in &graph.edges {
        if edge.is_feedback {
            continue;
        }
        *in_degree.entry(edge.dest_id.as_str()).or_insert(0) += 1;
        adjacency
            .entry(edge.source_id.as_str())
            .or_default()
            .push(edge.dest_id.as_str());
    }

    // Find source nodes (in_degree == 0)
    let sources: Vec<&str> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(&id, _)| id)
        .collect();

    // BFS longest-path layer assignment
    let mut layers: HashMap<String, usize> = HashMap::new();
    let mut queue: VecDeque<&str> = VecDeque::new();

    for &src in &sources {
        layers.insert(src.to_string(), 0);
        queue.push_back(src);
    }

    // Process nodes: for each node, propagate max(current_layer + 1) to successors
    let mut visited_counts: HashMap<&str, usize> = HashMap::new();

    while let Some(current) = queue.pop_front() {
        let current_layer = layers.get(current).copied().unwrap_or(0);

        if let Some(neighbors) = adjacency.get(current) {
            for &next in neighbors {
                let new_layer = current_layer + 1;
                let existing = layers.get(next).copied().unwrap_or(0);
                if new_layer > existing {
                    layers.insert(next.to_string(), new_layer);
                }

                // Track visit count to know when all predecessors are processed
                let count = visited_counts.entry(next).or_insert(0);
                *count += 1;
                let needed = in_degree.get(next).copied().unwrap_or(0);
                if *count >= needed {
                    queue.push_back(next);
                }
            }
        }
    }

    // Handle isolated nodes (no edges at all)
    for node in &graph.nodes {
        layers.entry(node.id.clone()).or_insert(0);
    }

    layers
}

// ─── Phase 2: Build Layer Structure ───

fn build_layers<'a>(
    node_layers: &HashMap<String, usize>,
    graph: &'a PatchGraph,
) -> Vec<Vec<&'a PatchNode>> {
    let max_layer = node_layers.values().copied().max().unwrap_or(0);
    let mut layers: Vec<Vec<&PatchNode>> = vec![vec![]; max_layer + 1];

    for node in &graph.nodes {
        let layer = node_layers.get(&node.id).copied().unwrap_or(0);
        layers[layer].push(node);
    }

    // Initial ordering: inlets first, then signal objects, then control, then outlets
    for layer in &mut layers {
        layer.sort_by(|a, b| {
            let a_priority = node_sort_priority(a);
            let b_priority = node_sort_priority(b);
            a_priority.cmp(&b_priority).then(a.id.cmp(&b.id))
        });
    }

    layers
}

fn node_sort_priority(node: &PatchNode) -> u32 {
    match node.object_name.as_str() {
        "inlet" | "inlet~" => 0,
        "outlet" | "outlet~" => 3,
        _ if node.is_signal => 1,
        _ => 2,
    }
}

// ─── Phase 3: Crossing Reduction ───

fn reduce_crossings(layers: &mut [Vec<&PatchNode>], graph: &PatchGraph) {
    if layers.len() < 2 {
        return;
    }

    // Build up/down adjacency maps (non-feedback edges only)
    let mut down_adj: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut up_adj: HashMap<&str, Vec<&str>> = HashMap::new();

    for edge in &graph.edges {
        if edge.is_feedback {
            continue;
        }
        down_adj
            .entry(edge.source_id.as_str())
            .or_default()
            .push(edge.dest_id.as_str());
        up_adj
            .entry(edge.dest_id.as_str())
            .or_default()
            .push(edge.source_id.as_str());
    }

    // Barycenter heuristic: sweep down then up, repeat 4 times
    for _iteration in 0..4 {
        // Down sweep
        for layer_idx in 1..layers.len() {
            let prev_positions: HashMap<&str, f64> = layers[layer_idx - 1]
                .iter()
                .enumerate()
                .map(|(i, n)| (n.id.as_str(), i as f64))
                .collect();

            layers[layer_idx].sort_by(|a, b| {
                let a_bary = barycenter(a.id.as_str(), &up_adj, &prev_positions);
                let b_bary = barycenter(b.id.as_str(), &up_adj, &prev_positions);
                a_bary
                    .partial_cmp(&b_bary)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        // Up sweep
        for layer_idx in (0..layers.len().saturating_sub(1)).rev() {
            let next_positions: HashMap<&str, f64> = layers[layer_idx + 1]
                .iter()
                .enumerate()
                .map(|(i, n)| (n.id.as_str(), i as f64))
                .collect();

            layers[layer_idx].sort_by(|a, b| {
                let a_bary = barycenter(a.id.as_str(), &down_adj, &next_positions);
                let b_bary = barycenter(b.id.as_str(), &down_adj, &next_positions);
                a_bary
                    .partial_cmp(&b_bary)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
    }
}

fn barycenter(
    node_id: &str,
    adj: &HashMap<&str, Vec<&str>>,
    positions: &HashMap<&str, f64>,
) -> f64 {
    let neighbors = match adj.get(node_id) {
        Some(n) if !n.is_empty() => n,
        _ => return f64::MAX, // No connections -> place at end
    };

    let mut sum: f64 = 0.0;
    let mut count: usize = 0;

    for &n in neighbors {
        if let Some(&pos) = positions.get(n) {
            sum += pos;
            count += 1;
        }
    }

    if count == 0 {
        f64::MAX
    } else {
        sum / count as f64
    }
}

// ─── Phase 4: Coordinate Assignment ───

fn assign_coordinates(layers: &[Vec<&PatchNode>]) -> LayoutResult {
    let mut positions = HashMap::new();
    let mut max_x: f64 = 0.0;
    let mut max_y: f64 = 0.0;

    for (layer_idx, layer) in layers.iter().enumerate() {
        let y = MARGIN_Y + (layer_idx as f64) * LAYER_SPACING;

        for (node_idx, node) in layer.iter().enumerate() {
            let x = MARGIN_X + (node_idx as f64) * NODE_SPACING;
            positions.insert(node.id.clone(), (x, y));
            if x > max_x {
                max_x = x;
            }
        }
        if y > max_y {
            max_y = y;
        }
    }

    LayoutResult {
        positions,
        patcher_size: (max_x + NODE_SPACING + MARGIN_X, max_y + LAYER_SPACING + MARGIN_Y),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flutmax_sema::graph::{NodePurity, PatchEdge, PatchGraph, PatchNode};

    fn make_node(id: &str, object_name: &str, is_signal: bool) -> PatchNode {
        PatchNode {
            id: id.into(),
            object_name: object_name.into(),
            args: vec![],
            num_inlets: if object_name.starts_with("inlet") {
                0
            } else {
                1
            },
            num_outlets: if object_name.starts_with("outlet") {
                0
            } else {
                1
            },
            is_signal,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        }
    }

    fn make_edge(src: &str, dst: &str) -> PatchEdge {
        PatchEdge {
            source_id: src.into(),
            source_outlet: 0,
            dest_id: dst.into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        }
    }

    #[test]
    fn test_empty_graph() {
        let graph = PatchGraph::new();
        let result = sugiyama_layout(&graph);
        assert!(result.positions.is_empty());
        assert_eq!(result.patcher_size, (200.0, 200.0));
    }

    #[test]
    fn test_single_node() {
        let mut graph = PatchGraph::new();
        graph.add_node(make_node("a", "cycle~", true));

        let result = sugiyama_layout(&graph);
        assert_eq!(result.positions.len(), 1);
        let (x, y) = result.positions["a"];
        assert_eq!(x, MARGIN_X);
        assert_eq!(y, MARGIN_Y);
    }

    #[test]
    fn test_linear_chain() {
        // A -> B -> C: 3 layers, 1 node each
        let mut graph = PatchGraph::new();
        graph.add_node(make_node("a", "cycle~", true));
        graph.add_node(make_node("b", "*~", true));
        graph.add_node(make_node("c", "ezdac~", true));
        graph.add_edge(make_edge("a", "b"));
        graph.add_edge(make_edge("b", "c"));

        let result = sugiyama_layout(&graph);
        assert_eq!(result.positions.len(), 3);

        let (ax, ay) = result.positions["a"];
        let (bx, by) = result.positions["b"];
        let (cx, cy) = result.positions["c"];

        // All should be in the same column (x = MARGIN_X)
        assert_eq!(ax, MARGIN_X);
        assert_eq!(bx, MARGIN_X);
        assert_eq!(cx, MARGIN_X);

        // Each layer is deeper
        assert!(ay < by);
        assert!(by < cy);

        // Spacing should be LAYER_SPACING
        assert!((by - ay - LAYER_SPACING).abs() < 0.01);
        assert!((cy - by - LAYER_SPACING).abs() < 0.01);
    }

    #[test]
    fn test_diamond_graph() {
        // A -> B, A -> C, B -> D, C -> D
        // Layer 0: A, Layer 1: B,C, Layer 2: D
        let mut graph = PatchGraph::new();
        graph.add_node(make_node("a", "button", false));
        graph.add_node(make_node("b", "+", false));
        graph.add_node(make_node("c", "*", false));
        graph.add_node(make_node("d", "print", false));
        graph.add_edge(make_edge("a", "b"));
        graph.add_edge(make_edge("a", "c"));
        graph.add_edge(make_edge("b", "d"));
        graph.add_edge(make_edge("c", "d"));

        let result = sugiyama_layout(&graph);
        assert_eq!(result.positions.len(), 4);

        let (_, ay) = result.positions["a"];
        let (_, by) = result.positions["b"];
        let (_, cy) = result.positions["c"];
        let (_, dy) = result.positions["d"];

        // A in layer 0, B and C in layer 1, D in layer 2
        assert!((ay - MARGIN_Y).abs() < 0.01);
        assert!((by - (MARGIN_Y + LAYER_SPACING)).abs() < 0.01);
        assert!((cy - (MARGIN_Y + LAYER_SPACING)).abs() < 0.01);
        assert!((dy - (MARGIN_Y + 2.0 * LAYER_SPACING)).abs() < 0.01);

        // B and C should be side by side (different x)
        let (bx, _) = result.positions["b"];
        let (cx, _) = result.positions["c"];
        assert!((bx - cx).abs() > 1.0, "B and C should have different x");
    }

    #[test]
    fn test_fanout() {
        // A -> B, A -> C, A -> D
        // Layer 0: A, Layer 1: B, C, D
        let mut graph = PatchGraph::new();
        graph.add_node(make_node("a", "button", false));
        graph.add_node(make_node("b", "print", false));
        graph.add_node(make_node("c", "int", false));
        graph.add_node(make_node("d", "float", false));
        graph.add_edge(make_edge("a", "b"));
        graph.add_edge(make_edge("a", "c"));
        graph.add_edge(make_edge("a", "d"));

        let result = sugiyama_layout(&graph);
        assert_eq!(result.positions.len(), 4);

        let (_, ay) = result.positions["a"];
        let (_, by) = result.positions["b"];
        let (_, cy) = result.positions["c"];
        let (_, dy) = result.positions["d"];

        // A in layer 0, B/C/D in layer 1
        assert!((ay - MARGIN_Y).abs() < 0.01);
        assert!((by - (MARGIN_Y + LAYER_SPACING)).abs() < 0.01);
        assert!((cy - (MARGIN_Y + LAYER_SPACING)).abs() < 0.01);
        assert!((dy - (MARGIN_Y + LAYER_SPACING)).abs() < 0.01);

        // Collect x positions of B, C, D - they should all be distinct
        let bx = result.positions["b"].0;
        let cx = result.positions["c"].0;
        let dx = result.positions["d"].0;
        let mut xs = vec![bx, cx, dx];
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert!(xs[1] - xs[0] > 1.0, "nodes in same layer should be spread");
        assert!(xs[2] - xs[1] > 1.0, "nodes in same layer should be spread");
    }

    #[test]
    fn test_inlet_outlet_placement() {
        // inlet -> cycle~ -> outlet~
        // inlets should be in layer 0 (top), outlets in the last layer (bottom)
        let mut graph = PatchGraph::new();
        graph.add_node(make_node("in0", "inlet", false));
        graph.add_node(make_node("osc", "cycle~", true));
        graph.add_node(make_node("out0", "outlet~", true));
        graph.add_edge(make_edge("in0", "osc"));
        graph.add_edge(make_edge("osc", "out0"));

        let result = sugiyama_layout(&graph);
        let (_, in_y) = result.positions["in0"];
        let (_, osc_y) = result.positions["osc"];
        let (_, out_y) = result.positions["out0"];

        // inlet at top, outlet at bottom
        assert!(in_y < osc_y, "inlet should be above cycle~");
        assert!(osc_y < out_y, "cycle~ should be above outlet~");
    }

    #[test]
    fn test_feedback_edges_excluded() {
        // A -> B (normal), B -> A (feedback)
        // Without feedback exclusion, this would be a cycle.
        // With feedback exclusion, A is layer 0, B is layer 1.
        let mut graph = PatchGraph::new();
        graph.add_node(make_node("a", "tapin~", true));
        graph.add_node(make_node("b", "tapout~", true));
        graph.add_edge(PatchEdge {
            source_id: "a".into(),
            source_outlet: 0,
            dest_id: "b".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });
        graph.add_edge(PatchEdge {
            source_id: "b".into(),
            source_outlet: 0,
            dest_id: "a".into(),
            dest_inlet: 0,
            is_feedback: true,
            order: None,
        });

        let result = sugiyama_layout(&graph);
        let (_, ay) = result.positions["a"];
        let (_, by) = result.positions["b"];
        assert!(ay < by, "tapin~ should be above tapout~ (feedback edge excluded)");
    }

    #[test]
    fn test_isolated_nodes() {
        // Two disconnected nodes
        let mut graph = PatchGraph::new();
        graph.add_node(make_node("a", "cycle~", true));
        graph.add_node(make_node("b", "noise~", true));

        let result = sugiyama_layout(&graph);
        assert_eq!(result.positions.len(), 2);

        // Both should be in layer 0 (no edges)
        let (_, ay) = result.positions["a"];
        let (_, by) = result.positions["b"];
        assert!((ay - MARGIN_Y).abs() < 0.01);
        assert!((by - MARGIN_Y).abs() < 0.01);

        // But at different x positions
        let ax = result.positions["a"].0;
        let bx = result.positions["b"].0;
        assert!((ax - bx).abs() > 1.0, "isolated nodes should be side by side");
    }

    #[test]
    fn test_crossing_reduction_improves_layout() {
        // Graph: A -> C, B -> D, A -> D, B -> C
        // Without crossing reduction, if layer 1 is [C, D],
        // edges A->D and B->C would cross.
        // Barycenter heuristic should swap to minimize crossings.
        let mut graph = PatchGraph::new();
        graph.add_node(make_node("a", "button", false));
        graph.add_node(make_node("b", "toggle", false));
        graph.add_node(make_node("c", "print", false));
        graph.add_node(make_node("d", "int", false));
        graph.add_edge(make_edge("a", "c"));
        graph.add_edge(make_edge("a", "d"));
        graph.add_edge(make_edge("b", "d"));

        let result = sugiyama_layout(&graph);

        // a and b should be in layer 0, c and d in layer 1
        let (_, ay) = result.positions["a"];
        let (_, by) = result.positions["b"];
        assert!((ay - by).abs() < 0.01, "a and b should be in same layer");

        let (_, cy) = result.positions["c"];
        let (_, dy) = result.positions["d"];
        assert!((cy - dy).abs() < 0.01, "c and d should be in same layer");
    }

    #[test]
    fn test_patcher_size_scales_with_graph() {
        let mut graph = PatchGraph::new();
        graph.add_node(make_node("a", "button", false));
        graph.add_node(make_node("b", "+", false));
        graph.add_node(make_node("c", "*", false));
        graph.add_node(make_node("d", "print", false));
        graph.add_edge(make_edge("a", "b"));
        graph.add_edge(make_edge("a", "c"));
        graph.add_edge(make_edge("b", "d"));
        graph.add_edge(make_edge("c", "d"));

        let result = sugiyama_layout(&graph);

        // Patcher should be at least big enough to contain all nodes
        let (pw, ph) = result.patcher_size;
        for (_, (x, y)) in &result.positions {
            assert!(*x < pw, "node x should be within patcher width");
            assert!(*y < ph, "node y should be within patcher height");
        }
    }
}
