//! Roundtrip tests: .maxpat -> decompile -> .flutmax -> compile -> .maxpat
//!
//! These tests verify that decompiling a .maxpat to .flutmax and recompiling it
//! produces a logically equivalent graph. The comparison ignores layout, IDs,
//! and formatting — only the logical topology (nodes + edges) must match.

use std::collections::{BTreeMap, BTreeSet, HashMap};

// ---------------------------------------------------------------------------
// Logical graph types (duplicated from graph_equivalence.rs since test modules
// cannot be shared as library code)
// ---------------------------------------------------------------------------

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
    // Skip comment boxes — they are non-functional (display only) and the
    // decompiler emits them as flutmax comments, so they don't roundtrip.
    let mut raw_texts: Vec<(String, String, String)> = Vec::new(); // (id, maxclass, raw_text)
    let mut text_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut comment_ids: BTreeSet<String> = BTreeSet::new();

    for box_wrapper in boxes {
        let b = &box_wrapper["box"];
        let id = b["id"].as_str().expect("box missing id").to_string();
        let maxclass = b["maxclass"]
            .as_str()
            .expect("box missing maxclass")
            .to_string();

        // Exclude comment boxes from the logical graph
        if maxclass == "comment" {
            comment_ids.insert(id);
            continue;
        }

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

        // Skip edges involving comment boxes
        if comment_ids.contains(source_id) || comment_ids.contains(dest_id) {
            continue;
        }

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

// ---------------------------------------------------------------------------
// Roundtrip helper
// ---------------------------------------------------------------------------

/// Perform a full roundtrip test:
/// 1. Decompile .maxpat JSON to .flutmax source
/// 2. Compile .flutmax source back to .maxpat JSON
/// 3. Extract logical graphs from both original and regenerated
/// 4. Assert graph equivalence
fn assert_roundtrip(original_maxpat: &str, label: &str) {
    // Step 1: Decompile .maxpat -> .flutmax
    let flutmax_source = flutmax_decompile::decompile(original_maxpat)
        .unwrap_or_else(|e| panic!("decompile failed for {}: {}", label, e));

    // Sanity check: the decompiled source should not be empty
    assert!(
        !flutmax_source.trim().is_empty(),
        "decompiled .flutmax source is empty for {}",
        label
    );

    // Step 2: Compile .flutmax -> .maxpat
    let regenerated_maxpat = flutmax_cli::compile(&flutmax_source)
        .unwrap_or_else(|e| panic!("compile failed for {} (roundtrip): {}", label, e));

    // Step 3: Extract logical graphs
    let orig_graph = extract_logical_graph(original_maxpat);
    let regen_graph = extract_logical_graph(&regenerated_maxpat);

    // Step 4: Compare
    assert_eq!(
        orig_graph.nodes, regen_graph.nodes,
        "\nRoundtrip node mismatch for {}.\n\nOriginal nodes:\n{:#?}\n\nRegenerated nodes:\n{:#?}",
        label, orig_graph.nodes, regen_graph.nodes
    );
    assert_eq!(
        orig_graph.edges, regen_graph.edges,
        "\nRoundtrip edge mismatch for {}.\n\nOriginal edges:\n{:#?}\n\nRegenerated edges:\n{:#?}",
        label, orig_graph.edges, regen_graph.edges
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn roundtrip_l1_sine() {
    let original = include_str!("../../../tests/e2e/expected/L1_sine.maxpat");
    assert_roundtrip(original, "L1_sine");
}

#[test]
fn roundtrip_l2_simple_synth() {
    let original = include_str!("../../../tests/e2e/expected/L2_simple_synth.maxpat");
    assert_roundtrip(original, "L2_simple_synth");
}

#[test]
fn roundtrip_l3_trigger_fanout() {
    let original = include_str!("../../../tests/e2e/expected/L3_trigger_fanout.maxpat");
    assert_roundtrip(original, "L3_trigger_fanout");
}

/// L3b contains a trigger object. The decompiler should remove it during
/// decompilation, and the compiler should re-insert it. The logical graphs
/// should match if trigger removal + re-insertion is correct.
#[test]
fn roundtrip_l3b_control_fanout() {
    let original = include_str!("../../../tests/e2e/expected/L3b_control_fanout.maxpat");
    assert_roundtrip(original, "L3b_control_fanout");
}
