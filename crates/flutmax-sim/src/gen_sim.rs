/// gen~ patcher simulator.
///
/// Parses gen~ .maxpat JSON (classnamespace="dsp.gen") and builds an execution
/// graph. Processes audio sample-by-sample using topological evaluation with
/// feedback-correct handling of `history` and `delay` nodes.
use crate::audio::AudioOutput;
use crate::ops::{execute_op, initial_state, num_outlets, parse_gen_op, GenOp, NodeState};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};

/// Error type for simulation failures.
#[derive(Debug)]
pub enum SimError {
    JsonParse(String),
    GraphBuild(String),
}

impl std::fmt::Display for SimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SimError::JsonParse(s) => write!(f, "JSON parse error: {s}"),
            SimError::GraphBuild(s) => write!(f, "Graph build error: {s}"),
        }
    }
}

impl std::error::Error for SimError {}

/// A node in the execution graph.
#[derive(Debug, Clone)]
struct SimNode {
    op: GenOp,
    /// Each inlet: (source_node_index, source_outlet_index).
    /// None means no connection (use default/arg).
    input_sources: Vec<Option<(usize, usize)>>,
    num_outlets: usize,
    arg: Option<f64>,
    /// Original box ID from the JSON (for wiring).
    #[allow(dead_code)]
    box_id: String,
}

/// Tracks which history/delay nodes need back-edge updates.
#[derive(Debug, Clone)]
struct BackEdge {
    /// Index of the history/delay node in the sorted node list.
    target_node: usize,
    /// Where the write input comes from: (source_node_index, source_outlet_index).
    source: Option<(usize, usize)>,
}

/// gen~ simulator.
pub struct GenSimulator {
    nodes: Vec<SimNode>,
    node_outputs: Vec<Vec<f64>>,
    state: Vec<NodeState>,
    inputs: Vec<f64>,
    outputs: Vec<f64>,
    back_edges: Vec<BackEdge>,
    sample_rate: f64,
    num_inputs: usize,
    num_outputs: usize,
}

impl GenSimulator {
    /// Build a GenSimulator from a gen~ patcher JSON string.
    pub fn from_json(json: &str) -> Result<Self, SimError> {
        let root: Value =
            serde_json::from_str(json).map_err(|e| SimError::JsonParse(e.to_string()))?;
        Self::from_value(&root)
    }

    /// Build a GenSimulator from a parsed JSON Value.
    pub fn from_value(root: &Value) -> Result<Self, SimError> {
        Self::from_value_with_sr(root, 44100.0)
    }

    /// Build a GenSimulator from a parsed JSON Value with a specific sample rate.
    pub fn from_value_with_sr(root: &Value, sample_rate: f64) -> Result<Self, SimError> {
        // Navigate to the patcher
        let patcher = root
            .get("patcher")
            .ok_or_else(|| SimError::JsonParse("No 'patcher' field".into()))?;

        let boxes = patcher
            .get("boxes")
            .and_then(|b| b.as_array())
            .ok_or_else(|| SimError::JsonParse("No 'boxes' array".into()))?;

        let lines = patcher
            .get("lines")
            .and_then(|l| l.as_array())
            .unwrap_or(&Vec::new())
            .clone();

        // Phase 1: Parse all boxes
        // Map from box ID → (index_in_raw_list, GenOp, arg)
        let mut box_map: HashMap<String, usize> = HashMap::new();
        let mut raw_nodes: Vec<(String, GenOp, Option<f64>)> = Vec::new();

        for box_val in boxes {
            let bx = box_val.get("box").unwrap_or(box_val);
            let id = bx
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let text = bx.get("text").and_then(|v| v.as_str()).unwrap_or("");

            if id.is_empty() || text.is_empty() {
                continue;
            }

            let (op, arg) = parse_gen_op(text);
            let idx = raw_nodes.len();
            box_map.insert(id.clone(), idx);
            raw_nodes.push((id, op, arg));
        }

        let n = raw_nodes.len();
        if n == 0 {
            return Ok(Self {
                nodes: Vec::new(),
                node_outputs: Vec::new(),
                state: Vec::new(),
                inputs: Vec::new(),
                outputs: Vec::new(),
                back_edges: Vec::new(),
                sample_rate,
                num_inputs: 0,
                num_outputs: 0,
            });
        }

        // Phase 2: Parse connections
        // connections[dest_node][dest_inlet] = (src_node, src_outlet)
        let mut connections: Vec<HashMap<usize, (usize, usize)>> = vec![HashMap::new(); n];
        // Also track: what feeds into history/delay inlet 0 (the write input)
        let mut history_write_sources: HashMap<usize, (usize, usize)> = HashMap::new();

        for line_val in &lines {
            let ln = line_val.get("patchline").unwrap_or(line_val);
            let src_arr = ln.get("source").and_then(|v| v.as_array());
            let dst_arr = ln.get("destination").and_then(|v| v.as_array());

            if let (Some(src), Some(dst)) = (src_arr, dst_arr) {
                let src_id = src.first().and_then(|v| v.as_str()).unwrap_or("");
                let src_outlet = src.get(1).and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let dst_id = dst.first().and_then(|v| v.as_str()).unwrap_or("");
                let dst_inlet = dst.get(1).and_then(|v| v.as_u64()).unwrap_or(0) as usize;

                if let (Some(&src_idx), Some(&dst_idx)) = (box_map.get(src_id), box_map.get(dst_id))
                {
                    let dst_op = &raw_nodes[dst_idx].1;
                    // History and Delay: inlet 0 is the write (back-edge)
                    if matches!(dst_op, GenOp::History | GenOp::Delay) && dst_inlet == 0 {
                        history_write_sources.insert(dst_idx, (src_idx, src_outlet));
                    } else {
                        connections[dst_idx].insert(dst_inlet, (src_idx, src_outlet));
                    }
                }
            }
        }

        // Phase 3: Topological sort
        // Build forward adjacency (excluding back-edges to history/delay writes)
        let mut forward_deps: Vec<HashSet<usize>> = vec![HashSet::new(); n];
        for (dst_idx, conn) in connections.iter().enumerate() {
            for &(src_idx, _) in conn.values() {
                forward_deps[dst_idx].insert(src_idx);
            }
        }

        // Kahn's algorithm
        let mut in_degree: Vec<usize> = vec![0; n];
        for deps in &forward_deps {
            for _ in deps {
                // Each dependency increments in_degree... wait, we need outgoing edges
            }
        }
        // Recompute: in_degree[i] = number of forward dependencies of node i
        for (i, deps) in forward_deps.iter().enumerate() {
            in_degree[i] = deps.len();
        }

        // Build reverse adjacency: who depends on me?
        let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (dst, deps) in forward_deps.iter().enumerate() {
            for &src in deps {
                dependents[src].push(dst);
            }
        }

        let mut queue: VecDeque<usize> = VecDeque::new();
        for (i, &deg) in in_degree.iter().enumerate().take(n) {
            if deg == 0 {
                queue.push_back(i);
            }
        }

        let mut sorted: Vec<usize> = Vec::with_capacity(n);
        while let Some(node) = queue.pop_front() {
            sorted.push(node);
            for &dep in &dependents[node] {
                in_degree[dep] -= 1;
                if in_degree[dep] == 0 {
                    queue.push_back(dep);
                }
            }
        }

        // If not all nodes sorted, there may be cycles not broken by history/delay
        // Add remaining nodes (shouldn't happen in well-formed gen~ patches)
        if sorted.len() < n {
            for i in 0..n {
                if !sorted.contains(&i) {
                    sorted.push(i);
                }
            }
        }

        // Phase 4: Build the final node list with remapped indices
        let mut old_to_new: Vec<usize> = vec![0; n];
        for (new_idx, &old_idx) in sorted.iter().enumerate() {
            old_to_new[old_idx] = new_idx;
        }

        let mut num_inputs = 0usize;
        let mut num_outputs = 0usize;

        let mut sim_nodes: Vec<SimNode> = Vec::with_capacity(n);
        let mut states: Vec<NodeState> = Vec::with_capacity(n);

        for &old_idx in &sorted {
            let (ref id, ref op, arg) = raw_nodes[old_idx];

            // Track I/O counts
            match op {
                GenOp::In(idx) => num_inputs = num_inputs.max(*idx + 1),
                GenOp::Out(idx) => num_outputs = num_outputs.max(*idx + 1),
                _ => {}
            }

            // Map input sources to new indices
            let conn = &connections[old_idx];
            // Determine max inlet index for this node
            let max_inlet = conn.keys().copied().max().unwrap_or(0);
            let num_inlets = if conn.is_empty() { 0 } else { max_inlet + 1 };

            let mut input_sources: Vec<Option<(usize, usize)>> = vec![None; num_inlets];
            for (&inlet, &(src_old, outlet)) in conn {
                if inlet < input_sources.len() {
                    input_sources[inlet] = Some((old_to_new[src_old], outlet));
                }
            }

            let outlets = num_outlets(op);
            states.push(initial_state(op, arg));

            sim_nodes.push(SimNode {
                op: op.clone(),
                input_sources,
                num_outlets: outlets,
                arg,
                box_id: id.clone(),
            });
        }

        // Build back-edges for history/delay write updates
        let mut back_edges: Vec<BackEdge> = Vec::new();
        for (&old_dst, &(old_src, outlet)) in &history_write_sources {
            back_edges.push(BackEdge {
                target_node: old_to_new[old_dst],
                source: Some((old_to_new[old_src], outlet)),
            });
        }

        // Initialize output buffers
        let node_outputs: Vec<Vec<f64>> =
            sim_nodes.iter().map(|n| vec![0.0; n.num_outlets]).collect();

        Ok(Self {
            nodes: sim_nodes,
            node_outputs,
            state: states,
            inputs: vec![0.0; num_inputs],
            outputs: vec![0.0; num_outputs],
            back_edges,
            sample_rate,
            num_inputs,
            num_outputs,
        })
    }

    /// Set the value for a gen~ input port (0-based index).
    pub fn set_input(&mut self, index: usize, value: f64) {
        if index < self.inputs.len() {
            self.inputs[index] = value;
        }
    }

    /// Get the current output values.
    pub fn get_outputs(&self) -> &[f64] {
        &self.outputs
    }

    /// Get the number of inputs.
    pub fn num_inputs(&self) -> usize {
        self.num_inputs
    }

    /// Get the number of outputs.
    pub fn num_outputs(&self) -> usize {
        self.num_outputs
    }

    /// Get the sample rate.
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Process a single sample through the graph.
    pub fn process_sample(&mut self) {
        for node_idx in 0..self.nodes.len() {
            // Gather inputs
            let node = &self.nodes[node_idx];
            let input_vals: Vec<f64> = node
                .input_sources
                .iter()
                .map(|src| {
                    src.map(|(ni, oi)| {
                        self.node_outputs
                            .get(ni)
                            .and_then(|outs| outs.get(oi).copied())
                            .unwrap_or(0.0)
                    })
                    .unwrap_or(0.0)
                })
                .collect();

            // For In nodes, override with the simulator input
            let effective_inputs = match &node.op {
                GenOp::In(idx) => vec![self.inputs.get(*idx).copied().unwrap_or(0.0)],
                _ => input_vals,
            };

            let result = execute_op(
                &node.op,
                &effective_inputs,
                node.arg,
                &mut self.state[node_idx],
                self.sample_rate,
            );

            // Store results
            for (i, &val) in result.iter().enumerate() {
                if i < self.node_outputs[node_idx].len() {
                    self.node_outputs[node_idx][i] = val;
                }
            }

            // For Out nodes, copy to simulator output
            if let GenOp::Out(idx) = &node.op {
                if *idx < self.outputs.len() {
                    self.outputs[*idx] = result.first().copied().unwrap_or(0.0);
                }
            }
        }

        // Back-edge pass: update history/delay states
        for edge in &self.back_edges {
            let write_val = edge
                .source
                .map(|(ni, oi)| {
                    self.node_outputs
                        .get(ni)
                        .and_then(|outs| outs.get(oi).copied())
                        .unwrap_or(0.0)
                })
                .unwrap_or(0.0);

            match &mut self.state[edge.target_node] {
                NodeState::History(val) => *val = write_val,
                NodeState::Delay(buf) => buf.write(write_val),
                _ => {}
            }
        }
    }

    /// Run for N samples, returning an AudioOutput with all output channels.
    pub fn run_samples(&mut self, n: usize) -> AudioOutput {
        let num_ch = self.num_outputs.max(1);
        let mut output = AudioOutput::new(num_ch, self.sample_rate);

        for _ in 0..n {
            self.process_sample();
            for ch in 0..self.num_outputs {
                output.channels[ch].push(self.outputs.get(ch).copied().unwrap_or(0.0));
            }
        }

        output
    }

    /// Run for a given duration in seconds.
    pub fn run_seconds(&mut self, seconds: f64) -> AudioOutput {
        let n = (seconds * self.sample_rate) as usize;
        self.run_samples(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a minimal gen~ patcher JSON.
    fn make_gen_json(boxes: &[(&str, &str)], lines: &[(&str, u64, &str, u64)]) -> String {
        let boxes_json: Vec<String> = boxes
            .iter()
            .map(|(id, text)| format!(r#"{{"box": {{"id": "{id}", "text": "{text}"}}}}"#,))
            .collect();

        let lines_json: Vec<String> = lines
            .iter()
            .map(|(src, src_out, dst, dst_in)| {
                format!(
                    r#"{{"patchline": {{"source": ["{src}", {src_out}], "destination": ["{dst}", {dst_in}]}}}}"#,
                )
            })
            .collect();

        format!(
            r#"{{"patcher": {{"boxes": [{}], "lines": [{}]}}}}"#,
            boxes_json.join(", "),
            lines_json.join(", ")
        )
    }

    #[test]
    fn test_passthrough() {
        // in 1 → out 1
        let json = make_gen_json(&[("a", "in 1"), ("b", "out 1")], &[("a", 0, "b", 0)]);
        let mut sim = GenSimulator::from_json(&json).unwrap();
        sim.set_input(0, 0.75);
        sim.process_sample();
        assert_eq!(sim.outputs[0], 0.75);
    }

    #[test]
    fn test_multiply() {
        // in 1 → * 0.5 → out 1
        let json = make_gen_json(
            &[("a", "in 1"), ("b", "* 0.5"), ("c", "out 1")],
            &[("a", 0, "b", 0), ("b", 0, "c", 0)],
        );
        let mut sim = GenSimulator::from_json(&json).unwrap();
        sim.set_input(0, 1.0);
        sim.process_sample();
        assert!((sim.outputs[0] - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_add_two_inputs() {
        // in 1 → + ← in 2 → out 1
        let json = make_gen_json(
            &[("a", "in 1"), ("b", "in 2"), ("c", "+"), ("d", "out 1")],
            &[("a", 0, "c", 0), ("b", 0, "c", 1), ("c", 0, "d", 0)],
        );
        let mut sim = GenSimulator::from_json(&json).unwrap();
        sim.set_input(0, 3.0);
        sim.set_input(1, 4.0);
        sim.process_sample();
        assert!((sim.outputs[0] - 7.0).abs() < 1e-10);
    }

    #[test]
    fn test_chain() {
        // in 1 → * 2 → + 10 → out 1
        let json = make_gen_json(
            &[("a", "in 1"), ("b", "* 2"), ("c", "+ 10"), ("d", "out 1")],
            &[("a", 0, "b", 0), ("b", 0, "c", 0), ("c", 0, "d", 0)],
        );
        let mut sim = GenSimulator::from_json(&json).unwrap();
        sim.set_input(0, 5.0);
        sim.process_sample();
        // 5 * 2 + 10 = 20
        assert!((sim.outputs[0] - 20.0).abs() < 1e-10);
    }

    #[test]
    fn test_history_feedback() {
        // Simple accumulator: history fb 0 → + ← in 1, output of + feeds back to history
        // history fb → + → out 1
        //              ↑      ↓ (back-edge to history)
        //             in 1
        let json = make_gen_json(
            &[
                ("inp", "in 1"),
                ("hist", "history fb 0"),
                ("add", "+"),
                ("outp", "out 1"),
            ],
            &[
                ("hist", 0, "add", 0), // history output → add inlet 0
                ("inp", 0, "add", 1),  // input → add inlet 1
                ("add", 0, "outp", 0), // add → out
                ("add", 0, "hist", 0), // add → history write (back-edge)
            ],
        );
        let mut sim = GenSimulator::from_json(&json).unwrap();
        sim.set_input(0, 1.0);

        // Sample 1: history=0, out=0+1=1, then history updated to 1
        sim.process_sample();
        assert!((sim.outputs[0] - 1.0).abs() < 1e-10);

        // Sample 2: history=1, out=1+1=2
        sim.process_sample();
        assert!((sim.outputs[0] - 2.0).abs() < 1e-10);

        // Sample 3: history=2, out=2+1=3
        sim.process_sample();
        assert!((sim.outputs[0] - 3.0).abs() < 1e-10);
    }

    #[test]
    fn test_cycle_generates_audio() {
        // cycle~ with fixed frequency → * 0.2 → out 1
        // We'll set in 1 to 440 Hz
        let json = make_gen_json(
            &[
                ("freq", "in 1"),
                ("osc", "cycle"),
                ("gain", "* 0.2"),
                ("outp", "out 1"),
            ],
            &[
                ("freq", 0, "osc", 0),
                ("osc", 0, "gain", 0),
                ("gain", 0, "outp", 0),
            ],
        );
        let mut sim = GenSimulator::from_json(&json).unwrap();
        sim.set_input(0, 440.0);

        let output = sim.run_samples(4410); // 0.1 seconds at 44100

        // Should not be silent
        assert!(!output.is_silent());
        // Peak should be around 0.2
        assert!(output.peak() < 0.25);
        assert!(output.peak() > 0.15);
    }

    #[test]
    fn test_run_seconds() {
        let json = make_gen_json(&[("a", "in 1"), ("b", "out 1")], &[("a", 0, "b", 0)]);
        let mut sim = GenSimulator::from_json(&json).unwrap();
        sim.set_input(0, 1.0);
        let output = sim.run_seconds(0.01); // 10ms
        assert_eq!(output.channels[0].len(), 441); // 44100 * 0.01
    }

    #[test]
    fn test_multiple_outputs() {
        // in 1 → out 1, in 1 → * 2 → out 2
        let json = make_gen_json(
            &[
                ("inp", "in 1"),
                ("mul", "* 2"),
                ("out1", "out 1"),
                ("out2", "out 2"),
            ],
            &[
                ("inp", 0, "out1", 0),
                ("inp", 0, "mul", 0),
                ("mul", 0, "out2", 0),
            ],
        );
        let mut sim = GenSimulator::from_json(&json).unwrap();
        sim.set_input(0, 3.0);
        sim.process_sample();
        assert!((sim.outputs[0] - 3.0).abs() < 1e-10);
        assert!((sim.outputs[1] - 6.0).abs() < 1e-10);
    }

    #[test]
    fn test_empty_patcher() {
        let json = r#"{"patcher": {"boxes": [], "lines": []}}"#;
        let sim = GenSimulator::from_json(json).unwrap();
        assert_eq!(sim.nodes.len(), 0);
    }

    #[test]
    fn test_neg_chain() {
        // in 1 → neg → out 1
        let json = make_gen_json(
            &[("a", "in 1"), ("b", "neg"), ("c", "out 1")],
            &[("a", 0, "b", 0), ("b", 0, "c", 0)],
        );
        let mut sim = GenSimulator::from_json(&json).unwrap();
        sim.set_input(0, 5.0);
        sim.process_sample();
        assert!((sim.outputs[0] - (-5.0)).abs() < 1e-10);
    }

    #[test]
    fn test_exp_chain() {
        // in 1 → exp → out 1
        let json = make_gen_json(
            &[("a", "in 1"), ("b", "exp"), ("c", "out 1")],
            &[("a", 0, "b", 0), ("b", 0, "c", 0)],
        );
        let mut sim = GenSimulator::from_json(&json).unwrap();
        sim.set_input(0, 1.0);
        sim.process_sample();
        assert!((sim.outputs[0] - std::f64::consts::E).abs() < 1e-10);
    }

    #[test]
    fn test_samplerate_node() {
        // samplerate → out 1
        let json = make_gen_json(
            &[("sr", "samplerate"), ("outp", "out 1")],
            &[("sr", 0, "outp", 0)],
        );
        let mut sim = GenSimulator::from_json(&json).unwrap();
        sim.process_sample();
        assert!((sim.outputs[0] - 44100.0).abs() < 1e-10);
    }
}
