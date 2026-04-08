/// RNBO patcher simulator.
///
/// Parses RNBO .maxpat JSON and handles param objects, MIDI (notein),
/// embedded gen~ patchers, basic math operators, and signal routing.
use crate::audio::AudioOutput;
use crate::gen_sim::{GenSimulator, SimError};
use crate::midi::MidiState;
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};

/// An RNBO-level node operation.
#[derive(Debug, Clone)]
enum RnboOp {
    /// Named parameter with current value.
    Param(String),
    /// MIDI note input: outlets are note, velocity, channel.
    NoteIn,
    /// Embedded gen~ patcher (identified by title/key).
    Gen(String),
    /// Signal input: in~ N (0-based).
    SignalIn(usize),
    /// Signal output: out~ N (0-based).
    SignalOut(usize),
    /// Basic math operations at RNBO level.
    Add,
    Sub,
    Mul,
    Div,
    /// MIDI note to frequency.
    Mtof,
    /// Pass-through for unsupported objects.
    Pass,
}

/// A node in the RNBO execution graph.
#[derive(Debug, Clone)]
struct RnboNode {
    op: RnboOp,
    /// Each inlet: Option<(source_node_index, source_outlet_index)>.
    input_sources: Vec<Option<(usize, usize)>>,
    num_outlets: usize,
    arg: Option<f64>,
    #[allow(dead_code)]
    box_id: String,
}

/// Parameter state.
#[derive(Debug, Clone)]
struct ParamState {
    value: f64,
    #[allow(dead_code)]
    min: f64,
    #[allow(dead_code)]
    max: f64,
}

/// RNBO simulator.
pub struct RnboSimulator {
    nodes: Vec<RnboNode>,
    node_outputs: Vec<Vec<f64>>,
    gen_sims: HashMap<String, GenSimulator>,
    /// Map from param name → param node index.
    #[allow(dead_code)]
    param_indices: HashMap<String, usize>,
    params: HashMap<String, ParamState>,
    midi_state: MidiState,
    signal_inputs: Vec<f64>,
    signal_outputs: Vec<f64>,
    sample_rate: f64,
    #[allow(dead_code)]
    num_signal_inputs: usize,
    num_signal_outputs: usize,
}

impl RnboSimulator {
    /// Build an RnboSimulator from RNBO patcher JSON.
    pub fn from_json(json: &str) -> Result<Self, SimError> {
        let root: Value =
            serde_json::from_str(json).map_err(|e| SimError::JsonParse(e.to_string()))?;
        Self::from_value(&root, 44100.0)
    }

    /// Build from a JSON Value with a given sample rate.
    pub fn from_value(root: &Value, sample_rate: f64) -> Result<Self, SimError> {
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

        // Phase 1: Parse boxes
        let mut box_map: HashMap<String, usize> = HashMap::new();
        let mut raw_nodes: Vec<(String, RnboOp, Option<f64>)> = Vec::new();
        let mut gen_sims: HashMap<String, GenSimulator> = HashMap::new();
        let mut params: HashMap<String, ParamState> = HashMap::new();
        let mut param_indices: HashMap<String, usize> = HashMap::new();

        for box_val in boxes {
            let bx = box_val.get("box").unwrap_or(box_val);
            let id = bx
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let maxclass = bx.get("maxclass").and_then(|v| v.as_str()).unwrap_or("");

            let text = bx.get("text").and_then(|v| v.as_str()).unwrap_or("");

            if id.is_empty() {
                continue;
            }

            let (op, arg) = parse_rnbo_op(maxclass, text, bx);

            // Handle embedded gen~
            if let RnboOp::Gen(ref title) = op {
                if let Some(sub_patcher) = bx.get("patcher") {
                    let gen_root = serde_json::json!({"patcher": sub_patcher});
                    if let Ok(gen_sim) = GenSimulator::from_value_with_sr(&gen_root, sample_rate) {
                        gen_sims.insert(title.clone(), gen_sim);
                    }
                }
            }

            // Handle param
            if let RnboOp::Param(ref name) = op {
                let default = arg.unwrap_or(0.0);
                let min = parse_attr_f64(bx, "minimum", 0.0);
                let max = parse_attr_f64(bx, "maximum", 1.0);
                params.insert(
                    name.clone(),
                    ParamState {
                        value: default,
                        min,
                        max,
                    },
                );
                let idx = raw_nodes.len();
                param_indices.insert(name.clone(), idx);
            }

            let idx = raw_nodes.len();
            box_map.insert(id.clone(), idx);
            raw_nodes.push((id, op, arg));
        }

        let n = raw_nodes.len();

        // Phase 2: Parse connections
        let mut connections: Vec<HashMap<usize, (usize, usize)>> = vec![HashMap::new(); n];

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
                    connections[dst_idx].insert(dst_inlet, (src_idx, src_outlet));
                }
            }
        }

        // Phase 3: Topological sort
        let mut forward_deps: Vec<HashSet<usize>> = vec![HashSet::new(); n];
        for (dst_idx, conn) in connections.iter().enumerate() {
            for &(src_idx, _) in conn.values() {
                forward_deps[dst_idx].insert(src_idx);
            }
        }

        let mut in_degree: Vec<usize> = vec![0; n];
        for (i, deps) in forward_deps.iter().enumerate() {
            in_degree[i] = deps.len();
        }

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

        // Add any remaining (shouldn't happen)
        if sorted.len() < n {
            for i in 0..n {
                if !sorted.contains(&i) {
                    sorted.push(i);
                }
            }
        }

        // Phase 4: Build final node list
        let mut old_to_new: Vec<usize> = vec![0; n];
        for (new_idx, &old_idx) in sorted.iter().enumerate() {
            old_to_new[old_idx] = new_idx;
        }

        let mut num_signal_inputs = 0usize;
        let mut num_signal_outputs = 0usize;

        let mut sim_nodes: Vec<RnboNode> = Vec::with_capacity(n);

        for &old_idx in &sorted {
            let (ref id, ref op, arg) = raw_nodes[old_idx];

            match op {
                RnboOp::SignalIn(idx) => num_signal_inputs = num_signal_inputs.max(*idx + 1),
                RnboOp::SignalOut(idx) => num_signal_outputs = num_signal_outputs.max(*idx + 1),
                _ => {}
            }

            let conn = &connections[old_idx];
            let max_inlet = conn.keys().copied().max().unwrap_or(0);
            let num_inlets = if conn.is_empty() { 0 } else { max_inlet + 1 };

            let mut input_sources: Vec<Option<(usize, usize)>> = vec![None; num_inlets];
            for (&inlet, &(src_old, outlet)) in conn {
                if inlet < input_sources.len() {
                    input_sources[inlet] = Some((old_to_new[src_old], outlet));
                }
            }

            let num_outlets = match op {
                RnboOp::NoteIn => 3, // note, velocity, channel
                _ => 1,
            };

            sim_nodes.push(RnboNode {
                op: op.clone(),
                input_sources,
                num_outlets,
                arg,
                box_id: id.clone(),
            });
        }

        // Remap param_indices
        let remapped_params: HashMap<String, usize> = param_indices
            .iter()
            .map(|(name, &old_idx)| (name.clone(), old_to_new[old_idx]))
            .collect();

        let node_outputs: Vec<Vec<f64>> =
            sim_nodes.iter().map(|n| vec![0.0; n.num_outlets]).collect();

        Ok(Self {
            nodes: sim_nodes,
            node_outputs,
            gen_sims,
            param_indices: remapped_params,
            params,
            midi_state: MidiState::new(),
            signal_inputs: vec![0.0; num_signal_inputs],
            signal_outputs: vec![0.0; num_signal_outputs],
            sample_rate,
            num_signal_inputs,
            num_signal_outputs,
        })
    }

    /// Set a named parameter value.
    pub fn set_param(&mut self, name: &str, value: f64) {
        if let Some(state) = self.params.get_mut(name) {
            state.value = value;
        }
    }

    /// Send raw MIDI bytes.
    pub fn send_midi(&mut self, bytes: &[u8]) {
        self.midi_state.process_bytes(bytes);
    }

    /// Convenience: send a Note On.
    pub fn send_note_on(&mut self, note: u8, vel: u8) {
        self.midi_state.note_on(note, vel);
    }

    /// Convenience: send a Note Off.
    pub fn send_note_off(&mut self, note: u8) {
        self.midi_state.note_off(note);
    }

    /// Set signal input samples (for in~ objects).
    pub fn set_signal_input(&mut self, index: usize, value: f64) {
        if index < self.signal_inputs.len() {
            self.signal_inputs[index] = value;
        }
    }

    /// Process a single sample.
    fn process_sample(&mut self) {
        for node_idx in 0..self.nodes.len() {
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

            let results = match &node.op {
                RnboOp::Param(name) => {
                    let val = self.params.get(name).map(|p| p.value).unwrap_or(0.0);
                    vec![val]
                }
                RnboOp::NoteIn => {
                    vec![
                        self.midi_state.note,
                        self.midi_state.velocity,
                        self.midi_state.channel,
                    ]
                }
                RnboOp::Gen(title) => {
                    if let Some(gen_sim) = self.gen_sims.get_mut(title) {
                        // Feed connected inputs to the gen~ simulator
                        for (i, &val) in input_vals.iter().enumerate() {
                            gen_sim.set_input(i, val);
                        }
                        gen_sim.process_sample();
                        // Return the first output (multi-output gen~ can be extended)
                        let outs = gen_sim.get_outputs();
                        if outs.is_empty() {
                            vec![0.0]
                        } else {
                            vec![outs[0]]
                        }
                    } else {
                        vec![0.0]
                    }
                }
                RnboOp::SignalIn(idx) => {
                    vec![self.signal_inputs.get(*idx).copied().unwrap_or(0.0)]
                }
                RnboOp::SignalOut(_) => {
                    vec![input_vals.first().copied().unwrap_or(0.0)]
                }
                RnboOp::Add => {
                    let a = input_vals.first().copied().unwrap_or(0.0);
                    let b = input_vals.get(1).copied().or(node.arg).unwrap_or(0.0);
                    vec![a + b]
                }
                RnboOp::Sub => {
                    let a = input_vals.first().copied().unwrap_or(0.0);
                    let b = input_vals.get(1).copied().or(node.arg).unwrap_or(0.0);
                    vec![a - b]
                }
                RnboOp::Mul => {
                    let a = input_vals.first().copied().unwrap_or(0.0);
                    let b = input_vals.get(1).copied().or(node.arg).unwrap_or(1.0);
                    vec![a * b]
                }
                RnboOp::Div => {
                    let a = input_vals.first().copied().unwrap_or(0.0);
                    let b = input_vals.get(1).copied().or(node.arg).unwrap_or(1.0);
                    vec![if b == 0.0 { 0.0 } else { a / b }]
                }
                RnboOp::Mtof => {
                    let note = input_vals.first().copied().unwrap_or(0.0);
                    vec![440.0 * 2.0_f64.powf((note - 69.0) / 12.0)]
                }
                RnboOp::Pass => {
                    vec![input_vals.first().copied().unwrap_or(0.0)]
                }
            };

            // Store results
            for (i, &val) in results.iter().enumerate() {
                if i < self.node_outputs[node_idx].len() {
                    self.node_outputs[node_idx][i] = val;
                }
            }

            // For SignalOut, copy to outputs
            if let RnboOp::SignalOut(idx) = &node.op {
                if *idx < self.signal_outputs.len() {
                    self.signal_outputs[*idx] = results.first().copied().unwrap_or(0.0);
                }
            }
        }
    }

    /// Run for N samples.
    pub fn run_samples(&mut self, n: usize) -> AudioOutput {
        let num_ch = self.num_signal_outputs.max(1);
        let mut output = AudioOutput::new(num_ch, self.sample_rate);

        for _ in 0..n {
            self.process_sample();
            for ch in 0..self.num_signal_outputs {
                output.channels[ch].push(self.signal_outputs.get(ch).copied().unwrap_or(0.0));
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

/// Parse an RNBO box into an operation.
fn parse_rnbo_op(maxclass: &str, text: &str, box_val: &Value) -> (RnboOp, Option<f64>) {
    let parts: Vec<&str> = text.split_whitespace().collect();
    let name = parts.first().copied().unwrap_or("");

    // Check maxclass first
    match maxclass {
        "newobj" => {} // continue to parse text
        "rnbo~" => return (RnboOp::Pass, None),
        _ => {}
    }

    // Handle by text content
    match name {
        "param" => {
            let param_name = parts.get(1).unwrap_or(&"unnamed").to_string();
            // Find default value: look for a numeric arg, or @value attribute
            let default = parts
                .iter()
                .skip(2)
                .find_map(|s| s.parse::<f64>().ok())
                .or_else(|| parse_attr(text, "value"))
                .unwrap_or(0.0);
            (RnboOp::Param(param_name), Some(default))
        }
        "notein" => (RnboOp::NoteIn, None),
        "gen~" => {
            // Extract title from "@title name" pattern
            let title = parse_attr_str(text, "title").unwrap_or_else(|| "gen".to_string());
            // Check for embedded patcher
            (RnboOp::Gen(title), None)
        }
        "in~" => {
            let n: usize = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
            (RnboOp::SignalIn(n - 1), None)
        }
        "out~" => {
            let n: usize = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
            (RnboOp::SignalOut(n - 1), None)
        }
        "+" => {
            let arg = parts.get(1).and_then(|s| s.parse::<f64>().ok());
            (RnboOp::Add, arg)
        }
        "-" => {
            let arg = parts.get(1).and_then(|s| s.parse::<f64>().ok());
            (RnboOp::Sub, arg)
        }
        "*" | "*~" => {
            let arg = parts.get(1).and_then(|s| s.parse::<f64>().ok());
            (RnboOp::Mul, arg)
        }
        "/" | "/~" => {
            let arg = parts.get(1).and_then(|s| s.parse::<f64>().ok());
            (RnboOp::Div, arg)
        }
        "mtof" => (RnboOp::Mtof, None),
        _ => {
            // Check if it looks like a gen~ box via the patcher's classnamespace
            if let Some(ns) = box_val
                .get("patcher")
                .and_then(|p| p.get("classnamespace"))
                .and_then(|v| v.as_str())
            {
                if ns == "dsp.gen" {
                    let title = parse_attr_str(text, "title").unwrap_or_else(|| "gen".to_string());
                    return (RnboOp::Gen(title), None);
                }
            }
            let arg = parts.get(1).and_then(|s| s.parse::<f64>().ok());
            (RnboOp::Pass, arg)
        }
    }
}

/// Parse a @key value from a text string.
fn parse_attr(text: &str, key: &str) -> Option<f64> {
    let pattern = format!("@{key}");
    let parts: Vec<&str> = text.split_whitespace().collect();
    for (i, &part) in parts.iter().enumerate() {
        if part == pattern {
            return parts.get(i + 1).and_then(|s| s.parse::<f64>().ok());
        }
    }
    None
}

/// Parse a string @key value from text.
fn parse_attr_str(text: &str, key: &str) -> Option<String> {
    let pattern = format!("@{key}");
    let parts: Vec<&str> = text.split_whitespace().collect();
    for (i, &part) in parts.iter().enumerate() {
        if part == pattern {
            return parts.get(i + 1).map(|s| s.to_string());
        }
    }
    None
}

/// Parse a numeric attribute from a JSON box object.
fn parse_attr_f64(box_val: &Value, key: &str, default: f64) -> f64 {
    box_val.get(key).and_then(|v| v.as_f64()).unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create an RNBO patcher JSON.
    fn make_rnbo_json(boxes: &[Value], lines: &[(&str, u64, &str, u64)]) -> String {
        let lines_json: Vec<Value> = lines
            .iter()
            .map(|(src, src_out, dst, dst_in)| {
                serde_json::json!({
                    "patchline": {
                        "source": [src, src_out],
                        "destination": [dst, dst_in]
                    }
                })
            })
            .collect();

        serde_json::json!({
            "patcher": {
                "boxes": boxes,
                "lines": lines_json
            }
        })
        .to_string()
    }

    fn simple_box(id: &str, text: &str) -> Value {
        serde_json::json!({
            "box": {
                "id": id,
                "maxclass": "newobj",
                "text": text
            }
        })
    }

    fn param_box(id: &str, name: &str, default: f64) -> Value {
        serde_json::json!({
            "box": {
                "id": id,
                "maxclass": "newobj",
                "text": format!("param {name} {default}")
            }
        })
    }

    fn gen_box(
        id: &str,
        title: &str,
        gen_boxes: &[(&str, &str)],
        gen_lines: &[(&str, u64, &str, u64)],
    ) -> Value {
        let boxes: Vec<Value> = gen_boxes
            .iter()
            .map(|(gid, text)| serde_json::json!({"box": {"id": gid, "text": text}}))
            .collect();
        let lines: Vec<Value> = gen_lines
            .iter()
            .map(|(src, src_out, dst, dst_in)| {
                serde_json::json!({
                    "patchline": {
                        "source": [src, src_out],
                        "destination": [dst, dst_in]
                    }
                })
            })
            .collect();

        serde_json::json!({
            "box": {
                "id": id,
                "maxclass": "newobj",
                "text": format!("gen~ @title {title}"),
                "patcher": {
                    "classnamespace": "dsp.gen",
                    "boxes": boxes,
                    "lines": lines
                }
            }
        })
    }

    #[test]
    fn test_param_to_output() {
        // param gain 0.5 → out~ 1
        let json = make_rnbo_json(
            &[param_box("p1", "gain", 0.5), simple_box("out", "out~ 1")],
            &[("p1", 0, "out", 0)],
        );
        let mut sim = RnboSimulator::from_json(&json).unwrap();
        let output = sim.run_samples(100);
        // Should output 0.5 on channel 0
        assert!((output.channels[0][0] - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_set_param() {
        let json = make_rnbo_json(
            &[param_box("p1", "gain", 0.0), simple_box("out", "out~ 1")],
            &[("p1", 0, "out", 0)],
        );
        let mut sim = RnboSimulator::from_json(&json).unwrap();
        sim.set_param("gain", 0.75);
        let output = sim.run_samples(10);
        assert!((output.channels[0][0] - 0.75).abs() < 1e-10);
    }

    #[test]
    fn test_notein_to_mtof() {
        // notein → mtof → out~ 1
        let json = make_rnbo_json(
            &[
                simple_box("ni", "notein"),
                simple_box("m", "mtof"),
                simple_box("out", "out~ 1"),
            ],
            &[("ni", 0, "m", 0), ("m", 0, "out", 0)],
        );
        let mut sim = RnboSimulator::from_json(&json).unwrap();
        sim.send_note_on(69, 100); // A4 → 440 Hz
        let output = sim.run_samples(10);
        assert!(
            (output.channels[0][0] - 440.0).abs() < 0.01,
            "Expected ~440, got {}",
            output.channels[0][0]
        );
    }

    #[test]
    fn test_rnbo_with_embedded_gen() {
        // notein → mtof → gen~("sine") → * 0.2 → out~ 1
        // gen~ internals: in 1 → cycle → out 1
        let json = make_rnbo_json(
            &[
                simple_box("ni", "notein"),
                simple_box("m", "mtof"),
                gen_box(
                    "gen",
                    "sine",
                    &[("gi", "in 1"), ("osc", "cycle"), ("go", "out 1")],
                    &[("gi", 0, "osc", 0), ("osc", 0, "go", 0)],
                ),
                simple_box("gain", "* 0.2"),
                simple_box("out", "out~ 1"),
            ],
            &[
                ("ni", 0, "m", 0),
                ("m", 0, "gen", 0),
                ("gen", 0, "gain", 0),
                ("gain", 0, "out", 0),
            ],
        );
        let mut sim = RnboSimulator::from_json(&json).unwrap();
        sim.send_note_on(69, 100); // A4

        let output = sim.run_samples(4410); // 0.1 seconds

        // Should produce audio
        assert!(!output.is_silent(), "Output should not be silent");
        // Peak should be around 0.2
        assert!(output.peak() < 0.25, "Peak too high: {}", output.peak());
        assert!(output.peak() > 0.10, "Peak too low: {}", output.peak());
    }

    #[test]
    fn test_rnbo_mul_with_arg() {
        // param vol 0.8 → * 0.5 → out~ 1
        let json = make_rnbo_json(
            &[
                param_box("p", "vol", 0.8),
                simple_box("mul", "* 0.5"),
                simple_box("out", "out~ 1"),
            ],
            &[("p", 0, "mul", 0), ("mul", 0, "out", 0)],
        );
        let mut sim = RnboSimulator::from_json(&json).unwrap();
        let output = sim.run_samples(10);
        assert!(
            (output.channels[0][0] - 0.4).abs() < 1e-10,
            "Expected 0.4, got {}",
            output.channels[0][0]
        );
    }

    #[test]
    fn test_note_off_silence() {
        // notein → mtof → gen~("sine") → out~ 1
        // After note off, velocity = 0 but freq still plays (need gate logic)
        // This test just verifies note off sets velocity to 0
        let json = make_rnbo_json(
            &[simple_box("ni", "notein"), simple_box("out", "out~ 1")],
            &[("ni", 1, "out", 0)], // velocity → out
        );
        let mut sim = RnboSimulator::from_json(&json).unwrap();
        sim.send_note_on(60, 100);
        let output = sim.run_samples(1);
        assert!((output.channels[0][0] - 100.0).abs() < 1e-10);

        sim.send_note_off(60);
        let output = sim.run_samples(1);
        assert!((output.channels[0][0]).abs() < 1e-10);
    }

    #[test]
    fn test_run_seconds() {
        let json = make_rnbo_json(
            &[param_box("p", "val", 1.0), simple_box("out", "out~ 1")],
            &[("p", 0, "out", 0)],
        );
        let mut sim = RnboSimulator::from_json(&json).unwrap();
        let output = sim.run_seconds(0.01);
        assert_eq!(output.channels[0].len(), 441);
    }

    #[test]
    fn test_empty_rnbo() {
        let json = r#"{"patcher": {"boxes": [], "lines": []}}"#;
        let sim = RnboSimulator::from_json(json).unwrap();
        assert_eq!(sim.nodes.len(), 0);
    }

    #[test]
    fn test_signal_input() {
        // in~ 1 → * 2 → out~ 1
        let json = make_rnbo_json(
            &[
                simple_box("inp", "in~ 1"),
                simple_box("mul", "* 2"),
                simple_box("out", "out~ 1"),
            ],
            &[("inp", 0, "mul", 0), ("mul", 0, "out", 0)],
        );
        let mut sim = RnboSimulator::from_json(&json).unwrap();
        sim.set_signal_input(0, 0.3);
        let output = sim.run_samples(1);
        assert!(
            (output.channels[0][0] - 0.6).abs() < 1e-10,
            "Expected 0.6, got {}",
            output.channels[0][0]
        );
    }
}
