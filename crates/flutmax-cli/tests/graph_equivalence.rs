//! Logical graph equivalence tests for flutmax.
//!
//! These tests verify that flutmax-generated .maxpat files have the correct
//! object graph structure by comparing logical graphs (ignoring layout, IDs,
//! and formatting).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

/// A logical graph extracted from a .maxpat, ignoring IDs, coordinates, and formatting.
#[derive(Debug, PartialEq, Eq)]
struct LogicalGraph {
    nodes: BTreeSet<LogicalNode>,
    edges: BTreeSet<LogicalEdge>,
}

/// A node in the logical graph, identified by its maxclass and text.
/// When multiple nodes share the same (maxclass, text), they are disambiguated
/// with a `#N` suffix on the text field.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
struct LogicalNode {
    maxclass: String,
    text: String,
}

/// An edge in the logical graph, using node-identifying text instead of IDs.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct LogicalEdge {
    source_text: String,
    source_outlet: u32,
    dest_text: String,
    dest_inlet: u32,
}

/// Extract a LogicalGraph from a .maxpat JSON string.
///
/// Node identification:
/// - `newobj` boxes: use the `text` field (e.g., "cycle~ 440", "*~ 0.5")
/// - Other boxes (inlet, outlet, outlet~, etc.): use `maxclass` + optional `comment`
///   - inlet with comment "freq" -> text = "inlet:freq"
///   - outlet~ with no comment -> text = "outlet~"
///
/// When multiple boxes resolve to the same text, a `#N` suffix is appended to
/// disambiguate (e.g., "outlet#0", "outlet#1"). The suffixes are assigned in
/// the order the boxes appear in the JSON array, which ensures a deterministic
/// mapping.
fn extract_logical_graph(maxpat_json: &str) -> LogicalGraph {
    let root: serde_json::Value =
        serde_json::from_str(maxpat_json).expect("failed to parse .maxpat JSON");

    let patcher = &root["patcher"];
    let boxes = patcher["boxes"].as_array().expect("missing boxes array");
    let lines = patcher["lines"].as_array().expect("missing lines array");

    // First pass: compute the raw text for each box and count occurrences.
    let mut raw_texts: Vec<(String, String, String)> = Vec::new(); // (id, maxclass, raw_text)
    let mut text_counts: BTreeMap<String, usize> = BTreeMap::new();

    for box_wrapper in boxes {
        let b = &box_wrapper["box"];
        let id = b["id"].as_str().expect("box missing id").to_string();
        let maxclass = b["maxclass"]
            .as_str()
            .expect("box missing maxclass")
            .to_string();

        let raw_text = if maxclass == "newobj" {
            b["text"].as_str().expect("newobj missing text").to_string()
        } else {
            match b.get("comment").and_then(|c| c.as_str()) {
                Some(comment) if !comment.is_empty() => {
                    format!("{}:{}", maxclass, comment)
                }
                _ => maxclass.clone(),
            }
        };

        *text_counts.entry(raw_text.clone()).or_insert(0) += 1;
        raw_texts.push((id, maxclass, raw_text));
    }

    // Second pass: assign disambiguated text for duplicates.
    let mut id_to_node: HashMap<String, LogicalNode> = HashMap::new();
    let mut dup_counters: HashMap<String, usize> = HashMap::new();

    for (id, maxclass, raw_text) in &raw_texts {
        let text = if text_counts[raw_text] > 1 {
            let idx = dup_counters.entry(raw_text.clone()).or_insert(0);
            let disambiguated = format!("{}#{}", raw_text, idx);
            *idx += 1;
            disambiguated
        } else {
            raw_text.clone()
        };

        id_to_node.insert(
            id.clone(),
            LogicalNode {
                maxclass: maxclass.clone(),
                text,
            },
        );
    }

    // Collect all nodes
    let nodes: BTreeSet<LogicalNode> = id_to_node.values().cloned().collect();

    // Build edges by resolving IDs to node texts
    let mut edges: BTreeSet<LogicalEdge> = BTreeSet::new();

    for line_wrapper in lines {
        let patchline = &line_wrapper["patchline"];
        let source = patchline["source"]
            .as_array()
            .expect("patchline missing source");
        let dest = patchline["destination"]
            .as_array()
            .expect("patchline missing destination");

        let source_id = source[0].as_str().expect("source id not a string");
        let source_outlet = source[1].as_u64().expect("source outlet not a number") as u32;
        let dest_id = dest[0].as_str().expect("dest id not a string");
        let dest_inlet = dest[1].as_u64().expect("dest inlet not a number") as u32;

        let source_node = id_to_node
            .get(source_id)
            .unwrap_or_else(|| panic!("unknown source id: {}", source_id));
        let dest_node = id_to_node
            .get(dest_id)
            .unwrap_or_else(|| panic!("unknown dest id: {}", dest_id));

        edges.insert(LogicalEdge {
            source_text: source_node.text.clone(),
            source_outlet,
            dest_text: dest_node.text.clone(),
            dest_inlet,
        });
    }

    LogicalGraph { nodes, edges }
}

/// Return the workspace root directory (two levels up from CARGO_MANIFEST_DIR).
fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .expect("no parent of crates/flutmax-cli")
        .parent()
        .expect("no grandparent")
        .to_path_buf()
}

/// Read a fixture .flutmax file.
fn read_fixture(name: &str) -> String {
    let path = workspace_root().join("tests/e2e/fixtures").join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read fixture {}: {}", path.display(), e))
}

/// Read an expected .maxpat file.
fn read_expected(name: &str) -> String {
    let path = workspace_root().join("tests/e2e/expected").join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read expected {}: {}", path.display(), e))
}

/// Compile a .flutmax source, extract the logical graph from the result, and
/// compare it against the logical graph extracted from the expected .maxpat.
fn assert_graph_equivalence(fixture_name: &str, expected_name: &str) {
    // 1. Read the .flutmax source
    let source = read_fixture(fixture_name);

    // 2. Compile through the full pipeline
    let generated_json = flutmax_cli::compile(&source)
        .unwrap_or_else(|e| panic!("compilation of {} failed: {}", fixture_name, e));

    // 3. Extract logical graph from generated output
    let generated_graph = extract_logical_graph(&generated_json);

    // 4. Read expected .maxpat and extract its logical graph
    let expected_json = read_expected(expected_name);
    let expected_graph = extract_logical_graph(&expected_json);

    // 5. Compare
    assert_eq!(
        generated_graph.nodes, expected_graph.nodes,
        "\nNode mismatch for {}.\n\nGenerated nodes:\n{:#?}\n\nExpected nodes:\n{:#?}",
        fixture_name, generated_graph.nodes, expected_graph.nodes
    );
    assert_eq!(
        generated_graph.edges, expected_graph.edges,
        "\nEdge mismatch for {}.\n\nGenerated edges:\n{:#?}\n\nExpected edges:\n{:#?}",
        fixture_name, generated_graph.edges, expected_graph.edges
    );
}

#[test]
fn test_l1_sine_graph_equivalence() {
    assert_graph_equivalence("L1_sine.flutmax", "L1_sine.maxpat");
}

#[test]
fn test_l2_simple_synth_graph_equivalence() {
    assert_graph_equivalence("L2_simple_synth.flutmax", "L2_simple_synth.maxpat");
}

#[test]
fn test_l3_trigger_fanout_graph_equivalence() {
    assert_graph_equivalence("L3_trigger_fanout.flutmax", "L3_trigger_fanout.maxpat");
}

#[test]
fn test_l3b_control_fanout_graph_equivalence() {
    assert_graph_equivalence("L3b_control_fanout.flutmax", "L3b_control_fanout.maxpat");
}

// ─── Abstraction (multi-file) tests ───

/// Read a fixture from the abstraction subdirectory.
fn read_abstraction_fixture(name: &str) -> String {
    let path = workspace_root()
        .join("tests/e2e/fixtures/abstraction")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read fixture {}: {}", path.display(), e))
}

/// Test that compiling a directory of .flutmax files produces correct
/// abstraction references with proper numinlets/numoutlets.
#[test]
fn test_abstraction_directory_compile() {
    use flutmax_sema::registry::AbstractionRegistry;

    // 1. Read both .flutmax files
    let osc_source = read_abstraction_fixture("oscillator.flutmax");
    let fm_source = read_abstraction_fixture("fm_synth.flutmax");

    // 2. Parse both
    let osc_ast = flutmax_parser::parse(&osc_source).expect("oscillator.flutmax should parse");
    let fm_ast = flutmax_parser::parse(&fm_source).expect("fm_synth.flutmax should parse");

    // 3. Register all in the registry
    let mut registry = AbstractionRegistry::new();
    registry.register("oscillator", &osc_ast);
    registry.register("fm_synth", &fm_ast);

    // 4. Compile oscillator with registry
    let osc_json = flutmax_cli::compile_with_registry(&osc_source, Some(&registry))
        .expect("oscillator should compile");
    let osc_parsed: serde_json::Value =
        serde_json::from_str(&osc_json).expect("oscillator output should be valid JSON");

    // oscillator.maxpat has: inlet, cycle~, outlet~
    let osc_boxes = osc_parsed["patcher"]["boxes"].as_array().unwrap();
    assert_eq!(osc_boxes.len(), 3, "oscillator should have 3 boxes");

    // 5. Compile fm_synth with registry
    let fm_json = flutmax_cli::compile_with_registry(&fm_source, Some(&registry))
        .expect("fm_synth should compile");
    let fm_parsed: serde_json::Value =
        serde_json::from_str(&fm_json).expect("fm_synth output should be valid JSON");

    // fm_synth.maxpat has: inlet, outlet~, oscillator, *~
    let fm_boxes = fm_parsed["patcher"]["boxes"].as_array().unwrap();
    assert_eq!(fm_boxes.len(), 4, "fm_synth should have 4 boxes");

    // Check that an "oscillator" box exists with correct properties
    let osc_box = fm_boxes
        .iter()
        .find(|b| {
            b["box"]["text"]
                .as_str()
                .map(|t| t.starts_with("oscillator"))
                .unwrap_or(false)
        })
        .expect("fm_synth should contain an oscillator box");

    // oscillator has 1 in_port and 1 out_port
    assert_eq!(
        osc_box["box"]["numinlets"].as_u64().unwrap(),
        1,
        "oscillator box should have 1 inlet"
    );
    assert_eq!(
        osc_box["box"]["numoutlets"].as_u64().unwrap(),
        1,
        "oscillator box should have 1 outlet"
    );

    // The text should be "oscillator" (the abstraction name, no alias)
    let text = osc_box["box"]["text"].as_str().unwrap();
    assert!(
        text == "oscillator",
        "oscillator box text should be 'oscillator', got '{}'",
        text
    );
}

/// Test that fm_synth compiles with registry and the oscillator box
/// has signal-type outlets (since oscillator's out[0] is signal).
#[test]
fn test_abstraction_signal_type_propagation() {
    use flutmax_sema::registry::AbstractionRegistry;

    let osc_source = read_abstraction_fixture("oscillator.flutmax");
    let fm_source = read_abstraction_fixture("fm_synth.flutmax");

    let osc_ast = flutmax_parser::parse(&osc_source).unwrap();
    let fm_ast = flutmax_parser::parse(&fm_source).unwrap();

    let mut registry = AbstractionRegistry::new();
    registry.register("oscillator", &osc_ast);
    registry.register("fm_synth", &fm_ast);

    let fm_json = flutmax_cli::compile_with_registry(&fm_source, Some(&registry)).unwrap();
    let fm_parsed: serde_json::Value = serde_json::from_str(&fm_json).unwrap();
    let fm_boxes = fm_parsed["patcher"]["boxes"].as_array().unwrap();

    let osc_box = fm_boxes
        .iter()
        .find(|b| {
            b["box"]["text"]
                .as_str()
                .map(|t| t.starts_with("oscillator"))
                .unwrap_or(false)
        })
        .expect("fm_synth should contain an oscillator box");

    // outlettype should contain "signal" since oscillator out[0] is signal
    let outlettype = osc_box["box"]["outlettype"].as_array().unwrap();
    assert_eq!(outlettype.len(), 1);
    assert_eq!(
        outlettype[0].as_str().unwrap(),
        "signal",
        "oscillator outlet should be signal type"
    );
}

/// Test that compile_with_registry(None) works the same as compile().
#[test]
fn test_compile_with_none_registry_same_as_compile() {
    let source = read_fixture("L2_simple_synth.flutmax");

    let result1 = flutmax_cli::compile(&source).unwrap();
    let result2 = flutmax_cli::compile_with_registry(&source, None).unwrap();

    // Both should produce identical JSON
    let parsed1: serde_json::Value = serde_json::from_str(&result1).unwrap();
    let parsed2: serde_json::Value = serde_json::from_str(&result2).unwrap();

    assert_eq!(parsed1, parsed2);
}
