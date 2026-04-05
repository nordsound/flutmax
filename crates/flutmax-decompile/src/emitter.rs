use crate::analyzer::DecompiledPatch;
use crate::parser::DecompileError;
use std::collections::{HashMap, HashSet};

/// Format inlet index for `.in` syntax: `[N]` for N > 0, empty for inlet 0.
fn format_inlet_index(inlet: u32) -> String {
    if inlet == 0 {
        String::new()
    } else {
        format!("[{}]", inlet)
    }
}

/// Emit a DecompiledPatch as .flutmax source code.
///
/// Output is organized for readability:
/// 1. Comments not associated with any wire (top-level)
/// 2. Port declarations (in/out)
/// 3. Message declarations
/// 4. Wire groups: each wire declaration followed by its direct connections,
///    with proximity-matched comments placed before their nearest wire.
/// 5. Remaining out assignments
///
/// Blank lines separate independent groups (wires whose expressions don't
/// reference recently defined wires).
pub fn emit(patch: &DecompiledPatch) -> String {
    let mut output = String::new();

    // Build wire name -> Y position map from ui_entries for comment proximity
    let wire_y_positions: HashMap<&str, f64> = patch.ui_entries.iter()
        .map(|e| (e.name.as_str(), e.rect[1]))
        .collect();

    // Sort comments by Y position for ordered placement
    let mut sorted_comments: Vec<&crate::analyzer::CommentInfo> = patch.comments.iter().collect();
    sorted_comments.sort_by(|a, b| a.y_position.partial_cmp(&b.y_position).unwrap_or(std::cmp::Ordering::Equal));

    // Match each comment to the nearest wire below it (by Y position).
    // A comment is associated with wire W if:
    //   comment.y < wire_W.y AND no other wire has a Y between comment.y and wire_W.y
    let mut comment_to_wire: HashMap<usize, Vec<usize>> = HashMap::new(); // wire_index -> comment_indices
    let mut unmatched_comments: Vec<usize> = Vec::new();

    // Build sorted wire Y values for matching
    let wire_positions: Vec<(&str, f64)> = patch.wires.iter()
        .map(|w| (w.name.as_str(), *wire_y_positions.get(w.name.as_str()).unwrap_or(&f64::MAX)))
        .collect();

    for (ci, comment) in sorted_comments.iter().enumerate() {
        // Find the first wire whose Y is >= comment's Y (the wire just below or at the comment)
        let mut best_wire: Option<(usize, f64)> = None;
        for (wi, (_name, wy)) in wire_positions.iter().enumerate() {
            if *wy >= comment.y_position {
                if best_wire.is_none() || *wy < best_wire.unwrap().1 {
                    best_wire = Some((wi, *wy));
                }
            }
        }
        if let Some((wi, _)) = best_wire {
            comment_to_wire.entry(wi).or_default().push(ci);
        } else {
            // Comment is below all wires — check if it's closest to the last wire (above it)
            let mut best_above: Option<(usize, f64)> = None;
            for (wi, (_name, wy)) in wire_positions.iter().enumerate() {
                if *wy < comment.y_position {
                    if best_above.is_none() || *wy > best_above.unwrap().1 {
                        best_above = Some((wi, *wy));
                    }
                }
            }
            if let Some((wi, _)) = best_above {
                comment_to_wire.entry(wi).or_default().push(ci);
            } else {
                unmatched_comments.push(ci);
            }
        }
    }

    // Emit unmatched comments at the top (no wires to associate with)
    for &ci in &unmatched_comments {
        emit_comment(&mut output, &sorted_comments[ci].text);
    }
    if !unmatched_comments.is_empty() {
        output.push('\n');
    }

    // Port declarations: in (implicit index — declaration order = index)
    for decl in &patch.in_decls {
        output.push_str(&format!(
            "in {}: {};\n",
            decl.name, decl.port_type
        ));
    }

    // Count out_assignments per index. Only merge if exactly one assignment per index.
    let mut out_assign_count: HashMap<u32, usize> = HashMap::new();
    for a in &patch.out_assignments {
        *out_assign_count.entry(a.index).or_insert(0) += 1;
    }
    // Build a map of out_assignment index → wire_name for inline merging (only for unique assignments)
    let out_assign_map: HashMap<u32, &str> = patch.out_assignments.iter()
        .filter(|a| out_assign_count.get(&a.index) == Some(&1))
        .map(|a| (a.index, a.wire_name.as_str()))
        .collect();
    // Track which out_assignment indices have been merged into out declarations
    let mut merged_out_indices: HashSet<u32> = HashSet::new();

    // Port declarations: out (implicit index — declaration order = index)
    // If exactly one out_assignment exists for this index, emit inline form: `out name: type = expr;`
    for decl in &patch.out_decls {
        if let Some(wire_name) = out_assign_map.get(&decl.index) {
            output.push_str(&format!(
                "out {}: {} = {};\n",
                decl.name, decl.port_type, wire_name
            ));
            merged_out_indices.insert(decl.index);
        } else {
            output.push_str(&format!(
                "out {}: {};\n",
                decl.name, decl.port_type
            ));
        }
    }

    // Blank line after declarations if there are any
    if !patch.in_decls.is_empty() || !patch.out_decls.is_empty() {
        output.push('\n');
    }

    // Message declarations
    for msg in &patch.messages {
        if msg.attrs.is_empty() {
            output.push_str(&format!("msg {} = \"{}\";\n", msg.name, escape_string(&msg.content)));
        } else {
            let attr_pairs: Vec<String> = msg.attrs.iter()
                .map(|(k, v)| format!("{}: {}", k, v))
                .collect();
            output.push_str(&format!("msg {} = \"{}\".attr({});\n",
                msg.name, escape_string(&msg.content), attr_pairs.join(", ")));
        }
    }
    if !patch.messages.is_empty() {
        output.push('\n');
    }

    // Build set of wire names and collect connections per wire
    let wire_name_set: HashSet<&str> = patch.wires.iter().map(|w| w.name.as_str()).collect();
    // Index direct connections by target wire for grouped emission
    let mut connections_by_target: HashMap<&str, Vec<&crate::analyzer::DirectConnectionInfo>> = HashMap::new();
    for dc in &patch.direct_connections {
        connections_by_target.entry(dc.target_wire.as_str()).or_default().push(dc);
    }

    // Wire declarations with grouped connections and proximity comments
    // Track recently defined wire names for block separation
    let mut recent_names: Vec<String> = Vec::new();
    let window_size = 4; // How many recent wires to check for references

    for (wi, wire) in patch.wires.iter().enumerate() {
        // Block separation: add extra blank line if this wire doesn't reference
        // any of the recently defined wires (indicates a new independent group)
        if !recent_names.is_empty() {
            let references_recent = recent_names.iter()
                .rev()
                .take(window_size)
                .any(|name| wire.expr.contains(name.as_str()));
            if !references_recent {
                output.push('\n');
            }
        }

        // Emit proximity-matched comments before this wire
        if let Some(comment_indices) = comment_to_wire.get(&wi) {
            for &ci in comment_indices {
                emit_comment(&mut output, &sorted_comments[ci].text);
            }
        }

        // Emit wire declaration
        if wire.attrs.is_empty() {
            output.push_str(&format!("wire {} = {};\n", wire.name, wire.expr));
        } else {
            let attr_pairs: Vec<String> = wire.attrs.iter()
                .map(|(k, v)| format!("{}: {}", k, v))
                .collect();
            output.push_str(&format!("wire {} = {}.attr({});\n",
                wire.name, wire.expr, attr_pairs.join(", ")));
        }

        // Emit direct connections that target this wire
        if let Some(conns) = connections_by_target.get(wire.name.as_str()) {
            for dc in conns {
                output.push_str(&format!("{}.in{} = {};\n", dc.target_wire, format_inlet_index(dc.inlet), dc.source_wire));
            }
        }

        recent_names.push(wire.name.clone());
    }

    // Emit remaining direct connections targeting messages (not wires)
    let mut has_remaining_dc = false;
    for dc in &patch.direct_connections {
        if !wire_name_set.contains(dc.target_wire.as_str()) {
            if !has_remaining_dc && !patch.wires.is_empty() {
                output.push('\n');
            }
            has_remaining_dc = true;
            output.push_str(&format!("{}.in{} = {};\n", dc.target_wire, format_inlet_index(dc.inlet), dc.source_wire));
        }
    }

    // Remaining out assignments (those not merged into out declarations)
    let remaining_out_assigns: Vec<_> = patch.out_assignments.iter()
        .filter(|a| !merged_out_indices.contains(&a.index))
        .collect();
    if (!patch.wires.is_empty() || !patch.direct_connections.is_empty()) && !remaining_out_assigns.is_empty() {
        output.push('\n');
    }
    for assign in &remaining_out_assigns {
        output.push_str(&format!("out[{}] = {};\n", assign.index, assign.wire_name));
    }

    output
}

/// Emit a comment box as flutmax comment lines.
fn emit_comment(output: &mut String, text: &str) {
    for line in text.lines() {
        output.push_str(&format!("// {}\n", line));
    }
}

/// Escape special characters in string literals for .flutmax output.
fn escape_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Emit a .uiflutmax sidecar file from the DecompiledPatch UI data.
///
/// Returns `None` if there are no UI entries (no positions or decorative attrs to record).
/// Returns `Some(json_string)` with the pretty-printed JSON content.
pub fn emit_ui_file(patch: &DecompiledPatch) -> Option<String> {
    use serde_json::{json, Map, Value};

    // Only generate if there's meaningful data
    let has_visual = !patch.comments.is_empty() || !patch.panels.is_empty() || !patch.images.is_empty();
    if patch.ui_entries.is_empty() && patch.patcher_rect.is_none() && !has_visual {
        return None;
    }

    let mut root = Map::new();

    // Patcher-level settings
    if let Some(rect) = patch.patcher_rect {
        let patcher_obj = json!({
            "rect": [rect[0], rect[1], rect[2], rect[3]]
        });
        root.insert("_patcher".to_string(), patcher_obj);
    }

    // Per-wire/message UI entries
    for entry in &patch.ui_entries {
        let mut obj = Map::new();

        // Always include rect (position)
        let rect_val = json!([entry.rect[0], entry.rect[1], entry.rect[2], entry.rect[3]]);
        obj.insert("rect".to_string(), rect_val);

        // Decorative attributes
        for (k, v) in &entry.decorative_attrs {
            // Try to parse as number, otherwise store as string
            if let Ok(n) = v.parse::<f64>() {
                if n == n.floor() && n.abs() < i64::MAX as f64 {
                    obj.insert(k.clone(), json!(n as i64));
                } else {
                    obj.insert(k.clone(), json!(n));
                }
            } else if v.starts_with('"') && v.ends_with('"') {
                // Quoted string — unquote for JSON
                let inner = &v[1..v.len()-1];
                obj.insert(k.clone(), Value::String(inner.to_string()));
            } else {
                obj.insert(k.clone(), Value::String(v.clone()));
            }
        }

        root.insert(entry.name.clone(), Value::Object(obj));
    }

    // _comments: position data for comment boxes (text is in .flutmax as // lines)
    if !patch.comments.is_empty() {
        let comments: Vec<Value> = patch.comments.iter().map(|c| {
            json!({
                "text": c.text,
                "rect": [c.rect[0], c.rect[1], c.rect[2], c.rect[3]]
            })
        }).collect();
        root.insert("_comments".to_string(), json!(comments));
    }

    // _panels: visual-only panel boxes
    if !patch.panels.is_empty() {
        let panels: Vec<Value> = patch.panels.iter().map(|p| {
            let mut obj = json!({ "rect": [p.rect[0], p.rect[1], p.rect[2], p.rect[3]] });
            for (k, v) in &p.attrs {
                if let Ok(n) = v.parse::<f64>() {
                    if n == n.floor() && n.abs() < i64::MAX as f64 {
                        obj[k] = json!(n as i64);
                    } else {
                        obj[k] = json!(n);
                    }
                } else if v.starts_with('"') && v.ends_with('"') {
                    let inner = &v[1..v.len()-1];
                    obj[k] = json!(inner);
                } else {
                    obj[k] = json!(v);
                }
            }
            obj
        }).collect();
        root.insert("_panels".to_string(), json!(panels));
    }

    // _images: visual-only image boxes (fpic)
    if !patch.images.is_empty() {
        let images: Vec<Value> = patch.images.iter().map(|i| {
            json!({
                "rect": [i.rect[0], i.rect[1], i.rect[2], i.rect[3]],
                "pic": i.pic
            })
        }).collect();
        root.insert("_images".to_string(), json!(images));
    }

    Some(serde_json::to_string_pretty(&Value::Object(root)).unwrap())
}

/// Main decompile entry point: .maxpat JSON string -> .flutmax source string.
///
/// This is the single-file API that skips subpatchers (backward compatible).
/// For multi-file decompilation with subpatcher extraction, use `decompile_multi`.
/// Uses positional arguments only. For named arguments, use `decompile_with_objdb`.
pub fn decompile(json_str: &str) -> Result<String, DecompileError> {
    let maxpat = crate::parser::parse_maxpat(json_str)?;
    let patch = crate::analyzer::analyze(&maxpat, None)?;
    Ok(emit(&patch))
}

/// Decompile with objdb for named arguments.
///
/// When the object database is provided, wire expressions use inlet names from
/// objdb (e.g., `biquad~(input: osc, frequency: cutoff)` instead of positional args).
pub fn decompile_with_objdb(
    json_str: &str,
    objdb: Option<&flutmax_objdb::ObjectDb>,
) -> Result<String, DecompileError> {
    let maxpat = crate::parser::parse_maxpat(json_str)?;
    let patch = crate::analyzer::analyze(&maxpat, objdb)?;
    Ok(emit(&patch))
}

/// Decompile result: main file + extracted subpatcher files.
#[derive(Debug)]
pub struct DecompileResult {
    /// All generated .flutmax files (filename → source code).
    pub files: HashMap<String, String>,
    /// Code files extracted from codebox objects (filename -> code content).
    pub code_files: HashMap<String, String>,
    /// Filenames of RNBO subpatchers (classnamespace: "rnbo").
    pub rnbo_patchers: HashSet<String>,
    /// The main file name (key into `files`).
    pub main_file: String,
}

/// Decompile a .maxpat JSON string into potentially multiple .flutmax files.
///
/// Embedded subpatchers (`[p name]`, `bpatcher`, `poly~`, `pfft~`) are
/// recursively decompiled into separate files. The parent patch references
/// them as abstraction calls.
pub fn decompile_multi(json_str: &str, main_name: &str) -> Result<DecompileResult, DecompileError> {
    decompile_multi_inner(json_str, main_name, None)
}

/// Multi-file decompile with objdb for named arguments.
pub fn decompile_multi_with_objdb(
    json_str: &str,
    main_name: &str,
    objdb: Option<&flutmax_objdb::ObjectDb>,
) -> Result<DecompileResult, DecompileError> {
    decompile_multi_inner(json_str, main_name, objdb)
}

fn decompile_multi_inner(
    json_str: &str,
    main_name: &str,
    objdb: Option<&flutmax_objdb::ObjectDb>,
) -> Result<DecompileResult, DecompileError> {
    let maxpat = crate::parser::parse_maxpat(json_str)?;
    let (patch, subpatchers) = crate::analyzer::analyze_recursive(&maxpat, main_name, objdb)?;

    let mut files = HashMap::new();
    let mut code_files = HashMap::new();
    let mut rnbo_patchers = HashSet::new();
    let main_file = format!("{}.flutmax", main_name);
    let main_source = emit(&patch);
    files.insert(main_file.clone(), main_source);

    // Detect RNBO subpatchers by matching embedded patchers (in order) to
    // the subpatcher names returned by analyze_recursive (same iteration order).
    let embedded_boxes: Vec<_> = maxpat.boxes.iter()
        .filter(|b| b.embedded_patcher.is_some())
        .collect();
    for (i, b) in embedded_boxes.iter().enumerate() {
        if let Some(ref embedded) = b.embedded_patcher {
            if embedded.classnamespace.as_deref() == Some("rnbo") {
                if let Some((name, _)) = subpatchers.get(i) {
                    rnbo_patchers.insert(format!("{}.flutmax", name));
                }
            }
        }
    }

    // Collect code files from the main patch
    for (filename, content) in &patch.code_files {
        code_files.insert(filename.clone(), content.clone());
    }

    for (name, sub_patch) in subpatchers {
        let sub_source = emit(&sub_patch);
        files.insert(format!("{}.flutmax", name), sub_source);
        // Collect code files from subpatches
        for (filename, content) in &sub_patch.code_files {
            code_files.insert(filename.clone(), content.clone());
        }
    }

    Ok(DecompileResult { files, code_files, rnbo_patchers, main_file })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::{CommentInfo, DecompiledPatch, InDeclInfo, MsgInfo, OutAssignInfo, OutDeclInfo, UiEntryInfo, WireInfo};

    #[test]
    fn emit_simple_patch() {
        let patch = DecompiledPatch {
            in_decls: vec![],
            out_decls: vec![OutDeclInfo {
                index: 0,
                name: "out_0".into(),
                port_type: "signal".into(),
            }],
            comments: vec![],
            messages: vec![],
            wires: vec![WireInfo {
                name: "w_1".into(),
                expr: "cycle~(440)".into(),
                attrs: vec![],
            }],
            out_assignments: vec![OutAssignInfo {
                index: 0,
                wire_name: "w_1".into(),
            }],
            direct_connections: vec![],
            code_files: vec![],
            ui_entries: vec![],
            patcher_rect: None,
            panels: vec![],
            images: vec![],
        };

        let source = emit(&patch);
        // Inline form: out declaration merged with out assignment
        assert!(source.contains("out out_0: signal = w_1;"));
        assert!(source.contains("wire w_1 = cycle~(440);"));
        // No separate out[0] = ... line
        assert!(!source.contains("out[0]"));
    }

    #[test]
    fn emit_with_inlet() {
        let patch = DecompiledPatch {
            in_decls: vec![InDeclInfo {
                index: 0,
                name: "freq".into(),
                port_type: "float".into(),
            }],
            out_decls: vec![OutDeclInfo {
                index: 0,
                name: "out_0".into(),
                port_type: "signal".into(),
            }],
            comments: vec![],
            messages: vec![],
            wires: vec![
                WireInfo {
                    name: "osc".into(),
                    expr: "cycle~(freq)".into(),
                    attrs: vec![],
                },
                WireInfo {
                    name: "scaled".into(),
                    expr: "mul~(osc, 0.5)".into(),
                    attrs: vec![],
                },
            ],
            out_assignments: vec![OutAssignInfo {
                index: 0,
                wire_name: "scaled".into(),
            }],
            direct_connections: vec![],
            code_files: vec![],
            ui_entries: vec![],
            patcher_rect: None,
            panels: vec![],
            images: vec![],
        };

        let source = emit(&patch);
        assert!(source.contains("in freq: float;"));
        // Inline form: out declaration merged with out assignment
        assert!(source.contains("out out_0: signal = scaled;"));
        assert!(source.contains("wire osc = cycle~(freq);"));
        assert!(source.contains("wire scaled = mul~(osc, 0.5);"));
        // No separate out[0] = ... line
        assert!(!source.contains("out[0]"));
    }

    #[test]
    fn emit_blank_line_separators() {
        let patch = DecompiledPatch {
            in_decls: vec![InDeclInfo {
                index: 0,
                name: "x".into(),
                port_type: "float".into(),
            }],
            out_decls: vec![OutDeclInfo {
                index: 0,
                name: "y".into(),
                port_type: "signal".into(),
            }],
            comments: vec![],
            messages: vec![],
            wires: vec![WireInfo {
                name: "w".into(),
                expr: "add(x, 1)".into(),
                attrs: vec![],
            }],
            out_assignments: vec![OutAssignInfo {
                index: 0,
                wire_name: "w".into(),
            }],
            direct_connections: vec![],
            code_files: vec![],
            ui_entries: vec![],
            patcher_rect: None,
            panels: vec![],
            images: vec![],
        };

        let source = emit(&patch);
        // Inline form: out declaration merged with out assignment
        assert!(source.contains("out y: signal = w;"));
        // There should be a blank line after declarations and before wires
        assert!(source.contains("= w;\n\n"));
        // No separate out[0] = ... line (merged into declaration)
        assert!(!source.contains("out[0]"));
    }

    #[test]
    fn emit_no_declarations() {
        let patch = DecompiledPatch {
            in_decls: vec![],
            out_decls: vec![],
            comments: vec![],
            messages: vec![],
            wires: vec![WireInfo {
                name: "w".into(),
                expr: "cycle~(440)".into(),
                attrs: vec![],
            }],
            out_assignments: vec![],
            direct_connections: vec![],
            code_files: vec![],
            ui_entries: vec![],
            patcher_rect: None,
            panels: vec![],
            images: vec![],
        };

        let source = emit(&patch);
        // Should not start with a blank line
        assert!(source.starts_with("wire"));
    }

    #[test]
    fn emit_with_comments() {
        let patch = DecompiledPatch {
            in_decls: vec![],
            out_decls: vec![],
            comments: vec![
                CommentInfo { text: "This is a comment".into(), rect: [0.0, 10.0, 200.0, 20.0], y_position: 10.0 },
                CommentInfo { text: "Another comment".into(), rect: [0.0, 20.0, 200.0, 20.0], y_position: 20.0 },
            ],
            messages: vec![],
            wires: vec![WireInfo {
                name: "w".into(),
                expr: "cycle~(440)".into(),
                attrs: vec![],
            }],
            out_assignments: vec![],
            direct_connections: vec![],
            code_files: vec![],
            ui_entries: vec![],
            patcher_rect: None,
            panels: vec![],
            images: vec![],
        };

        let source = emit(&patch);
        assert!(source.contains("// This is a comment\n"));
        assert!(source.contains("// Another comment\n"));
        // Comments should appear before wires
        let comment_pos = source.find("// This is a comment").unwrap();
        let wire_pos = source.find("wire w =").unwrap();
        assert!(comment_pos < wire_pos);
    }

    #[test]
    fn emit_with_messages() {
        let patch = DecompiledPatch {
            in_decls: vec![],
            out_decls: vec![],
            comments: vec![],
            messages: vec![
                MsgInfo {
                    name: "click".into(),
                    content: "bang".into(),
                    attrs: vec![],
                },
                MsgInfo {
                    name: "format".into(),
                    content: "set $1 $2".into(),
                    attrs: vec![],
                },
            ],
            wires: vec![WireInfo {
                name: "w".into(),
                expr: "print(click)".into(),
                attrs: vec![],
            }],
            out_assignments: vec![],
            direct_connections: vec![],
            code_files: vec![],
            ui_entries: vec![],
            patcher_rect: None,
            panels: vec![],
            images: vec![],
        };

        let source = emit(&patch);
        assert!(source.contains("msg click = \"bang\";\n"));
        assert!(source.contains("msg format = \"set $1 $2\";\n"));
        // Messages should appear before wires
        let msg_pos = source.find("msg click").unwrap();
        let wire_pos = source.find("wire w =").unwrap();
        assert!(msg_pos < wire_pos);
    }

    #[test]
    fn emit_message_with_special_chars() {
        let patch = DecompiledPatch {
            in_decls: vec![],
            out_decls: vec![],
            comments: vec![],
            messages: vec![MsgInfo {
                name: "msg_1".into(),
                content: "say \"hello\" world".into(),
                attrs: vec![],
            }],
            wires: vec![],
            out_assignments: vec![],
            direct_connections: vec![],
            code_files: vec![],
            ui_entries: vec![],
            patcher_rect: None,
            panels: vec![],
            images: vec![],
        };

        let source = emit(&patch);
        assert!(source.contains(r#"msg msg_1 = "say \"hello\" world";"#));
    }

    const L1_JSON: &str = include_str!("../../../tests/e2e/expected/L1_sine.maxpat");
    const L2_JSON: &str = include_str!("../../../tests/e2e/expected/L2_simple_synth.maxpat");
    const L3B_JSON: &str = include_str!("../../../tests/e2e/expected/L3b_control_fanout.maxpat");

    #[test]
    fn decompile_l1_produces_valid_source() {
        let source = decompile(L1_JSON).unwrap();
        // Should contain wire and out declaration (inline or separate)
        assert!(source.contains("wire"));
        assert!(source.contains("cycle~(440)"));
        // Out declaration: either inline `out name: type = expr;` or separate `out[0] = ...;`
        assert!(source.contains("out "), "should contain out declaration: {}", source);
    }

    #[test]
    fn decompile_l2_produces_valid_source() {
        let source = decompile(L2_JSON).unwrap();
        assert!(source.contains("in "));
        assert!(source.contains("out "));
        assert!(source.contains("cycle~"));
        assert!(source.contains("mul~"));
        assert!(source.contains("0.5"));
    }

    #[test]
    fn decompile_l3b_no_trigger() {
        let source = decompile(L3B_JSON).unwrap();
        // trigger should not appear in the decompiled source
        assert!(
            !source.contains("trigger"),
            "Decompiled source should not contain trigger: {}",
            source
        );
        // Should have in, out, wire declarations
        assert!(source.contains("in "));
        assert!(source.contains("out "));
        assert!(source.contains("wire"));
    }

    // -----------------------------------------------------------------------
    // decompile_multi tests
    // -----------------------------------------------------------------------

    #[test]
    fn decompile_multi_flat_patch() {
        let result = decompile_multi(L1_JSON, "sine").unwrap();
        assert_eq!(result.main_file, "sine.flutmax");
        assert_eq!(result.files.len(), 1, "Flat patch should produce 1 file");
        assert!(result.files.contains_key("sine.flutmax"));
        let source = &result.files["sine.flutmax"];
        assert!(source.contains("cycle~(440)"));
    }

    #[test]
    fn decompile_multi_one_subpatcher() {
        let json = r#"{
            "patcher": {
                "boxes": [
                    { "box": { "id": "obj-1", "maxclass": "newobj", "text": "cycle~ 440", "numinlets": 2, "numoutlets": 1, "outlettype": ["signal"] } },
                    {
                        "box": {
                            "id": "obj-2",
                            "maxclass": "newobj",
                            "text": "p myfilter",
                            "numinlets": 1,
                            "numoutlets": 1,
                            "outlettype": ["signal"],
                            "patcher": {
                                "boxes": [
                                    { "box": { "id": "sub-1", "maxclass": "inlet~", "numinlets": 0, "numoutlets": 1, "outlettype": ["signal"] } },
                                    { "box": { "id": "sub-2", "maxclass": "newobj", "text": "biquad~", "numinlets": 6, "numoutlets": 1, "outlettype": ["signal"] } },
                                    { "box": { "id": "sub-3", "maxclass": "outlet~", "numinlets": 1, "numoutlets": 0, "outlettype": [] } }
                                ],
                                "lines": [
                                    { "patchline": { "source": ["sub-1", 0], "destination": ["sub-2", 0] } },
                                    { "patchline": { "source": ["sub-2", 0], "destination": ["sub-3", 0] } }
                                ]
                            }
                        }
                    },
                    { "box": { "id": "obj-3", "maxclass": "outlet", "numinlets": 1, "numoutlets": 0, "outlettype": [] } }
                ],
                "lines": [
                    { "patchline": { "source": ["obj-1", 0], "destination": ["obj-2", 0] } },
                    { "patchline": { "source": ["obj-2", 0], "destination": ["obj-3", 0] } }
                ]
            }
        }"#;

        let result = decompile_multi(json, "synth").unwrap();
        assert_eq!(result.main_file, "synth.flutmax");
        assert_eq!(result.files.len(), 2, "Should produce 2 files: main + subpatcher");

        // Main file should reference the subpatcher
        let main_source = &result.files["synth.flutmax"];
        assert!(
            main_source.contains("myfilter"),
            "Main should reference subpatcher: {}",
            main_source
        );

        // Subpatcher file should exist
        let sub_file = result.files.keys().find(|k| k.contains("myfilter")).unwrap();
        let sub_source = &result.files[sub_file];
        assert!(
            sub_source.contains("biquad~"),
            "Subpatcher should contain biquad~: {}",
            sub_source
        );
        assert!(sub_source.contains("in "), "Subpatcher should have inlet declaration");
        assert!(sub_source.contains("out "), "Subpatcher should have outlet declaration");
    }

    #[test]
    fn decompile_multi_nested() {
        let json = r#"{
            "patcher": {
                "boxes": [
                    {
                        "box": {
                            "id": "obj-1",
                            "maxclass": "newobj",
                            "text": "p outer",
                            "numinlets": 1,
                            "numoutlets": 1,
                            "outlettype": [""],
                            "patcher": {
                                "boxes": [
                                    { "box": { "id": "sub-1", "maxclass": "inlet", "numinlets": 0, "numoutlets": 1, "outlettype": [""] } },
                                    {
                                        "box": {
                                            "id": "sub-2",
                                            "maxclass": "newobj",
                                            "text": "p inner",
                                            "numinlets": 1,
                                            "numoutlets": 1,
                                            "outlettype": [""],
                                            "patcher": {
                                                "boxes": [
                                                    { "box": { "id": "deep-1", "maxclass": "inlet", "numinlets": 0, "numoutlets": 1, "outlettype": [""] } },
                                                    { "box": { "id": "deep-2", "maxclass": "newobj", "text": "print", "numinlets": 1, "numoutlets": 0, "outlettype": [] } },
                                                    { "box": { "id": "deep-3", "maxclass": "outlet", "numinlets": 1, "numoutlets": 0, "outlettype": [] } }
                                                ],
                                                "lines": [
                                                    { "patchline": { "source": ["deep-1", 0], "destination": ["deep-2", 0] } },
                                                    { "patchline": { "source": ["deep-1", 0], "destination": ["deep-3", 0] } }
                                                ]
                                            }
                                        }
                                    },
                                    { "box": { "id": "sub-3", "maxclass": "outlet", "numinlets": 1, "numoutlets": 0, "outlettype": [] } }
                                ],
                                "lines": [
                                    { "patchline": { "source": ["sub-1", 0], "destination": ["sub-2", 0] } },
                                    { "patchline": { "source": ["sub-2", 0], "destination": ["sub-3", 0] } }
                                ]
                            }
                        }
                    }
                ],
                "lines": []
            }
        }"#;

        let result = decompile_multi(json, "main").unwrap();
        assert_eq!(result.main_file, "main.flutmax");
        // Should produce 3 files: main, outer, inner
        assert_eq!(result.files.len(), 3, "Expected 3 files, got: {:?}", result.files.keys().collect::<Vec<_>>());
    }

    #[test]
    fn decompile_backward_compat_with_subpatchers() {
        // The old decompile() should still work even if patch has subpatchers
        // (it will treat them as regular newobj boxes)
        let json = r#"{
            "patcher": {
                "boxes": [
                    {
                        "box": {
                            "id": "obj-1",
                            "maxclass": "newobj",
                            "text": "p myfilter",
                            "numinlets": 1,
                            "numoutlets": 1,
                            "outlettype": [""],
                            "patcher": {
                                "boxes": [
                                    { "box": { "id": "sub-1", "maxclass": "inlet", "numinlets": 0, "numoutlets": 1, "outlettype": [""] } },
                                    { "box": { "id": "sub-2", "maxclass": "outlet", "numinlets": 1, "numoutlets": 0, "outlettype": [] } }
                                ],
                                "lines": [
                                    { "patchline": { "source": ["sub-1", 0], "destination": ["sub-2", 0] } }
                                ]
                            }
                        }
                    }
                ],
                "lines": []
            }
        }"#;

        // Old API should not panic; it simply treats the subpatcher box as a regular node
        let source = decompile(json).unwrap();
        assert!(source.contains("wire"), "Should produce a wire for the p box: {}", source);
    }

    // -----------------------------------------------------------------------
    // .attr() chain tests
    // -----------------------------------------------------------------------

    #[test]
    fn emit_wire_with_attrs() {
        let patch = DecompiledPatch {
            in_decls: vec![],
            out_decls: vec![],
            comments: vec![],
            messages: vec![],
            wires: vec![WireInfo {
                name: "w_1".into(),
                expr: "flonum()".into(),
                attrs: vec![
                    ("minimum".into(), "0.".into()),
                    ("maximum".into(), "100.".into()),
                ],
            }],
            out_assignments: vec![],
            direct_connections: vec![],
            code_files: vec![],
            ui_entries: vec![],
            patcher_rect: None,
            panels: vec![],
            images: vec![],
        };

        let source = emit(&patch);
        assert!(
            source.contains("wire w_1 = flonum().attr(minimum: 0., maximum: 100.);"),
            "Expected .attr() chain, got: {}",
            source
        );
    }

    #[test]
    fn emit_wire_without_attrs_unchanged() {
        let patch = DecompiledPatch {
            in_decls: vec![],
            out_decls: vec![],
            comments: vec![],
            messages: vec![],
            wires: vec![WireInfo {
                name: "w_1".into(),
                expr: "cycle~(440)".into(),
                attrs: vec![],
            }],
            out_assignments: vec![],
            direct_connections: vec![],
            code_files: vec![],
            ui_entries: vec![],
            patcher_rect: None,
            panels: vec![],
            images: vec![],
        };

        let source = emit(&patch);
        assert!(
            source.contains("wire w_1 = cycle~(440);"),
            "No .attr() should appear: {}",
            source
        );
        assert!(!source.contains(".attr("), "Should not contain .attr(): {}", source);
    }

    #[test]
    fn emit_msg_with_attrs() {
        let patch = DecompiledPatch {
            in_decls: vec![],
            out_decls: vec![],
            comments: vec![],
            messages: vec![MsgInfo {
                name: "click".into(),
                content: "bang".into(),
                attrs: vec![
                    ("some_attr".into(), "\"value\"".into()),
                ],
            }],
            wires: vec![],
            out_assignments: vec![],
            direct_connections: vec![],
            code_files: vec![],
            ui_entries: vec![],
            patcher_rect: None,
            panels: vec![],
            images: vec![],
        };

        let source = emit(&patch);
        assert!(
            source.contains("msg click = \"bang\".attr(some_attr: \"value\");"),
            "Expected .attr() on msg, got: {}",
            source
        );
    }

    #[test]
    fn decompile_flonum_with_attrs() {
        // A flonum box with minimum/maximum attributes in JSON
        let json = r#"{
            "patcher": {
                "boxes": [
                    {
                        "box": {
                            "id": "obj-1",
                            "maxclass": "flonum",
                            "numinlets": 1,
                            "numoutlets": 2,
                            "outlettype": ["", "bang"],
                            "patching_rect": [100, 100, 50, 22],
                            "minimum": 0.0,
                            "maximum": 100.0
                        }
                    },
                    {
                        "box": {
                            "id": "obj-2",
                            "maxclass": "outlet",
                            "numinlets": 1,
                            "numoutlets": 0,
                            "outlettype": []
                        }
                    }
                ],
                "lines": [
                    {
                        "patchline": {
                            "source": ["obj-1", 0],
                            "destination": ["obj-2", 0]
                        }
                    }
                ]
            }
        }"#;

        let source = decompile(json).unwrap();
        assert!(
            source.contains(".attr("),
            "Decompiled flonum should have .attr(): {}",
            source
        );
        assert!(
            source.contains("minimum: 0"),
            "Should contain minimum attr: {}",
            source
        );
        assert!(
            source.contains("maximum: 100"),
            "Should contain maximum attr: {}",
            source
        );
    }

    #[test]
    fn decompile_newobj_with_text_attrs() {
        // A newobj with @phase attribute in text
        let json = r#"{
            "patcher": {
                "boxes": [
                    {
                        "box": {
                            "id": "obj-1",
                            "maxclass": "newobj",
                            "text": "cycle~ 440 @phase 0.5",
                            "numinlets": 2,
                            "numoutlets": 1,
                            "outlettype": ["signal"]
                        }
                    },
                    {
                        "box": {
                            "id": "obj-2",
                            "maxclass": "outlet",
                            "numinlets": 1,
                            "numoutlets": 0,
                            "outlettype": []
                        }
                    }
                ],
                "lines": [
                    {
                        "patchline": {
                            "source": ["obj-1", 0],
                            "destination": ["obj-2", 0]
                        }
                    }
                ]
            }
        }"#;

        let source = decompile(json).unwrap();
        assert!(
            source.contains(".attr("),
            "Decompiled newobj with @phase should have .attr(): {}",
            source
        );
        assert!(
            source.contains("phase: 0.5"),
            "Should contain phase attr: {}",
            source
        );
        // The 440 arg should still be in the expression, not in attrs
        assert!(
            source.contains("440"),
            "Should still contain 440 arg: {}",
            source
        );
    }

    #[test]
    fn decompile_no_attrs_unchanged() {
        // A simple newobj with no attributes - should not have .attr()
        let json = r#"{
            "patcher": {
                "boxes": [
                    {
                        "box": {
                            "id": "obj-1",
                            "maxclass": "newobj",
                            "text": "cycle~ 440",
                            "numinlets": 2,
                            "numoutlets": 1,
                            "outlettype": ["signal"]
                        }
                    },
                    {
                        "box": {
                            "id": "obj-2",
                            "maxclass": "outlet",
                            "numinlets": 1,
                            "numoutlets": 0,
                            "outlettype": []
                        }
                    }
                ],
                "lines": [
                    {
                        "patchline": {
                            "source": ["obj-1", 0],
                            "destination": ["obj-2", 0]
                        }
                    }
                ]
            }
        }"#;

        let source = decompile(json).unwrap();
        assert!(
            !source.contains(".attr("),
            "Simple patch should NOT have .attr(): {}",
            source
        );
    }

    #[test]
    fn decompile_newobj_with_multiple_text_attrs() {
        // A newobj with multiple @ attributes
        let json = r#"{
            "patcher": {
                "boxes": [
                    {
                        "box": {
                            "id": "obj-1",
                            "maxclass": "newobj",
                            "text": "live.dial @parameter_longname Cutoff @minimum 20. @maximum 20000.",
                            "numinlets": 1,
                            "numoutlets": 2,
                            "outlettype": ["", "float"]
                        }
                    },
                    {
                        "box": {
                            "id": "obj-2",
                            "maxclass": "outlet",
                            "numinlets": 1,
                            "numoutlets": 0,
                            "outlettype": []
                        }
                    }
                ],
                "lines": [
                    {
                        "patchline": {
                            "source": ["obj-1", 0],
                            "destination": ["obj-2", 0]
                        }
                    }
                ]
            }
        }"#;

        let source = decompile(json).unwrap();
        assert!(
            source.contains(".attr("),
            "Should have .attr(): {}",
            source
        );
        assert!(
            source.contains("parameter_longname: Cutoff"),
            "Should contain parameter_longname: {}",
            source
        );
        assert!(
            source.contains("minimum: 20."),
            "Should contain minimum: {}",
            source
        );
        assert!(
            source.contains("maximum: 20000."),
            "Should contain maximum: {}",
            source
        );
    }

    // -----------------------------------------------------------------------
    // Codebox tests
    // -----------------------------------------------------------------------

    #[test]
    fn decompile_multi_v8_codebox_produces_code_files() {
        let json = r#"{
            "patcher": {
                "classnamespace": "box",
                "boxes": [
                    {
                        "box": {
                            "id": "obj-1",
                            "maxclass": "v8.codebox",
                            "text": "",
                            "code": "function bang() {\n  outlet(0, 42);\n}\n",
                            "filename": "none",
                            "numinlets": 1,
                            "numoutlets": 1,
                            "outlettype": [""],
                            "patching_rect": [50, 50, 200, 100]
                        }
                    },
                    {
                        "box": {
                            "id": "obj-2",
                            "maxclass": "newobj",
                            "text": "print result",
                            "numinlets": 1,
                            "numoutlets": 0,
                            "outlettype": [],
                            "patching_rect": [50, 200, 80, 22]
                        }
                    }
                ],
                "lines": [
                    {
                        "patchline": {
                            "source": ["obj-1", 0],
                            "destination": ["obj-2", 0]
                        }
                    }
                ]
            }
        }"#;

        let result = decompile_multi(json, "test_codebox").unwrap();

        // Should have 1 .flutmax file
        assert_eq!(result.files.len(), 1);
        assert!(result.files.contains_key("test_codebox.flutmax"));

        // Should have 1 code file (.js)
        assert_eq!(
            result.code_files.len(), 1,
            "Expected 1 code file, got: {:?}",
            result.code_files
        );

        // The code file should be a .js file with the JS content
        let (filename, content) = result.code_files.iter().next().unwrap();
        assert!(filename.ends_with(".js"), "Expected .js extension: {}", filename);
        assert!(
            content.contains("function bang()"),
            "Code file should contain JS code: {}",
            content
        );

        // The .flutmax source should reference the codebox
        let source = &result.files["test_codebox.flutmax"];
        assert!(
            source.contains("v8.codebox("),
            "Source should contain v8.codebox reference: {}",
            source
        );
    }

    #[test]
    fn decompile_single_api_does_not_break_with_codebox() {
        let json = r#"{
            "patcher": {
                "classnamespace": "box",
                "boxes": [
                    {
                        "box": {
                            "id": "obj-1",
                            "maxclass": "v8.codebox",
                            "text": "",
                            "code": "function bang() {\n  outlet(0, 42);\n}\n",
                            "filename": "none",
                            "numinlets": 1,
                            "numoutlets": 1,
                            "outlettype": [""],
                            "patching_rect": [50, 50, 200, 100]
                        }
                    }
                ],
                "lines": []
            }
        }"#;

        // The single-file API should not panic
        let source = decompile(json).unwrap();
        assert!(
            source.contains("v8.codebox("),
            "Decompiled source should contain v8.codebox: {}",
            source
        );
    }

    // -----------------------------------------------------------------------
    // .uiflutmax emit tests
    // -----------------------------------------------------------------------

    #[test]
    fn emit_ui_file_basic() {
        let patch = DecompiledPatch {
            in_decls: vec![],
            out_decls: vec![],
            comments: vec![],
            messages: vec![],
            wires: vec![WireInfo {
                name: "cycle".into(),
                expr: "cycle~(440)".into(),
                attrs: vec![],
            }],
            out_assignments: vec![],
            direct_connections: vec![],
            code_files: vec![],
            ui_entries: vec![UiEntryInfo {
                name: "cycle".into(),
                rect: [100.0, 150.0, 80.0, 22.0],
                decorative_attrs: vec![],
            }],
            patcher_rect: Some([100.0, 100.0, 640.0, 480.0]),
            panels: vec![],
            images: vec![],
        };

        let ui_json = emit_ui_file(&patch);
        assert!(ui_json.is_some(), "Should produce UI file");
        let ui_json = ui_json.unwrap();

        // Parse and verify JSON structure
        let parsed: serde_json::Value = serde_json::from_str(&ui_json).unwrap();
        assert!(parsed["_patcher"]["rect"].is_array());
        assert_eq!(parsed["_patcher"]["rect"][0], 100.0);
        assert!(parsed["cycle"]["rect"].is_array());
        assert_eq!(parsed["cycle"]["rect"][0], 100.0);
        assert_eq!(parsed["cycle"]["rect"][1], 150.0);
    }

    #[test]
    fn emit_ui_file_with_decorative_attrs() {
        let patch = DecompiledPatch {
            in_decls: vec![],
            out_decls: vec![],
            comments: vec![],
            messages: vec![],
            wires: vec![WireInfo {
                name: "biquad".into(),
                expr: "biquad~()".into(),
                attrs: vec![("minimum".into(), "0".into())],
            }],
            out_assignments: vec![],
            direct_connections: vec![],
            code_files: vec![],
            ui_entries: vec![UiEntryInfo {
                name: "biquad".into(),
                rect: [200.0, 250.0, 80.0, 22.0],
                decorative_attrs: vec![
                    ("background".into(), "0".into()),
                    ("bordercolor".into(), "\"0.0 0.0 0.0 1.0\"".into()),
                ],
            }],
            patcher_rect: None,
            panels: vec![],
            images: vec![],
        };

        let ui_json = emit_ui_file(&patch);
        assert!(ui_json.is_some());
        let ui_json = ui_json.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&ui_json).unwrap();

        // Should have biquad entry with rect and decorative attrs
        assert_eq!(parsed["biquad"]["rect"][0], 200.0);
        assert_eq!(parsed["biquad"]["background"], 0);
        assert_eq!(parsed["biquad"]["bordercolor"], "0.0 0.0 0.0 1.0");
        // Should NOT have _patcher (no patcher_rect provided)
        assert!(parsed.get("_patcher").is_none());
    }

    #[test]
    fn emit_ui_file_empty_returns_none() {
        let patch = DecompiledPatch {
            in_decls: vec![],
            out_decls: vec![],
            comments: vec![],
            messages: vec![],
            wires: vec![],
            out_assignments: vec![],
            direct_connections: vec![],
            code_files: vec![],
            ui_entries: vec![],
            patcher_rect: None,
            panels: vec![],
            images: vec![],
        };

        let ui_json = emit_ui_file(&patch);
        assert!(ui_json.is_none(), "Empty patch should not produce UI file");
    }

    #[test]
    fn decompile_flonum_decorative_not_in_flutmax_source() {
        // A flonum with decorative attrs (bgcolor) should NOT have them in .flutmax output
        let json = r#"{
            "patcher": {
                "boxes": [
                    {
                        "box": {
                            "id": "obj-1",
                            "maxclass": "flonum",
                            "numinlets": 1,
                            "numoutlets": 2,
                            "outlettype": ["", "bang"],
                            "patching_rect": [100, 100, 50, 22],
                            "minimum": 0.0,
                            "maximum": 100.0,
                            "bgcolor": [0.3, 0.3, 0.3, 1.0],
                            "textcolor": [1.0, 1.0, 1.0, 1.0]
                        }
                    },
                    {
                        "box": {
                            "id": "obj-2",
                            "maxclass": "outlet",
                            "numinlets": 1,
                            "numoutlets": 0,
                            "outlettype": []
                        }
                    }
                ],
                "lines": [
                    {
                        "patchline": {
                            "source": ["obj-1", 0],
                            "destination": ["obj-2", 0]
                        }
                    }
                ]
            }
        }"#;

        let source = decompile(json).unwrap();
        // Functional attrs should still be present
        assert!(
            source.contains("minimum"),
            "Should still contain functional attr minimum: {}",
            source
        );
        // Decorative attrs should NOT be in .flutmax source
        assert!(
            !source.contains("bgcolor"),
            "Should NOT contain decorative attr bgcolor in .flutmax: {}",
            source
        );
        assert!(
            !source.contains("textcolor"),
            "Should NOT contain decorative attr textcolor in .flutmax: {}",
            source
        );
    }

    // -----------------------------------------------------------------------
    // Wire+connection grouping tests (E54)
    // -----------------------------------------------------------------------

    #[test]
    fn emit_wire_grouped_with_connections() {
        use crate::analyzer::DirectConnectionInfo;

        let patch = DecompiledPatch {
            in_decls: vec![],
            out_decls: vec![],
            comments: vec![],
            messages: vec![],
            wires: vec![
                WireInfo { name: "a".into(), expr: "cycle~(440)".into(), attrs: vec![] },
                WireInfo { name: "b".into(), expr: "gain~(a)".into(), attrs: vec![] },
            ],
            out_assignments: vec![],
            direct_connections: vec![
                DirectConnectionInfo {
                    target_wire: "b".into(),
                    inlet: 1,
                    source_wire: "a".into(),
                },
            ],
            code_files: vec![],
            ui_entries: vec![],
            patcher_rect: None,
            panels: vec![],
            images: vec![],
        };

        let source = emit(&patch);
        // The connection b.in[1] = a should appear right after wire b, not after all wires
        let wire_b_pos = source.find("wire b = gain~(a);").unwrap();
        let conn_pos = source.find("b.in[1] = a;").unwrap();
        assert!(
            conn_pos > wire_b_pos,
            "Connection should appear after its wire: {}",
            source
        );
        // And there should be no blank line between wire b and its connection
        let between = &source[wire_b_pos..conn_pos];
        assert!(
            !between.contains("\n\n"),
            "No blank line between wire and its connection: {:?}",
            between
        );
    }

    #[test]
    fn emit_comment_proximity_placement() {
        let patch = DecompiledPatch {
            in_decls: vec![],
            out_decls: vec![],
            comments: vec![
                CommentInfo { text: "oscillator section".into(), rect: [100.0, 95.0, 200.0, 20.0], y_position: 95.0 },
                CommentInfo { text: "gain section".into(), rect: [100.0, 195.0, 200.0, 20.0], y_position: 195.0 },
            ],
            messages: vec![],
            wires: vec![
                WireInfo { name: "osc".into(), expr: "cycle~(440)".into(), attrs: vec![] },
                WireInfo { name: "gain".into(), expr: "mul~(osc, 0.5)".into(), attrs: vec![] },
            ],
            out_assignments: vec![],
            direct_connections: vec![],
            code_files: vec![],
            ui_entries: vec![
                UiEntryInfo { name: "osc".into(), rect: [100.0, 100.0, 80.0, 22.0], decorative_attrs: vec![] },
                UiEntryInfo { name: "gain".into(), rect: [100.0, 200.0, 80.0, 22.0], decorative_attrs: vec![] },
            ],
            patcher_rect: None,
            panels: vec![],
            images: vec![],
        };

        let source = emit(&patch);
        // "oscillator section" comment should appear before wire osc
        let osc_comment_pos = source.find("// oscillator section").unwrap();
        let osc_wire_pos = source.find("wire osc =").unwrap();
        assert!(osc_comment_pos < osc_wire_pos, "Comment should be before its wire: {}", source);

        // "gain section" comment should appear before wire gain
        let gain_comment_pos = source.find("// gain section").unwrap();
        let gain_wire_pos = source.find("wire gain =").unwrap();
        assert!(gain_comment_pos < gain_wire_pos, "Comment should be before its wire: {}", source);

        // "gain section" should appear after "wire osc" (not at the top)
        assert!(gain_comment_pos > osc_wire_pos, "Gain comment should be after osc wire: {}", source);
    }

    #[test]
    fn emit_block_separation() {
        let patch = DecompiledPatch {
            in_decls: vec![],
            out_decls: vec![],
            comments: vec![],
            messages: vec![],
            wires: vec![
                // Group 1: connected chain
                WireInfo { name: "osc".into(), expr: "cycle~(440)".into(), attrs: vec![] },
                WireInfo { name: "gain".into(), expr: "mul~(osc, 0.5)".into(), attrs: vec![] },
                // Group 2: independent chain (doesn't reference osc or gain)
                WireInfo { name: "lfo".into(), expr: "cycle~(1)".into(), attrs: vec![] },
                WireInfo { name: "depth".into(), expr: "mul~(lfo, 100)".into(), attrs: vec![] },
            ],
            out_assignments: vec![],
            direct_connections: vec![],
            code_files: vec![],
            ui_entries: vec![],
            patcher_rect: None,
            panels: vec![],
            images: vec![],
        };

        let source = emit(&patch);
        // Between gain and lfo there should be an extra blank line (independent groups)
        let gain_pos = source.find("wire gain =").unwrap();
        let lfo_pos = source.find("wire lfo =").unwrap();
        let between = &source[gain_pos..lfo_pos];
        // Should have double newline (wire gain line + blank line)
        assert!(
            between.contains("\n\n"),
            "Should have blank line between independent groups: {:?}",
            between
        );
    }

}
