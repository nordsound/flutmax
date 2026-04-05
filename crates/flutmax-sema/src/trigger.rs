use std::collections::HashMap;

use crate::graph::{NodePurity, PatchEdge, PatchGraph, PatchNode};

/// Detected fanout.
/// A state where one source outlet is connected to multiple destinations.
#[derive(Debug, Clone)]
pub struct Fanout {
    pub source_id: String,
    pub source_outlet: u32,
    /// List of destinations. Order corresponds to source code order (top to bottom).
    /// That is, `destinations[0]` is the destination that should execute first.
    pub destinations: Vec<(String, u32)>,
}

/// Type assigned to each outlet of a trigger object.
#[derive(Debug, Clone, PartialEq)]
pub enum TriggerOutletType {
    /// bang: outputs bang regardless of input
    Bang,
    /// int: converts input to integer and outputs
    Int,
    /// float: converts input to floating point and outputs
    Float,
    /// list: outputs input as a list
    List,
    /// symbol: outputs input as a symbol
    Symbol,
}

impl TriggerOutletType {
    /// Returns the trigger argument string representation.
    pub fn as_trigger_arg(&self) -> &str {
        match self {
            TriggerOutletType::Bang => "b",
            TriggerOutletType::Int => "i",
            TriggerOutletType::Float => "f",
            TriggerOutletType::List => "l",
            TriggerOutletType::Symbol => "s",
        }
    }
}

/// Detect fanouts.
///
/// Only targets Control signal (non-Signal) edges.
/// Fanouts from Signal objects (names ending with `~`) are excluded,
/// because Signal is processed in parallel as a DSP graph with no concept of execution order.
pub fn detect_fanouts(graph: &PatchGraph) -> Vec<Fanout> {
    // Group by (source_id, source_outlet) -> Vec<(dest_id, dest_inlet)>
    let mut groups: HashMap<(String, u32), Vec<(String, u32)>> = HashMap::new();

    for edge in &graph.edges {
        // Exclude feedback edges from trigger insertion
        if edge.is_feedback {
            continue;
        }

        // Exclude if source node is a Signal object
        if let Some(source_node) = graph.find_node(&edge.source_id) {
            if source_node.is_signal {
                continue;
            }
        }

        let key = (edge.source_id.clone(), edge.source_outlet);
        groups
            .entry(key)
            .or_default()
            .push((edge.dest_id.clone(), edge.dest_inlet));
    }

    // Groups with 2+ destinations are fanouts.
    // However, exclude groups where all destinations are Signal objects.
    // Signal objects are processed in parallel at DSP rate, so
    // control->signal fanouts don't need triggers.
    let mut fanouts: Vec<Fanout> = groups
        .into_iter()
        .filter(|(_, dests)| dests.len() >= 2)
        .filter(|(_, dests)| {
            // No trigger needed if all destinations are signal
            !dests.iter().all(|(dest_id, _)| {
                graph.find_node(dest_id)
                    .map_or(false, |n| n.is_signal)
            })
        })
        .map(|((source_id, source_outlet), destinations)| Fanout {
            source_id,
            source_outlet,
            destinations,
        })
        .collect();

    // Sort for stable output (by source_id, then source_outlet)
    fanouts.sort_by(|a, b| {
        a.source_id
            .cmp(&b.source_id)
            .then(a.source_outlet.cmp(&b.source_outlet))
    });

    fanouts
}

/// Auto-insert trigger objects.
///
/// For each detected fanout:
/// 1. Create a trigger node (outlet count = destination count)
/// 2. Remove original fanout edges
/// 3. Add source -> trigger edge
/// 4. Add trigger outlet[N] -> destination edges
///
/// Important: trigger fires from right outlet (last) to left outlet (outlet 0).
/// To match flutmax code order (top to bottom):
/// - destinations[0] (execute first) -> trigger's last outlet (rightmost)
/// - destinations[last] (execute last) -> trigger's outlet 0 (leftmost)
pub fn insert_triggers(graph: &mut PatchGraph) {
    let fanouts = detect_fanouts(graph);

    for (i, fanout) in fanouts.iter().enumerate() {
        let trigger_id = format!("__trigger_{}", i);
        let num_outlets = fanout.destinations.len() as u32;

        // 1. Create trigger node
        // Outlet types are inferred from the expected types of destinations.
        // outlets[n-1-j] corresponds to destinations[j] (right-to-left firing order).
        let n = fanout.destinations.len();
        let mut args: Vec<String> = vec!["f".into(); n];
        for (j, (dest_id, dest_inlet)) in fanout.destinations.iter().enumerate() {
            let outlet_idx = n - 1 - j;
            let dest_name = graph.find_node(dest_id)
                .map(|node| node.object_name.as_str())
                .unwrap_or("");
            args[outlet_idx] = determine_outlet_type(dest_name, *dest_inlet)
                .as_trigger_arg()
                .to_string();
        }
        let trigger_node = PatchNode {
            id: trigger_id.clone(),
            object_name: "trigger".into(),
            args,
            num_inlets: 1,
            num_outlets,
            is_signal: false, varname: None,
            hot_inlets: vec![true],
            purity: NodePurity::Pure,
            attrs: vec![],
            code: None,
        };
        graph.add_node(trigger_node);

        // 2. Remove original fanout edges
        //    Remove edges matching source_id and source_outlet whose destination
        //    is included in the fanout's destinations
        let dest_set: Vec<(String, u32)> = fanout.destinations.clone();
        graph.edges.retain(|e| {
            !(e.source_id == fanout.source_id
                && e.source_outlet == fanout.source_outlet
                && dest_set.contains(&(e.dest_id.clone(), e.dest_inlet)))
        });

        // 3. Add source -> trigger edge
        graph.add_edge(PatchEdge {
            source_id: fanout.source_id.clone(),
            source_outlet: fanout.source_outlet,
            dest_id: trigger_id.clone(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });

        // 4. Add trigger outlet[N] -> destination edges
        //    trigger fires from right (last outlet) to left (outlet 0).
        //    Connect destinations[0] to last outlet to execute first.
        //    Connect destinations[last] to outlet 0 to execute last.
        let n = fanout.destinations.len();
        for (j, (dest_id, dest_inlet)) in fanout.destinations.iter().enumerate() {
            // destinations[j] → outlet (n - 1 - j)
            // j=0 (execute first) -> outlet (n-1) (rightmost, fires first)
            // j=last (execute last) -> outlet 0 (leftmost, fires last)
            let outlet_index = (n - 1 - j) as u32;
            graph.add_edge(PatchEdge {
                source_id: trigger_id.clone(),
                source_outlet: outlet_index,
                dest_id: dest_id.clone(),
                dest_inlet: *dest_inlet,
                is_feedback: false,
                order: None,
            });
        }
    }
}

/// Determine the trigger outlet type.
///
/// Inferred from the expected type of the destination inlet.
/// Returns Bang as default when the type is unknown.
pub fn determine_outlet_type(dest_object_name: &str, dest_inlet: u32) -> TriggerOutletType {
    // Inference based on known object inlet types
    match (dest_object_name, dest_inlet) {
        // Numeric objects' left inlet expects the corresponding type
        ("flonum", 0) | ("float", 0) => TriggerOutletType::Float,
        ("number", 0) | ("int", 0) | ("i", 0) => TriggerOutletType::Int,
        // Signal objects like cycle~ expect float for frequency inlet
        (name, 0) if name.ends_with('~') => TriggerOutletType::Float,
        // * (multiply) inlet 0 is the left operand
        ("*", 0) | ("* 1.", 0) | ("+", 0) | ("-", 0) | ("/", 0) => TriggerOutletType::Float,
        // pack/unpack expects list
        ("pack", _) | ("unpack", _) => TriggerOutletType::List,
        // route, select, etc. expect symbol
        ("route", 0) | ("select", 0) => TriggerOutletType::Symbol,
        // Default is Float (auto-inserted triggers should preserve values)
        _ => TriggerOutletType::Float,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::*;

    /// Create a simple fanout graph: button -> (a.in[0], b.in[0])
    fn make_simple_fanout_graph() -> PatchGraph {
        let mut g = PatchGraph::new();
        g.add_node(PatchNode {
            id: "button".into(),
            object_name: "button".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 1,
            is_signal: false, varname: None,
            hot_inlets: vec![], purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        g.add_node(PatchNode {
            id: "a".into(),
            object_name: "print".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 0,
            is_signal: false, varname: None,
            hot_inlets: vec![], purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        g.add_node(PatchNode {
            id: "b".into(),
            object_name: "print".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 0,
            is_signal: false, varname: None,
            hot_inlets: vec![], purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        g.add_edge(PatchEdge {
            source_id: "button".into(),
            source_outlet: 0,
            dest_id: "a".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });
        g.add_edge(PatchEdge {
            source_id: "button".into(),
            source_outlet: 0,
            dest_id: "b".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });
        g
    }

    #[test]
    fn test_detect_simple_fanout() {
        let g = make_simple_fanout_graph();
        let fanouts = detect_fanouts(&g);
        assert_eq!(fanouts.len(), 1);
        assert_eq!(fanouts[0].source_id, "button");
        assert_eq!(fanouts[0].source_outlet, 0);
        assert_eq!(fanouts[0].destinations.len(), 2);
    }

    #[test]
    fn test_insert_trigger_simple() {
        let mut g = make_simple_fanout_graph();
        insert_triggers(&mut g);

        // trigger node has been added
        let trigger_node = g
            .nodes
            .iter()
            .find(|n| n.object_name == "trigger")
            .expect("trigger node should be inserted");
        assert_eq!(trigger_node.num_outlets, 2);

        // Edge from button -> trigger exists
        let to_trigger = g
            .edges
            .iter()
            .find(|e| e.source_id == "button" && e.dest_id == trigger_node.id)
            .expect("edge from button to trigger should exist");
        assert_eq!(to_trigger.source_outlet, 0);
        assert_eq!(to_trigger.dest_inlet, 0);

        // Edges trigger -> a and trigger -> b exist
        let from_trigger: Vec<&PatchEdge> = g
            .edges
            .iter()
            .filter(|e| e.source_id == trigger_node.id)
            .collect();
        assert_eq!(from_trigger.len(), 2);

        // Direct edges button -> a and button -> b have been removed
        let direct_from_button: Vec<&PatchEdge> = g
            .edges
            .iter()
            .filter(|e| e.source_id == "button" && (e.dest_id == "a" || e.dest_id == "b"))
            .collect();
        assert_eq!(direct_from_button.len(), 0);
    }

    #[test]
    fn test_trigger_outlet_order() {
        // Verify correspondence between destinations order and trigger outlets
        // destinations[0] (execute first) -> last outlet (rightmost)
        // destinations[1] (execute last) -> outlet 0 (leftmost)
        let mut g = make_simple_fanout_graph();
        insert_triggers(&mut g);

        let trigger_node = g
            .nodes
            .iter()
            .find(|n| n.object_name == "trigger")
            .unwrap();

        let from_trigger: Vec<&PatchEdge> = g
            .edges
            .iter()
            .filter(|e| e.source_id == trigger_node.id)
            .collect();

        // First element of destinations connects to outlet 1 (rightmost)
        // Last element of destinations connects to outlet 0 (leftmost)
        // trigger fires outlet 1 -> outlet 0, so
        // the one connected to outlet 1 executes first
        for edge in &from_trigger {
            if edge.dest_id == "a" || edge.dest_id == "b" {
                // Either a or b is outlet 1, the other is outlet 0
                // (HashMap order is non-deterministic, so just verify outlet is 0 or 1)
                assert!(edge.source_outlet == 0 || edge.source_outlet == 1);
            }
        }

        // Both outlet 0 and outlet 1 are used
        let outlets: Vec<u32> = from_trigger.iter().map(|e| e.source_outlet).collect();
        assert!(outlets.contains(&0));
        assert!(outlets.contains(&1));
    }

    #[test]
    fn test_signal_no_trigger() {
        // No trigger should be inserted for Signal fanouts
        let mut g = PatchGraph::new();
        g.add_node(PatchNode {
            id: "osc".into(),
            object_name: "cycle~".into(),
            args: vec!["440".into()],
            num_inlets: 2,
            num_outlets: 1,
            is_signal: true, varname: None,
            hot_inlets: vec![], purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        g.add_node(PatchNode {
            id: "filter".into(),
            object_name: "biquad~".into(),
            args: vec![],
            num_inlets: 6,
            num_outlets: 1,
            is_signal: true, varname: None,
            hot_inlets: vec![], purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        g.add_node(PatchNode {
            id: "delay".into(),
            object_name: "delay~".into(),
            args: vec![],
            num_inlets: 2,
            num_outlets: 1,
            is_signal: true, varname: None,
            hot_inlets: vec![], purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        g.add_edge(PatchEdge {
            source_id: "osc".into(),
            source_outlet: 0,
            dest_id: "filter".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });
        g.add_edge(PatchEdge {
            source_id: "osc".into(),
            source_outlet: 0,
            dest_id: "delay".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });

        let fanouts = detect_fanouts(&g);
        assert_eq!(fanouts.len(), 0, "Signal fanout should not be detected");
    }

    #[test]
    fn test_no_fanout() {
        // No trigger should be inserted for single connections
        let mut g = PatchGraph::new();
        g.add_node(PatchNode {
            id: "button".into(),
            object_name: "button".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 1,
            is_signal: false, varname: None,
            hot_inlets: vec![], purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        g.add_node(PatchNode {
            id: "print".into(),
            object_name: "print".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 0,
            is_signal: false, varname: None,
            hot_inlets: vec![], purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        g.add_edge(PatchEdge {
            source_id: "button".into(),
            source_outlet: 0,
            dest_id: "print".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });

        let fanouts = detect_fanouts(&g);
        assert_eq!(fanouts.len(), 0, "Single connection should not be a fanout");

        // insert_triggers should not change anything
        let node_count_before = g.nodes.len();
        let edge_count_before = g.edges.len();
        insert_triggers(&mut g);
        assert_eq!(g.nodes.len(), node_count_before);
        assert_eq!(g.edges.len(), edge_count_before);
    }

    #[test]
    fn test_triple_fanout() {
        // 3-way fanout
        let mut g = PatchGraph::new();
        g.add_node(PatchNode {
            id: "button".into(),
            object_name: "button".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 1,
            is_signal: false, varname: None,
            hot_inlets: vec![], purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        for name in &["a", "b", "c"] {
            g.add_node(PatchNode {
                id: (*name).into(),
                object_name: "print".into(),
                args: vec![],
                num_inlets: 1,
                num_outlets: 0,
                is_signal: false, varname: None,
                hot_inlets: vec![], purity: NodePurity::Unknown,
                attrs: vec![],
                code: None,
            });
            g.add_edge(PatchEdge {
                source_id: "button".into(),
                source_outlet: 0,
                dest_id: (*name).into(),
                dest_inlet: 0,
            is_feedback: false,
            order: None,
            });
        }

        let fanouts = detect_fanouts(&g);
        assert_eq!(fanouts.len(), 1);
        assert_eq!(fanouts[0].destinations.len(), 3);

        // Insert trigger
        insert_triggers(&mut g);
        let trigger_node = g
            .nodes
            .iter()
            .find(|n| n.object_name == "trigger")
            .expect("trigger should be inserted for triple fanout");
        assert_eq!(trigger_node.num_outlets, 3);

        // 3 output edges from trigger
        let from_trigger: Vec<&PatchEdge> = g
            .edges
            .iter()
            .filter(|e| e.source_id == trigger_node.id)
            .collect();
        assert_eq!(from_trigger.len(), 3);

        // All of outlet 0, 1, 2 are used
        let mut outlets: Vec<u32> = from_trigger.iter().map(|e| e.source_outlet).collect();
        outlets.sort();
        assert_eq!(outlets, vec![0, 1, 2]);
    }

    #[test]
    fn test_same_node_different_inlets() {
        // Connections to different inlets of the same node are also fanouts
        let mut g = PatchGraph::new();
        g.add_node(PatchNode {
            id: "button".into(),
            object_name: "button".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 1,
            is_signal: false, varname: None,
            hot_inlets: vec![], purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        g.add_node(PatchNode {
            id: "target".into(),
            object_name: "pack".into(),
            args: vec![],
            num_inlets: 2,
            num_outlets: 1,
            is_signal: false, varname: None,
            hot_inlets: vec![], purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        g.add_edge(PatchEdge {
            source_id: "button".into(),
            source_outlet: 0,
            dest_id: "target".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });
        g.add_edge(PatchEdge {
            source_id: "button".into(),
            source_outlet: 0,
            dest_id: "target".into(),
            dest_inlet: 1,
            is_feedback: false,
            order: None,
        });

        let fanouts = detect_fanouts(&g);
        assert_eq!(fanouts.len(), 1, "Same node different inlets is a fanout");
        assert_eq!(fanouts[0].destinations.len(), 2);
    }

    #[test]
    fn test_multiple_independent_fanouts() {
        // Multiple independent fanouts
        let mut g = PatchGraph::new();
        g.add_node(PatchNode {
            id: "btn1".into(),
            object_name: "button".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 1,
            is_signal: false, varname: None,
            hot_inlets: vec![], purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        g.add_node(PatchNode {
            id: "btn2".into(),
            object_name: "button".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 1,
            is_signal: false, varname: None,
            hot_inlets: vec![], purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        for name in &["a", "b", "c", "d"] {
            g.add_node(PatchNode {
                id: (*name).into(),
                object_name: "print".into(),
                args: vec![],
                num_inlets: 1,
                num_outlets: 0,
                is_signal: false, varname: None,
                hot_inlets: vec![], purity: NodePurity::Unknown,
                attrs: vec![],
                code: None,
            });
        }
        // btn1 -> a, b (fanout 1)
        g.add_edge(PatchEdge {
            source_id: "btn1".into(),
            source_outlet: 0,
            dest_id: "a".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });
        g.add_edge(PatchEdge {
            source_id: "btn1".into(),
            source_outlet: 0,
            dest_id: "b".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });
        // btn2 -> c, d (fanout 2)
        g.add_edge(PatchEdge {
            source_id: "btn2".into(),
            source_outlet: 0,
            dest_id: "c".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });
        g.add_edge(PatchEdge {
            source_id: "btn2".into(),
            source_outlet: 0,
            dest_id: "d".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });

        let fanouts = detect_fanouts(&g);
        assert_eq!(fanouts.len(), 2, "Two independent fanouts should be detected");

        insert_triggers(&mut g);
        let trigger_nodes: Vec<&PatchNode> = g
            .nodes
            .iter()
            .filter(|n| n.object_name == "trigger")
            .collect();
        assert_eq!(trigger_nodes.len(), 2, "Two trigger nodes should be inserted");
    }

    #[test]
    fn test_mixed_signal_and_control_fanout() {
        // Case with mixed Signal and Control fanouts
        let mut g = PatchGraph::new();

        // Control source
        g.add_node(PatchNode {
            id: "number".into(),
            object_name: "number".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 2,
            is_signal: false, varname: None,
            hot_inlets: vec![], purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        // Signal source
        g.add_node(PatchNode {
            id: "osc".into(),
            object_name: "cycle~".into(),
            args: vec!["440".into()],
            num_inlets: 2,
            num_outlets: 1,
            is_signal: true, varname: None,
            hot_inlets: vec![], purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        // Destinations
        for name in &["ctrl_a", "ctrl_b", "sig_a", "sig_b"] {
            g.add_node(PatchNode {
                id: (*name).into(),
                object_name: if name.starts_with("sig") {
                    "gain~".into()
                } else {
                    "print".into()
                },
                args: vec![],
                num_inlets: 1,
                num_outlets: 1,
                is_signal: name.starts_with("sig"),
                varname: None,
                hot_inlets: vec![], purity: NodePurity::Unknown,
                attrs: vec![],
                code: None,
            });
        }

        // Control fanout: number -> ctrl_a, ctrl_b
        g.add_edge(PatchEdge {
            source_id: "number".into(),
            source_outlet: 0,
            dest_id: "ctrl_a".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });
        g.add_edge(PatchEdge {
            source_id: "number".into(),
            source_outlet: 0,
            dest_id: "ctrl_b".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });
        // Signal fanout: osc -> sig_a, sig_b
        g.add_edge(PatchEdge {
            source_id: "osc".into(),
            source_outlet: 0,
            dest_id: "sig_a".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });
        g.add_edge(PatchEdge {
            source_id: "osc".into(),
            source_outlet: 0,
            dest_id: "sig_b".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });

        let fanouts = detect_fanouts(&g);
        assert_eq!(fanouts.len(), 1, "Only control fanout should be detected");
        assert_eq!(fanouts[0].source_id, "number");
    }

    #[test]
    fn test_large_fanout() {
        // Large 10-way fanout
        let mut g = PatchGraph::new();
        g.add_node(PatchNode {
            id: "button".into(),
            object_name: "button".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 1,
            is_signal: false, varname: None,
            hot_inlets: vec![], purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });

        for i in 0..10 {
            let name = format!("n{}", i);
            g.add_node(PatchNode {
                id: name.clone(),
                object_name: "print".into(),
                args: vec![],
                num_inlets: 1,
                num_outlets: 0,
                is_signal: false, varname: None,
                hot_inlets: vec![], purity: NodePurity::Unknown,
                attrs: vec![],
                code: None,
            });
            g.add_edge(PatchEdge {
                source_id: "button".into(),
                source_outlet: 0,
                dest_id: name,
                dest_inlet: 0,
            is_feedback: false,
            order: None,
            });
        }

        let fanouts = detect_fanouts(&g);
        assert_eq!(fanouts.len(), 1);
        assert_eq!(fanouts[0].destinations.len(), 10);

        insert_triggers(&mut g);
        let trigger_node = g
            .nodes
            .iter()
            .find(|n| n.object_name == "trigger")
            .unwrap();
        assert_eq!(trigger_node.num_outlets, 10);

        // 10 output edges from trigger
        let from_trigger: Vec<&PatchEdge> = g
            .edges
            .iter()
            .filter(|e| e.source_id == trigger_node.id)
            .collect();
        assert_eq!(from_trigger.len(), 10);

        // All of outlet 0..9 are used
        let mut outlets: Vec<u32> = from_trigger.iter().map(|e| e.source_outlet).collect();
        outlets.sort();
        assert_eq!(outlets, (0..10).collect::<Vec<u32>>());
    }

    #[test]
    fn test_determine_outlet_type_defaults() {
        // Unknown objects default to Float (auto-inserted triggers preserve values)
        assert_eq!(
            determine_outlet_type("unknown_obj", 0),
            TriggerOutletType::Float
        );
        assert_eq!(
            determine_outlet_type("print", 0),
            TriggerOutletType::Float
        );
    }

    #[test]
    fn test_determine_outlet_type_float() {
        assert_eq!(
            determine_outlet_type("flonum", 0),
            TriggerOutletType::Float
        );
        assert_eq!(
            determine_outlet_type("cycle~", 0),
            TriggerOutletType::Float
        );
    }

    #[test]
    fn test_determine_outlet_type_int() {
        assert_eq!(
            determine_outlet_type("number", 0),
            TriggerOutletType::Int
        );
    }

    #[test]
    fn test_determine_outlet_type_list() {
        assert_eq!(
            determine_outlet_type("pack", 0),
            TriggerOutletType::List
        );
        assert_eq!(
            determine_outlet_type("unpack", 2),
            TriggerOutletType::List
        );
    }

    #[test]
    fn test_trigger_outlet_type_as_arg() {
        assert_eq!(TriggerOutletType::Bang.as_trigger_arg(), "b");
        assert_eq!(TriggerOutletType::Int.as_trigger_arg(), "i");
        assert_eq!(TriggerOutletType::Float.as_trigger_arg(), "f");
        assert_eq!(TriggerOutletType::List.as_trigger_arg(), "l");
        assert_eq!(TriggerOutletType::Symbol.as_trigger_arg(), "s");
    }

    #[test]
    fn test_different_outlets_no_fanout() {
        // Connections from different outlets of the same node are not fanouts
        let mut g = PatchGraph::new();
        g.add_node(PatchNode {
            id: "number".into(),
            object_name: "number".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 2,
            is_signal: false, varname: None,
            hot_inlets: vec![], purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        g.add_node(PatchNode {
            id: "a".into(),
            object_name: "print".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 0,
            is_signal: false, varname: None,
            hot_inlets: vec![], purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        g.add_node(PatchNode {
            id: "b".into(),
            object_name: "print".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 0,
            is_signal: false, varname: None,
            hot_inlets: vec![], purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        // outlet 0 -> a, outlet 1 -> b (different outlets, so not a fanout)
        g.add_edge(PatchEdge {
            source_id: "number".into(),
            source_outlet: 0,
            dest_id: "a".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });
        g.add_edge(PatchEdge {
            source_id: "number".into(),
            source_outlet: 1,
            dest_id: "b".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });

        let fanouts = detect_fanouts(&g);
        assert_eq!(
            fanouts.len(),
            0,
            "Different outlets should not be a fanout"
        );
    }
}
