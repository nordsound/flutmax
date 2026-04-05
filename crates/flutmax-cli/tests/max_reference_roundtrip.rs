//! Max reference patch roundtrip test suite.
//!
//! Tests all .maxpat files from the Max.app installation.
//! Each patch is: decompile -> compile -> logical graph comparison.
//!
//! Result categories:
//! - PASS: roundtrip produces matching logical graph
//! - SKIP: decompile unsupported (invalid JSON, read errors, unsupported features)
//! - COMPILE_FAIL: decompile succeeds but compile fails (unsupported language features)
//! - MISMATCH: both succeed but logical graphs differ (these are real bugs)
//!
//! The test asserts that MISMATCH == 0. COMPILE_FAIL is reported but does not
//! fail the test — those represent features not yet supported in the compiler.
//!
//! Wave 3: Patches containing subpatchers are now processed via `decompile_multi()`,
//! which produces multiple .flutmax files. The main file is compiled with an
//! AbstractionRegistry populated from the subpatcher files.

use std::collections::{BTreeMap, BTreeSet, HashMap};

// ---------------------------------------------------------------------------
// Result type
// ---------------------------------------------------------------------------

#[allow(dead_code)]
enum RoundtripResult {
    Pass,
    Skip(String),
    CompileFail(String),
    Mismatch(String),
}

// ---------------------------------------------------------------------------
// Logical graph types (same as roundtrip.rs — duplicated because test modules
// cannot share code as library items)
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
struct LogicalGraph {
    nodes: BTreeSet<LogicalNode>,
    edges: BTreeSet<LogicalEdge>,
}

/// Strip disambiguation suffix `#N` from a node text, returning the base name.
fn strip_disambiguation(text: &str) -> &str {
    // Find the last '#' followed by only digits
    if let Some(pos) = text.rfind('#') {
        let suffix = &text[pos + 1..];
        if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
            return &text[..pos];
        }
    }
    text
}

impl LogicalGraph {
    /// Compare two logical graphs with tolerance for disambiguation ordering.
    ///
    /// When nodes have the same base text (e.g., `gain~#0` and `gain~#1`),
    /// the specific `#N` assignment may differ between original and regenerated
    /// because the compiler's auto-layout produces different coordinates.
    /// This comparison strips `#N` suffixes and compares multisets of:
    /// - node base texts (with counts)
    /// - edge base texts (source_base:outlet -> dest_base:inlet)
    fn eq_tolerant(&self, other: &LogicalGraph) -> bool {
        // Compare node multisets (stripping disambiguation)
        let self_node_counts = Self::node_base_counts(&self.nodes);
        let other_node_counts = Self::node_base_counts(&other.nodes);
        if self_node_counts != other_node_counts {
            return false;
        }

        // Compare edge multisets (stripping disambiguation from references)
        let self_edge_counts = Self::edge_base_counts(&self.edges);
        let other_edge_counts = Self::edge_base_counts(&other.edges);
        self_edge_counts == other_edge_counts
    }

    fn node_base_counts(nodes: &BTreeSet<LogicalNode>) -> BTreeMap<String, usize> {
        let mut counts = BTreeMap::new();
        for node in nodes {
            let base = strip_disambiguation(&node.text).to_string();
            *counts.entry(base).or_insert(0) += 1;
        }
        counts
    }

    fn edge_base_counts(
        edges: &BTreeSet<LogicalEdge>,
    ) -> BTreeMap<(String, u32, String, u32), usize> {
        let mut counts = BTreeMap::new();
        for edge in edges {
            let key = (
                strip_disambiguation(&edge.source_text).to_string(),
                edge.source_outlet,
                strip_disambiguation(&edge.dest_text).to_string(),
                edge.dest_inlet,
            );
            *counts.entry(key).or_insert(0) += 1;
        }
        counts
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
struct LogicalNode {
    maxclass: String,
    text: String,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
struct LogicalEdge {
    source_text: String,
    source_outlet: u32,
    dest_text: String,
    dest_inlet: u32,
}

// ---------------------------------------------------------------------------
// Text normalization (mirrors decompiler's transformations)
// ---------------------------------------------------------------------------

/// Normalize Max object text for logical graph comparison.
///
/// This mirrors the transformations the decompiler applies:
/// 1. Strip @attributes (`param foobar @min 0` → `param foobar`)
/// 2. Normalize trailing-dot floats (`127.` → `127`)
/// 3. Split operator-number fusions (`/2` → `/ 2`)
/// 4. Strip trailing zero-valued default arguments (`*~ 0` → `*~`, `+ 0` → `+`)
/// Split text by whitespace, keeping quoted strings intact.
fn split_respecting_quotes(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in text.chars() {
        if ch == '"' {
            in_quotes = !in_quotes;
            current.push(ch);
        } else if ch.is_whitespace() && !in_quotes {
            if !current.is_empty() {
                parts.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

fn normalize_object_text(text: &str) -> String {
    let parts = split_respecting_quotes(text);
    if parts.is_empty() {
        return text.to_string();
    }

    let mut result_parts: Vec<String> = Vec::new();

    // First part: possibly split operator-number fusion
    let first = &parts[0];
    let (name, extra) = split_op_num(first);

    // Normalize object names that the flutmax grammar cannot express.
    // The decompiler falls back to `newobj` for these; the test must match.
    //
    // 1. Pure numeric names: `1`, `44100`, `1.1666` (integer/float constants)
    // 2. Digit-starting names: `2input-router` (valid Max abstractions, not in grammar)
    // 3. Non-tilde dotted identifiers with operator segments: `jit.*`, `jit.-`
    //    (flutmax grammar only allows operator segments in tilde identifiers like mc.+~)
    let name = if name
        .chars()
        .next()
        .map_or(false, |c| c.is_ascii_digit() || c == '-')
        && name
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == '-' || c == 'e' || c == 'E')
    {
        // Pure numeric object name — treat as a literal arg of newobj
        result_parts.push("newobj".to_string());
        result_parts.push(normalize_trailing_dot(&name));
        name
    } else if name.chars().next().map_or(false, |c| c.is_ascii_digit()) {
        // Digit-starting non-numeric name (e.g., `2input-router`) — fallback to newobj
        result_parts.push("newobj".to_string());
        name
    } else if name.starts_with('#') || name.starts_with('$') {
        // Template-prefixed name (e.g., `#2Controls`) — fallback to newobj
        result_parts.push("newobj".to_string());
        name
    } else if name.starts_with('"') {
        // Quoted object name (e.g., `"DSP Status"`) — fallback to newobj
        result_parts.push("newobj".to_string());
        name
    } else if !name.ends_with('~') && name.contains('.') {
        // Non-tilde dotted identifier: check for operator segments
        let has_operator_segment = name.split('.').skip(1).any(|seg| {
            seg.chars()
                .next()
                .map_or(false, |c| "*/%!<>=+-&|^".contains(c))
        });
        if has_operator_segment {
            // e.g., jit.*, jit.- — fallback to newobj
            result_parts.push("newobj".to_string());
            name
        } else {
            result_parts.push(name.clone());
            name
        }
    } else {
        result_parts.push(name.clone());
        name
    };
    let _ = name; // suppress unused warning

    // Normalize trigger abbreviation: `t` → `trigger` so that
    // original `t b i` matches recompiled `trigger b i`.
    if result_parts.last().map(|s| s.as_str()) == Some("t") {
        *result_parts.last_mut().unwrap() = "trigger".to_string();
    }

    if let Some(arg) = extra {
        result_parts.push(normalize_trailing_dot(&arg));
    }

    // Remaining parts: stop at @, normalize trailing-dot floats
    for p in &parts[1..] {
        if p.starts_with('@') {
            break;
        }
        // Strip surrounding quotes from quoted args (e.g., "IAC Driver Bus 1" → IAC Driver Bus 1)
        // Both decompiler and codegen may handle quotes differently, but the
        // semantic content is the same.
        let stripped = if p.starts_with('"') && p.ends_with('"') && p.len() >= 2 {
            &p[1..p.len() - 1]
        } else {
            p.as_str()
        };
        result_parts.push(normalize_trailing_dot(stripped));
    }

    // Strip trailing zero-valued default arguments.
    // Many Max objects default unspecified arguments to 0. The decompiler
    // may add explicit `0` placeholders for unconnected inlets, creating
    // text like `*~ 0` where the original was just `*~`. Both are
    // functionally equivalent, so we normalize by removing trailing zeros.
    while result_parts.len() > 1 {
        let last = result_parts.last().unwrap();
        if last == "0" || last == "0.0" || last == "0." {
            result_parts.pop();
        } else {
            break;
        }
    }

    result_parts.join(" ")
}

/// Split operator-number fusion: "/2" → ("/", Some("2"))
fn split_op_num(token: &str) -> (String, Option<String>) {
    let op_chars = |c: char| "*/%+-!<>=&|^".contains(c);
    let chars: Vec<char> = token.chars().collect();
    if chars.is_empty() || !op_chars(chars[0]) {
        return (token.to_string(), None);
    }
    let mut op_end = 0;
    for (i, &c) in chars.iter().enumerate() {
        if op_chars(c) || c == '~' {
            op_end = i + 1;
        } else {
            break;
        }
    }
    if op_end >= chars.len() {
        return (token.to_string(), None);
    }
    let rest_start = chars[op_end];
    if rest_start.is_ascii_digit() || rest_start == '.' || rest_start == '-' {
        let op: String = chars[..op_end].iter().collect();
        let arg: String = chars[op_end..].iter().collect();
        (op, Some(arg))
    } else {
        (token.to_string(), None)
    }
}

/// Normalize trailing-dot floats: "127." → "127", "0.5" → "0.5"
fn normalize_trailing_dot(s: &str) -> String {
    if s.ends_with('.') && s.len() > 1 {
        let prefix = &s[..s.len() - 1];
        let check = prefix.trim_start_matches('-');
        if !check.is_empty() && check.chars().all(|c| c.is_ascii_digit()) {
            return prefix.to_string();
        }
    }
    s.to_string()
}

// ---------------------------------------------------------------------------
// Logical graph extraction
// ---------------------------------------------------------------------------

/// Check if an object text represents a trigger/t node.
fn is_trigger_node(text: &str) -> bool {
    let first_word = text.split_whitespace().next().unwrap_or("");
    // Strip disambiguation suffix (#N) before checking, since duplicated trigger
    // nodes get texts like "t#0", "t#1" which should still be recognized.
    let base = strip_disambiguation(first_word);
    base == "trigger" || base == "t"
}

/// Check if a trigger node is value-preserving (all outlets use non-bang types).
///
/// Value-preserving triggers (`t f f`, `t i f`, `t l a`) can be normalized
/// away without semantic loss — the input value passes through each outlet
/// with at most a type coercion.
///
/// Triggers with any `b` (bang) outlet destroy the input value and replace
/// it with a bang. These are semantically meaningful and must be kept in
/// the logical graph so that differences are detected.
fn is_value_preserving_trigger(text: &str) -> bool {
    if !is_trigger_node(text) {
        return false;
    }
    let parts: Vec<&str> = text.split_whitespace().collect();
    // Strip disambiguation from object name
    let base_name = strip_disambiguation(parts[0]);
    let _ = base_name;
    // No arguments → trigger defaults to single `b` outlet → not value-preserving
    if parts.len() <= 1 {
        return false;
    }
    // All argument types must be value-preserving (non-bang)
    parts[1..].iter().all(|arg| {
        let base = strip_disambiguation(arg);
        matches!(base, "f" | "i" | "l" | "a" | "s")
    })
}

/// Check that `code` fields from the original .maxpat are preserved in the
/// regenerated output. Returns a mismatch description if code is lost.
fn check_code_field_preservation(orig_json: &str, regen_json: &str) -> Option<String> {
    let orig: serde_json::Value = serde_json::from_str(orig_json).ok()?;
    let regen: serde_json::Value = serde_json::from_str(regen_json).ok()?;

    // Collect code fields from original (top-level patcher only)
    let orig_codes = extract_code_fields(&orig);
    if orig_codes.is_empty() {
        return None; // No codebox in original — nothing to check
    }

    let regen_codes = extract_code_fields(&regen);

    // Every original code field should have a matching entry in the regenerated output.
    // Match by maxclass (v8.codebox / codebox).
    let mut missing = Vec::new();
    for (maxclass, code) in &orig_codes {
        let found = regen_codes
            .iter()
            .any(|(mc, rc)| mc == maxclass && rc == code);
        if !found {
            let preview: String = code.chars().take(60).collect();
            missing.push(format!("{} code lost: \"{}...\"", maxclass, preview));
        }
    }

    if missing.is_empty() {
        None
    } else {
        Some(format!("code field not preserved: {}", missing.join("; ")))
    }
}

/// Extract (maxclass, code) pairs from top-level patcher boxes.
fn extract_code_fields(root: &serde_json::Value) -> Vec<(String, String)> {
    let mut result = Vec::new();
    let boxes = match root.pointer("/patcher/boxes").and_then(|b| b.as_array()) {
        Some(b) => b,
        None => return result,
    };
    for bw in boxes {
        let b = &bw["box"];
        let maxclass = b["maxclass"].as_str().unwrap_or("");
        if let Some(code) = b.get("code").and_then(|c| c.as_str()) {
            if !code.is_empty() {
                result.push((maxclass.to_string(), code.to_string()));
            }
        }
    }
    result
}

/// Check if a varname can survive the decompile->compile roundtrip unchanged.
///
/// A varname is roundtrippable if:
/// 1. It's a valid flutmax plain identifier (letters, digits, underscores, hyphens)
/// 2. It's not a reserved keyword (which gets w_ prefixed by sanitize_name)
/// 3. It starts with a letter or underscore (not a digit or hyphen)
///
/// Varnames on comment boxes are also excluded since comments don't produce wires.
fn is_roundtrippable_varname(vn: &str) -> bool {
    if vn.is_empty() {
        return false;
    }
    let first = vn.chars().next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    if !vn
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return false;
    }
    // Reserved keywords get w_ prefix during sanitization
    if matches!(
        vn,
        "wire"
            | "in"
            | "out"
            | "feedback"
            | "state"
            | "msg"
            | "signal"
            | "float"
            | "int"
            | "bang"
            | "list"
            | "symbol"
    ) {
        return false;
    }
    true
}

/// Check that important attributes are preserved in regenerated output.
///
/// Compares varnames between original and regenerated .maxpat.
/// Only checks varnames that are valid flutmax identifiers (ones that should
/// survive the text roundtrip without sanitization loss).
/// Returns a mismatch description if important attributes are lost.
fn check_important_attrs_preserved(orig_json: &str, regen_json: &str) -> Option<String> {
    let orig: serde_json::Value = serde_json::from_str(orig_json).ok()?;
    let regen: serde_json::Value = serde_json::from_str(regen_json).ok()?;

    let orig_boxes = orig.pointer("/patcher/boxes")?.as_array()?;
    let regen_boxes = regen.pointer("/patcher/boxes")?.as_array()?;

    // Check varnames: all original roundtrippable varnames should appear in
    // regenerated output. Not 1:1 matching because box order may differ.
    // Exclude comment, inlet, outlet, and visual-only boxes:
    // - Comment boxes don't produce wires; their varnames are naturally lost.
    // - Inlet/outlet boxes become in/out declarations; their varnames are
    //   replaced by port names derived from the comment field.
    // - Panel/fpic/swatch boxes are visual-only decorations stored in .uiflutmax;
    //   their varnames are not preserved in the logic roundtrip.
    let excluded_classes = ["comment", "inlet", "outlet", "panel", "fpic", "swatch"];
    let orig_varnames: Vec<&str> = orig_boxes
        .iter()
        .filter(|bw| {
            let mc = bw["box"]["maxclass"].as_str().unwrap_or("");
            !excluded_classes.contains(&mc)
        })
        .filter_map(|bw| bw["box"]["varname"].as_str())
        .filter(|vn| is_roundtrippable_varname(vn))
        .collect();
    let regen_varnames: Vec<&str> = regen_boxes
        .iter()
        .filter_map(|bw| bw["box"]["varname"].as_str())
        .collect();

    let mut missing_varnames = Vec::new();
    for vn in &orig_varnames {
        if !regen_varnames.contains(vn) {
            missing_varnames.push(*vn);
        }
    }

    if !missing_varnames.is_empty() {
        return Some(format!("varname lost: {:?}", missing_varnames));
    }

    None
}

/// Find trigger nodes whose ALL outgoing connections go to signal objects.
/// These triggers are unnecessary — signal objects process at DSP rate and
/// don't need control-rate ordering.
fn find_unnecessary_triggers(maxpat_json: &str) -> Vec<String> {
    let root: serde_json::Value = match serde_json::from_str(maxpat_json) {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    let boxes = match root.pointer("/patcher/boxes").and_then(|b| b.as_array()) {
        Some(b) => b,
        None => return vec![],
    };
    let lines = match root.pointer("/patcher/lines").and_then(|l| l.as_array()) {
        Some(l) => l,
        None => return vec![],
    };

    // Build id → text map and identify trigger IDs
    let mut id_to_text: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut trigger_ids: Vec<String> = Vec::new();
    for bw in boxes {
        let b = &bw["box"];
        let id = b["id"].as_str().unwrap_or("").to_string();
        let maxclass = b["maxclass"].as_str().unwrap_or("");
        let text = b["text"].as_str().unwrap_or("").to_string();
        if maxclass == "newobj" {
            let first = text.split_whitespace().next().unwrap_or("");
            if first == "trigger" || first == "t" {
                trigger_ids.push(id.clone());
            }
        }
        id_to_text.insert(id, text);
    }

    // For each trigger, check if ALL destinations are signal objects
    let mut unnecessary = Vec::new();
    for tid in &trigger_ids {
        let mut all_signal = true;
        let mut has_dest = false;
        for lw in lines {
            let pl = &lw["patchline"];
            let src_id = pl["source"]
                .as_array()
                .and_then(|a| a[0].as_str())
                .unwrap_or("");
            if src_id != tid {
                continue;
            }
            has_dest = true;
            let dst_id = pl["destination"]
                .as_array()
                .and_then(|a| a[0].as_str())
                .unwrap_or("");
            let dst_text = id_to_text.get(dst_id).map(|s| s.as_str()).unwrap_or("");
            let dst_name = dst_text.split_whitespace().next().unwrap_or("");
            if !dst_name.ends_with('~') {
                all_signal = false;
                break;
            }
        }
        if has_dest && all_signal {
            let text = id_to_text.get(tid).map(|s| s.as_str()).unwrap_or("trigger");
            // Only flag value-preserving triggers (auto-inserted by compiler).
            // Non-standard triggers like `t 5` or `t gettime` are intentional.
            let normalized = normalize_object_text(text);
            if is_value_preserving_trigger(&normalized) {
                unnecessary.push(text.to_string());
            }
        }
    }
    unnecessary
}

fn extract_logical_graph(maxpat_json: &str) -> LogicalGraph {
    let root: serde_json::Value =
        serde_json::from_str(maxpat_json).expect("failed to parse .maxpat JSON");

    let patcher = &root["patcher"];
    let boxes = patcher["boxes"].as_array().expect("missing boxes array");
    let lines = patcher["lines"].as_array().expect("missing lines array");

    // First pass: compute the raw text for each box and count occurrences.
    // Skip comment boxes — they are non-functional (display only) and the
    // decompiler emits them as flutmax comments, so they don't roundtrip.
    //
    // Also extract patching_rect for deterministic disambiguation ordering.
    // (id, maxclass, raw_text, y_coord, x_coord)
    let mut raw_texts: Vec<(String, String, String, f64, f64)> = Vec::new();
    let mut text_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut comment_ids: BTreeSet<String> = BTreeSet::new();

    for box_wrapper in boxes {
        let b = &box_wrapper["box"];
        let id = b["id"].as_str().expect("box missing id").to_string();
        let maxclass = b["maxclass"]
            .as_str()
            .expect("box missing maxclass")
            .to_string();

        // Exclude visual-only boxes from the logical graph.
        // Comments are emitted as // lines in .flutmax; panels and fpic
        // are visual-only decorations stored in .uiflutmax (when unconnected).
        // Note: swatch is NOT excluded here because it can be functional (color picker
        // with connections). The decompiler only filters unconnected visual boxes.
        if matches!(maxclass.as_str(), "comment" | "panel" | "fpic") {
            comment_ids.insert(id);
            continue;
        }

        let raw_text = if maxclass == "newobj" {
            let full_text = match b["text"].as_str() {
                Some(t) => t.to_string(),
                None => {
                    // Some newobj boxes lack a text field (corrupt or placeholder);
                    // use maxclass as fallback instead of panicking.
                    maxclass.clone()
                }
            };
            normalize_object_text(&full_text)
        } else {
            // For non-newobj boxes, use just the maxclass.
            // Don't include `comment` field — it's display metadata (port labels
            // on inlet/outlet, button labels on textbutton, etc.) that the
            // decompiler doesn't reliably roundtrip. The functional identity
            // of the box is its maxclass.
            maxclass.clone()
        };

        // Extract patching_rect [x, y, w, h] for deterministic ordering
        let (x, y) = if let Some(rect) = b.get("patching_rect").and_then(|r| r.as_array()) {
            (
                rect.first().and_then(|v| v.as_f64()).unwrap_or(0.0),
                rect.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0),
            )
        } else {
            (0.0, 0.0)
        };

        *text_counts.entry(raw_text.clone()).or_insert(0) += 1;
        raw_texts.push((id, maxclass, raw_text, y, x));
    }

    // Second pass: assign disambiguated text for duplicates.
    // Use sequential numbering — the specific #N assignment doesn't need to
    // match between original and regenerated because the final comparison
    // uses disambiguation-tolerant matching (see LogicalGraph::eq_tolerant).
    let mut id_to_node: HashMap<String, LogicalNode> = HashMap::new();
    let mut dup_counters: HashMap<String, usize> = HashMap::new();

    for (id, maxclass, raw_text, _y, _x) in &raw_texts {
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

    // Build edges
    let mut edges: BTreeSet<LogicalEdge> = BTreeSet::new();

    // Track value-preserving trigger IDs for normalization.
    // Only triggers where ALL outlets are non-bang types (f/i/l/a/s) are
    // normalized away. Triggers with bang outlets (e.g., `t b b`) destroy
    // input values and must remain in the graph for semantic comparison.
    let trigger_ids: BTreeSet<String> = id_to_node
        .iter()
        .filter(|(_, node)| is_value_preserving_trigger(&node.text))
        .map(|(id, _)| id.clone())
        .collect();

    // Build raw edges first, then normalize triggers out
    // trigger_incoming: trigger_id -> Vec<(source_id, source_outlet)> — what feeds into inlet 0
    // A trigger can have multiple sources (fanin on inlet 0), all of which reach
    // all trigger outputs. Using Vec instead of single tuple ensures all sources
    // are reconnected through the trigger.
    let mut trigger_incoming: HashMap<String, Vec<(String, u32)>> = HashMap::new();
    // trigger_outgoing: (trigger_id, outlet) -> Vec<(dest_id, dest_inlet)>
    let mut trigger_outgoing: HashMap<(String, u32), Vec<(String, u32)>> = HashMap::new();

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

        // Skip edges involving unknown boxes (could happen if box was filtered)
        if !id_to_node.contains_key(source_id) || !id_to_node.contains_key(dest_id) {
            continue;
        }

        // Track trigger connections for normalization
        if trigger_ids.contains(dest_id) && dest_inlet == 0 {
            trigger_incoming
                .entry(dest_id.to_string())
                .or_default()
                .push((source_id.to_string(), source_outlet));
        }
        if trigger_ids.contains(source_id) {
            trigger_outgoing
                .entry((source_id.to_string(), source_outlet))
                .or_default()
                .push((dest_id.to_string(), dest_inlet));
        }

        // Only add non-trigger edges directly
        if !trigger_ids.contains(source_id) && !trigger_ids.contains(dest_id) {
            let source_node = &id_to_node[source_id];
            let dest_node = &id_to_node[dest_id];
            edges.insert(LogicalEdge {
                source_text: source_node.text.clone(),
                source_outlet,
                dest_text: dest_node.text.clone(),
                dest_inlet,
            });
        }
    }

    // Reconnect through trigger nodes, handling chained triggers.
    // For each trigger, resolve ultimate non-trigger sources (backward) and
    // ultimate non-trigger destinations (forward through trigger chains).

    // Resolve ultimate non-trigger sources for a trigger (tracing backward).
    fn resolve_sources(
        trigger_id: &str,
        trigger_incoming: &HashMap<String, Vec<(String, u32)>>,
        trigger_ids: &BTreeSet<String>,
        depth: usize,
    ) -> Vec<(String, u32)> {
        if depth > 20 {
            return Vec::new();
        }
        let mut results = Vec::new();
        if let Some(sources) = trigger_incoming.get(trigger_id) {
            for (src_id, src_outlet) in sources {
                if trigger_ids.contains(src_id) {
                    results.extend(resolve_sources(
                        src_id,
                        trigger_incoming,
                        trigger_ids,
                        depth + 1,
                    ));
                } else {
                    results.push((src_id.clone(), *src_outlet));
                }
            }
        }
        results
    }

    // Resolve ultimate non-trigger destinations for a trigger (tracing forward).
    fn resolve_dests(
        trigger_id: &str,
        trigger_outgoing: &HashMap<(String, u32), Vec<(String, u32)>>,
        trigger_ids: &BTreeSet<String>,
        depth: usize,
    ) -> Vec<(String, u32)> {
        if depth > 20 {
            return Vec::new();
        }
        let mut results = Vec::new();
        for ((tid, _outlet), dests) in trigger_outgoing {
            if tid != trigger_id {
                continue;
            }
            for (dest_id, dest_inlet) in dests {
                if trigger_ids.contains(dest_id) {
                    results.extend(resolve_dests(
                        dest_id,
                        trigger_outgoing,
                        trigger_ids,
                        depth + 1,
                    ));
                } else {
                    results.push((dest_id.clone(), *dest_inlet));
                }
            }
        }
        results
    }

    // For each trigger, connect all ultimate sources to all ultimate destinations.
    let mut processed_triggers = BTreeSet::new();
    for trigger_id in &trigger_ids {
        if processed_triggers.contains(trigger_id) {
            continue;
        }
        processed_triggers.insert(trigger_id.clone());

        let sources = resolve_sources(trigger_id, &trigger_incoming, &trigger_ids, 0);
        let dests = resolve_dests(trigger_id, &trigger_outgoing, &trigger_ids, 0);

        for (src_id, src_outlet) in &sources {
            let source_node = &id_to_node[src_id.as_str()];
            for (dest_id, dest_inlet) in &dests {
                let dest_node = &id_to_node[dest_id.as_str()];
                edges.insert(LogicalEdge {
                    source_text: source_node.text.clone(),
                    source_outlet: *src_outlet,
                    dest_text: dest_node.text.clone(),
                    dest_inlet: *dest_inlet,
                });
            }
        }
    }

    // Remove only value-preserving trigger nodes from the node set.
    // Bang-containing triggers remain as regular nodes.
    let nodes: BTreeSet<LogicalNode> = id_to_node
        .values()
        .filter(|n| !is_value_preserving_trigger(&n.text))
        .cloned()
        .collect();

    LogicalGraph { nodes, edges }
}

// ---------------------------------------------------------------------------
// File discovery
// ---------------------------------------------------------------------------

fn find_all_maxpat_files(dir: &str) -> Vec<String> {
    let mut files = Vec::new();
    collect_maxpat_files(std::path::Path::new(dir), &mut files);
    files.sort();
    files
}

fn collect_maxpat_files(dir: &std::path::Path, files: &mut Vec<String>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_maxpat_files(&path, files);
            } else if path.extension().and_then(|e| e.to_str()) == Some("maxpat") {
                files.push(path.to_string_lossy().into_owned());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Subpatcher detection
// ---------------------------------------------------------------------------

/// Check if a .maxpat JSON contains subpatchers or other features that
/// make it unsuitable for roundtrip testing.
fn has_subpatchers(json_str: &str) -> bool {
    let value: serde_json::Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(_) => return true, // Skip invalid JSON
    };

    let boxes = match value.pointer("/patcher/boxes") {
        Some(b) => b.as_array(),
        None => return true,
    };

    if let Some(boxes) = boxes {
        for item in boxes {
            let box_obj = &item["box"];

            // Check for embedded subpatcher
            if box_obj.get("patcher").is_some() {
                return true;
            }

            // Check for objects that contain subpatchers
            if let Some(text) = box_obj["text"].as_str() {
                let first_word = text.split_whitespace().next().unwrap_or("");
                if matches!(
                    first_word,
                    "p" | "patcher"
                        | "poly~"
                        | "pfft~"
                        | "gen~"
                        | "jit.gen"
                        | "jit.pix"
                        | "vst~"
                        | "amxd~"
                        | "mc.gen~"
                        | "rnbo~"
                ) {
                    return true;
                }
            }

            // Check for bpatcher
            if box_obj["maxclass"].as_str() == Some("bpatcher") {
                return true;
            }

            // Check for codebox objects (need multi-file for code extraction)
            if matches!(
                box_obj["maxclass"].as_str(),
                Some("v8.codebox") | Some("codebox")
            ) {
                if box_obj
                    .get("code")
                    .and_then(|c| c.as_str())
                    .map_or(false, |c| !c.is_empty())
                {
                    return true;
                }
            }
        }
    }

    false
}

// ---------------------------------------------------------------------------
// Multi-file decompile helper
// ---------------------------------------------------------------------------

/// Decompile a subpatcher-containing .maxpat to multiple .flutmax files,
/// parse each subpatcher file, and build an AbstractionRegistry.
///
/// Returns the main file source, the populated registry, code files,
/// RNBO patcher filenames, and RNBO source files (filename -> source).
fn decompile_multi_and_register(
    json_str: &str,
    base_name: &str,
) -> Result<
    (
        String,
        flutmax_sema::registry::AbstractionRegistry,
        std::collections::HashMap<String, String>,
        std::collections::HashSet<String>,
        std::collections::HashMap<String, String>,
        std::collections::HashSet<String>,
        std::collections::HashMap<String, String>,
    ),
    RoundtripResult,
> {
    let result = match flutmax_decompile::decompile_multi(json_str, base_name) {
        Ok(r) => r,
        Err(e) => return Err(RoundtripResult::Skip(format!("decompile_multi: {}", e))),
    };

    let mut registry = flutmax_sema::registry::AbstractionRegistry::new();

    // Collect RNBO source files
    let mut rnbo_sources: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for filename in &result.rnbo_patchers {
        if let Some(source) = result.files.get(filename) {
            rnbo_sources.insert(filename.clone(), source.clone());
        }
    }

    // Collect gen~ source files
    let mut gen_sources: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for filename in &result.gen_patchers {
        if let Some(source) = result.files.get(filename) {
            gen_sources.insert(filename.clone(), source.clone());
        }
    }

    // Parse subpatcher files and register their interfaces
    for (filename, source) in &result.files {
        if *filename != result.main_file {
            let name = filename.trim_end_matches(".flutmax");
            match flutmax_parser::parse(source) {
                Ok(ast) => {
                    registry.register(name, &ast);
                }
                Err(e) => {
                    return Err(RoundtripResult::CompileFail(format!(
                        "parse sub {}: {}",
                        filename, e
                    )));
                }
            }
        }
    }

    let main_source = match result.files.get(&result.main_file) {
        Some(s) => s.clone(),
        None => {
            return Err(RoundtripResult::Skip(
                "no main file in decompile result".into(),
            ))
        }
    };

    Ok((
        main_source,
        registry,
        result.code_files,
        result.rnbo_patchers,
        rnbo_sources,
        result.gen_patchers,
        gen_sources,
    ))
}

// ---------------------------------------------------------------------------
// Single file roundtrip
// ---------------------------------------------------------------------------

fn test_single_roundtrip(path: &str) -> RoundtripResult {
    // 1. Read file
    let json_str = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => return RoundtripResult::Skip(format!("read error: {}", e)),
    };

    // 2. Validate basic JSON structure
    let value: serde_json::Value = match serde_json::from_str(&json_str) {
        Ok(v) => v,
        Err(e) => return RoundtripResult::Skip(format!("invalid JSON: {}", e)),
    };

    // Must have patcher.boxes and patcher.lines
    if value.pointer("/patcher/boxes").is_none() {
        return RoundtripResult::Skip("missing patcher/boxes".into());
    }
    if value.pointer("/patcher/lines").is_none() {
        return RoundtripResult::Skip("missing patcher/lines".into());
    }

    // 3. Extract base name for multi-file decompile
    let base_name = std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("main");

    // 4. Decompile and compile — route based on subpatcher presence
    let (main_source, registry, code_files, rnbo_patchers, rnbo_sources, gen_patchers, gen_sources) =
        if has_subpatchers(&json_str) {
            // Use multi-file decompile for subpatcher patches
            match decompile_multi_and_register(&json_str, base_name) {
                Ok((source, reg, cf, rp, rs, gp, gs)) => (source, Some(reg), cf, rp, rs, gp, gs),
                Err(result) => return result,
            }
        } else {
            // Use simple decompile for flat patches
            match flutmax_decompile::decompile(&json_str) {
                Ok(s) => (
                    s,
                    None,
                    std::collections::HashMap::new(),
                    std::collections::HashSet::new(),
                    std::collections::HashMap::new(),
                    std::collections::HashSet::new(),
                    std::collections::HashMap::new(),
                ),
                Err(e) => return RoundtripResult::Skip(format!("decompile: {}", e)),
            }
        };

    // 5. Compile (with registry and code_files for codebox support)
    let code_files_ref = if code_files.is_empty() {
        None
    } else {
        Some(&code_files)
    };
    let regenerated_json = match flutmax_cli::compile_with_registry_and_code_files(
        &main_source,
        registry.as_ref(),
        code_files_ref,
    ) {
        Ok(s) => s,
        Err(e) => return RoundtripResult::CompileFail(format!("{}", e)),
    };

    // 6. Compare logical graphs (main patcher only — subpatchers are separate files)
    let orig_graph = match std::panic::catch_unwind(|| extract_logical_graph(&json_str)) {
        Ok(g) => g,
        Err(_) => {
            return RoundtripResult::Skip("graph extraction panicked on original".into());
        }
    };
    let regen_graph = match std::panic::catch_unwind(|| extract_logical_graph(&regenerated_json)) {
        Ok(g) => g,
        Err(_) => {
            return RoundtripResult::Mismatch(
                "graph extraction panicked on regenerated output".into(),
            );
        }
    };

    // Use disambiguation-tolerant comparison: nodes with the same base text
    // (e.g., gain~#0, gain~#1) are treated as interchangeable because the
    // specific #N assignment depends on JSON array order / layout coordinates,
    // which may differ between original and regenerated.
    if orig_graph.eq_tolerant(&regen_graph) {
        // Graph topology matches. Now check for unnecessary trigger insertion:
        // triggers whose ALL destinations are signal objects are never needed,
        // because signal processing has no execution ordering concerns.
        let unnecessary = find_unnecessary_triggers(&regenerated_json);
        if !unnecessary.is_empty() {
            return RoundtripResult::Mismatch(format!(
                "unnecessary trigger in signal path: {:?}",
                unnecessary
            ));
        }

        // Check codebox code field preservation: if the original has boxes with
        // `code` fields (v8.codebox, codebox), verify they are restored in the
        // regenerated output.
        if let Some(code_mismatch) = check_code_field_preservation(&json_str, &regenerated_json) {
            return RoundtripResult::Mismatch(code_mismatch);
        }

        // Check important attribute preservation (varnames, parameter_enable)
        if let Some(attr_mismatch) = check_important_attrs_preserved(&json_str, &regenerated_json) {
            return RoundtripResult::Mismatch(attr_mismatch);
        }

        // Check RNBO subpatcher roundtrip
        if !rnbo_patchers.is_empty() {
            let orig_value: serde_json::Value = serde_json::from_str(&json_str).unwrap();
            let orig_boxes = orig_value["patcher"]["boxes"].as_array().unwrap();

            for rnbo_filename in &rnbo_patchers {
                let rnbo_name = rnbo_filename.trim_end_matches(".flutmax");

                // Find the rnbo~ box with embedded patcher in original
                let orig_rnbo_json = orig_boxes.iter().find_map(|bw| {
                    let b = &bw["box"];
                    if let Some(patcher) = b.get("patcher") {
                        if patcher.get("classnamespace").and_then(|v| v.as_str()) == Some("rnbo") {
                            return Some(serde_json::json!({"patcher": patcher}));
                        }
                    }
                    None
                });

                if let Some(orig_rnbo) = orig_rnbo_json {
                    // Get the decompiled RNBO source
                    if let Some(rnbo_source) = rnbo_sources.get(rnbo_filename) {
                        // Compile with RNBO mode
                        match flutmax_cli::compile_rnbo(rnbo_source) {
                            Ok(regen_rnbo_json) => {
                                // Compare logical graphs
                                let orig_str = serde_json::to_string(&orig_rnbo).unwrap();
                                let orig_rnbo_graph = match std::panic::catch_unwind(|| {
                                    extract_logical_graph(&orig_str)
                                }) {
                                    Ok(g) => g,
                                    Err(_) => continue, // Skip if graph extraction panics
                                };
                                let regen_rnbo_graph = match std::panic::catch_unwind(|| {
                                    extract_logical_graph(&regen_rnbo_json)
                                }) {
                                    Ok(g) => g,
                                    Err(_) => {
                                        return RoundtripResult::Mismatch(format!(
                                            "RNBO subpatcher '{}' graph extraction panicked on regenerated output", rnbo_name
                                        ));
                                    }
                                };
                                if !orig_rnbo_graph.eq_tolerant(&regen_rnbo_graph) {
                                    return RoundtripResult::Mismatch(format!(
                                        "RNBO subpatcher '{}' graph mismatch",
                                        rnbo_name
                                    ));
                                }
                            }
                            Err(e) => {
                                return RoundtripResult::CompileFail(format!(
                                    "RNBO compile {}: {}",
                                    rnbo_name, e
                                ));
                            }
                        }
                    }
                }
            }
        }

        // Check gen~ subpatcher roundtrip
        if !gen_patchers.is_empty() {
            let orig_value: serde_json::Value = serde_json::from_str(&json_str).unwrap();
            let orig_boxes = orig_value["patcher"]["boxes"].as_array().unwrap();

            // Collect all gen~ embedded patchers from original
            let orig_gen_patchers: Vec<serde_json::Value> = orig_boxes
                .iter()
                .filter_map(|bw| {
                    let b = &bw["box"];
                    if let Some(patcher) = b.get("patcher") {
                        if patcher.get("classnamespace").and_then(|v| v.as_str()) == Some("dsp.gen")
                        {
                            return Some(serde_json::json!({"patcher": patcher}));
                        }
                    }
                    None
                })
                .collect();

            for gen_filename in &gen_patchers {
                let gen_name = gen_filename.trim_end_matches(".flutmax");

                if let Some(gen_source) = gen_sources.get(gen_filename) {
                    match flutmax_cli::compile_gen(gen_source) {
                        Ok(regen_gen_json) => {
                            let regen_gen_graph = match std::panic::catch_unwind(|| {
                                extract_logical_graph(&regen_gen_json)
                            }) {
                                Ok(g) => g,
                                Err(_) => {
                                    return RoundtripResult::Mismatch(format!(
                                        "gen~ '{}' graph extraction panicked on regenerated output",
                                        gen_name
                                    ));
                                }
                            };

                            // Find a matching gen~ patcher from the original (by graph equivalence)
                            let mut matched = false;
                            for orig_gen in &orig_gen_patchers {
                                let orig_str = serde_json::to_string(orig_gen).unwrap();
                                let orig_gen_graph = match std::panic::catch_unwind(|| {
                                    extract_logical_graph(&orig_str)
                                }) {
                                    Ok(g) => g,
                                    Err(_) => continue,
                                };
                                if orig_gen_graph.eq_tolerant(&regen_gen_graph) {
                                    matched = true;
                                    break;
                                }
                            }

                            if !matched {
                                // Detailed diff against first original gen~ for debugging
                                let detail = if let Some(first_orig) = orig_gen_patchers.first() {
                                    let orig_str = serde_json::to_string(first_orig).unwrap();
                                    if let Ok(orig_g) = std::panic::catch_unwind(|| {
                                        extract_logical_graph(&orig_str)
                                    }) {
                                        let orig_ec = LogicalGraph::edge_base_counts(&orig_g.edges);
                                        let regen_ec =
                                            LogicalGraph::edge_base_counts(&regen_gen_graph.edges);
                                        let missing_e: Vec<_> = orig_ec
                                            .iter()
                                            .filter(|(k, v)| regen_ec.get(k).unwrap_or(&0) < *v)
                                            .take(5)
                                            .map(|(k, _)| {
                                                format!("{}:{}->{}:{}", k.0, k.1, k.2, k.3)
                                            })
                                            .collect();
                                        let extra_e: Vec<_> = regen_ec
                                            .iter()
                                            .filter(|(k, v)| orig_ec.get(k).unwrap_or(&0) < *v)
                                            .take(5)
                                            .map(|(k, _)| {
                                                format!("{}:{}->{}:{}", k.0, k.1, k.2, k.3)
                                            })
                                            .collect();
                                        format!(
                                            "missing_edges={:?}, extra_edges={:?}",
                                            missing_e, extra_e
                                        )
                                    } else {
                                        "graph extraction failed".to_string()
                                    }
                                } else {
                                    "no original gen~ patchers".to_string()
                                };
                                return RoundtripResult::Mismatch(format!(
                                    "gen~ '{}': {}",
                                    gen_name, detail
                                ));
                            }
                        }
                        Err(e) => {
                            return RoundtripResult::CompileFail(format!(
                                "gen~ compile {}: {}",
                                gen_name, e
                            ));
                        }
                    }
                }
            }
        }

        RoundtripResult::Pass
    } else {
        // Build a detailed mismatch report using base-text multisets
        let orig_node_counts = LogicalGraph::node_base_counts(&orig_graph.nodes);
        let regen_node_counts = LogicalGraph::node_base_counts(&regen_graph.nodes);
        let orig_edge_counts = LogicalGraph::edge_base_counts(&orig_graph.edges);
        let regen_edge_counts = LogicalGraph::edge_base_counts(&regen_graph.edges);

        let mut detail = format!(
            "orig={} nodes/{} edges, regen={} nodes/{} edges",
            orig_graph.nodes.len(),
            orig_graph.edges.len(),
            regen_graph.nodes.len(),
            regen_graph.edges.len(),
        );

        // Node differences (base text level)
        let missing_nodes: Vec<_> = orig_node_counts
            .iter()
            .filter(|(k, v)| regen_node_counts.get(*k).unwrap_or(&0) < *v)
            .map(|(k, v)| {
                let regen_count = regen_node_counts.get(k).unwrap_or(&0);
                if *v > 1 || *regen_count > 0 {
                    format!("{}(x{}→x{})", k, v, regen_count)
                } else {
                    k.clone()
                }
            })
            .collect();
        let extra_nodes: Vec<_> = regen_node_counts
            .iter()
            .filter(|(k, v)| orig_node_counts.get(*k).unwrap_or(&0) < *v)
            .map(|(k, v)| {
                let orig_count = orig_node_counts.get(k).unwrap_or(&0);
                if *v > 1 || *orig_count > 0 {
                    format!("{}(x{}→x{})", k, orig_count, v)
                } else {
                    k.clone()
                }
            })
            .collect();

        if !missing_nodes.is_empty() {
            detail.push_str(&format!("\n    missing nodes: {:?}", missing_nodes));
        }
        if !extra_nodes.is_empty() {
            detail.push_str(&format!("\n    extra nodes: {:?}", extra_nodes));
        }

        // Edge differences (base text level)
        let missing_edges: Vec<_> = orig_edge_counts
            .iter()
            .filter(|(k, v)| regen_edge_counts.get(k).unwrap_or(&0) < *v)
            .map(|(k, _)| format!("{}:{}->{}:{}", k.0, k.1, k.2, k.3))
            .collect();
        let extra_edges: Vec<_> = regen_edge_counts
            .iter()
            .filter(|(k, v)| orig_edge_counts.get(k).unwrap_or(&0) < *v)
            .map(|(k, _)| format!("{}:{}->{}:{}", k.0, k.1, k.2, k.3))
            .collect();

        if !missing_edges.is_empty() {
            detail.push_str(&format!("\n    missing edges: {:?}", missing_edges));
        }
        if !extra_edges.is_empty() {
            detail.push_str(&format!("\n    extra edges: {:?}", extra_edges));
        }

        RoundtripResult::Mismatch(detail)
    }
}

// ---------------------------------------------------------------------------
// Aggregate results
// ---------------------------------------------------------------------------

#[allow(dead_code)]
struct SuiteResults {
    total: usize,
    pass: usize,
    skip: usize,
    compile_fail: usize,
    mismatch: usize,
    compile_fail_details: Vec<(String, String)>,
    mismatch_details: Vec<(String, String)>,
}

fn run_roundtrip_suite(label: &str, dir: &str) -> SuiteResults {
    let maxpat_files = find_all_maxpat_files(dir);
    let total = maxpat_files.len();

    let mut pass = 0usize;
    let mut skip = 0usize;
    let mut compile_fail = 0usize;
    let mut mismatch = 0usize;
    let mut compile_fail_details: Vec<(String, String)> = Vec::new();
    let mut mismatch_details: Vec<(String, String)> = Vec::new();

    for (i, path) in maxpat_files.iter().enumerate() {
        // Print progress every 100 files
        if (i + 1) % 100 == 0 || i + 1 == total {
            eprint!("\r  [{}/{}] processing...", i + 1, total);
        }

        let display_path = path.strip_prefix(dir).unwrap_or(path).to_string();

        match test_single_roundtrip(path) {
            RoundtripResult::Pass => pass += 1,
            RoundtripResult::Skip(_) => skip += 1,
            RoundtripResult::CompileFail(reason) => {
                compile_fail += 1;
                compile_fail_details.push((display_path, reason));
            }
            RoundtripResult::Mismatch(reason) => {
                mismatch += 1;
                mismatch_details.push((display_path, reason));
            }
        }
    }

    eprintln!(); // newline after progress

    // Print summary
    eprintln!("\n=== {} Roundtrip Results ===", label);
    eprintln!("Total:        {}", total);
    if total > 0 {
        let pct = |n: usize| n as f64 / total as f64 * 100.0;
        eprintln!("Pass:         {:>4} ({:.1}%)", pass, pct(pass));
        eprintln!("Skip:         {:>4} ({:.1}%)", skip, pct(skip));
        eprintln!(
            "Compile fail: {:>4} ({:.1}%)",
            compile_fail,
            pct(compile_fail)
        );
        eprintln!("Mismatch:     {:>4} ({:.1}%)", mismatch, pct(mismatch));
    }

    // Print mismatch details (these are the important ones)
    if !mismatch_details.is_empty() {
        eprintln!("\n--- Graph Mismatches (FAIL) ---");
        for (path, reason) in &mismatch_details {
            eprintln!("  {}: {}", path, reason);
        }
    }

    // Print compile fail details (informational)
    if !compile_fail_details.is_empty() {
        eprintln!(
            "\n--- Compile Failures ({} total, showing first 50) ---",
            compile_fail_details.len()
        );
        for (path, reason) in compile_fail_details.iter().take(50) {
            // Truncate long error messages
            let short_reason = if reason.len() > 120 {
                format!("{}...", &reason[..120])
            } else {
                reason.clone()
            };
            eprintln!("  {}: {}", path, short_reason);
        }
        if compile_fail_details.len() > 50 {
            eprintln!("  ... and {} more", compile_fail_details.len() - 50);
        }
    }

    SuiteResults {
        total,
        pass,
        skip,
        compile_fail,
        mismatch,
        compile_fail_details,
        mismatch_details,
    }
}

// ---------------------------------------------------------------------------
// Known mismatches
// ---------------------------------------------------------------------------

/// Known graph mismatches that are tracked but should not block the test.
/// These represent bugs in the decompile/compile pipeline that need fixing.
/// When a bug is fixed, remove the entry from `known_mismatches.txt`.
///
/// Primary causes (495 patches):
/// - `trigger b b` auto-insertion: compiler inserts bang triggers that destroy
///   input values. Should use `trigger f f` (value-preserving) or skip for signal.
/// - Template argument loss: `#N`/`$fN` args stripped during decompile, losing
///   abstraction default values (e.g., `*~ #1` → `*~`).
fn known_mismatch_paths() -> Vec<String> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/known_mismatches.txt");
    match std::fs::read_to_string(path) {
        Ok(content) => content
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect(),
        Err(_) => vec![],
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Test all .maxpat files from the Max.app installation.
///
/// The test PASSES as long as there are no *unexpected* mismatches.
/// Known mismatches are tracked in `known_mismatch_paths()` and reported
/// separately. Skips and compile failures are expected.
/// If Max.app is not installed, the test is silently skipped (CI compatibility).
#[test]
fn max_reference_roundtrip_all() {
    let max_dir = match flutmax_validate::find_max_c74_dir() {
        Some(dir) => dir,
        None => {
            eprintln!("SKIP: Max not installed (set MAX_INSTALL_PATH to override)");
            return;
        }
    };

    let results = run_roundtrip_suite("Max Reference", max_dir.to_str().unwrap());

    if results.total == 0 {
        eprintln!("WARNING: No .maxpat files found in {}", max_dir.display());
        return;
    }

    // Separate known vs unexpected mismatches
    let known = known_mismatch_paths();
    let mut known_found: Vec<&(String, String)> = Vec::new();
    let mut unexpected: Vec<&(String, String)> = Vec::new();

    for entry in &results.mismatch_details {
        if known.iter().any(|k| entry.0 == *k) {
            known_found.push(entry);
        } else {
            unexpected.push(entry);
        }
    }

    // Report known mismatches
    if !known_found.is_empty() {
        eprintln!(
            "\n--- Known Mismatches ({}/{} tracked) ---",
            known_found.len(),
            known.len()
        );
        for (path, _reason) in &known_found {
            eprintln!("  {} (known)", path);
        }
    }

    // Check for known mismatches that have been fixed (they would be Pass now)
    let fixed: Vec<&String> = known
        .iter()
        .filter(|k| !known_found.iter().any(|f| f.0 == **k))
        .collect();
    if !fixed.is_empty() {
        eprintln!("\n--- Possibly Fixed (remove from known list) ---");
        for path in &fixed {
            eprintln!("  {}", path);
        }
    }

    // Assert no unexpected mismatches
    assert!(
        unexpected.is_empty(),
        "{} unexpected graph mismatches:\n{}",
        unexpected.len(),
        unexpected
            .iter()
            .map(|(p, r)| format!("  {}: {}", p, r))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Test .maxpat files from the user's Max projects directory.
///
/// Set `FLUTMAX_USER_PATCHES_DIR` to a directory containing .maxpat files.
/// Silently skipped if the env var is not set or the directory does not exist.
#[test]
fn user_patches_roundtrip() {
    let user_dir = match std::env::var("FLUTMAX_USER_PATCHES_DIR") {
        Ok(dir) => dir,
        Err(_) => {
            eprintln!("SKIP: FLUTMAX_USER_PATCHES_DIR not set");
            return;
        }
    };

    if !std::path::Path::new(&user_dir).exists() {
        eprintln!("SKIP: User Max patches directory not found at {}", user_dir);
        return;
    }

    let results = run_roundtrip_suite("User Patches", &user_dir);

    if results.total == 0 {
        eprintln!("WARNING: No .maxpat files found in {}", user_dir);
        return;
    }

    // Known mismatches in user patches (argument loss, cycle reordering)
    let user_known = vec!["max_mixer/mixer.maxpat", "max_mixer/mixer_test.maxpat"];
    let unexpected_user: Vec<_> = results
        .mismatch_details
        .iter()
        .filter(|(path, _)| !user_known.iter().any(|k| path == k))
        .collect();

    if !unexpected_user.is_empty() {
        eprintln!("\n--- Unexpected User Patch Mismatches ---");
        for (path, reason) in &unexpected_user {
            eprintln!("  {}: {}", path, reason);
        }
    }

    assert_eq!(
        unexpected_user.len(),
        0,
        "{} unexpected user patch mismatches:\n{}",
        unexpected_user.len(),
        unexpected_user
            .iter()
            .map(|(p, r)| format!("  {}: {}", p, r))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Test v8.codebox code field preservation through the roundtrip.
#[test]
fn codebox_code_field_roundtrip() {
    let path = "/Applications/Max.app/Contents/Resources/C74/packages/Jitter Geometry/patchers/abstractions/geom.edgelines.maxpat";
    if !std::path::Path::new(path).exists() {
        eprintln!("SKIP: test file not found");
        return;
    }

    let json_str = std::fs::read_to_string(path).unwrap();
    let base_name = "geom_edgelines";

    // Step 1: Decompile with code extraction
    let result = flutmax_decompile::decompile_multi(&json_str, base_name).unwrap();
    assert!(
        !result.code_files.is_empty(),
        "decompile should extract code files from v8.codebox"
    );

    // Step 2: Parse and register subpatchers
    let mut registry = flutmax_sema::registry::AbstractionRegistry::new();
    for (filename, source) in &result.files {
        if *filename != result.main_file {
            let name = filename.trim_end_matches(".flutmax");
            let ast = flutmax_parser::parse(source).unwrap();
            registry.register(name, &ast);
        }
    }

    let main_source = result.files.get(&result.main_file).unwrap();
    eprintln!("  main source:\n{}", main_source);

    // Step 3: Compile with code_files
    let regenerated = flutmax_cli::compile_with_registry_and_code_files(
        main_source,
        Some(&registry),
        Some(&result.code_files),
    )
    .unwrap();

    // Step 4: Verify code field is present in regenerated JSON
    let regen_val: serde_json::Value = serde_json::from_str(&regenerated).unwrap();
    let regen_codes = extract_code_fields(&regen_val);
    let orig_val: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let orig_codes = extract_code_fields(&orig_val);

    eprintln!("  original code fields: {}", orig_codes.len());
    eprintln!("  regenerated code fields: {}", regen_codes.len());

    assert_eq!(
        orig_codes.len(),
        regen_codes.len(),
        "code field count mismatch: orig={}, regen={}",
        orig_codes.len(),
        regen_codes.len()
    );

    for (maxclass, orig_code) in &orig_codes {
        let found = regen_codes
            .iter()
            .any(|(mc, rc)| mc == maxclass && rc == orig_code);
        assert!(
            found,
            "code not preserved for {}: {:?}...",
            maxclass,
            &orig_code[..orig_code.len().min(60)]
        );
    }
}

/// Test real-world .maxpat files from GitHub repositories (E40).
///
/// These patches are more complex than Max.app reference patches and exercise
/// edge cases in the decompile/compile pipeline.
#[test]
fn real_patches_roundtrip() {
    let real_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/real_patches/");

    if !std::path::Path::new(real_dir).exists() {
        eprintln!("SKIP: real_patches directory not found at {}", real_dir);
        return;
    }

    let results = run_roundtrip_suite("Real Patches (GitHub)", real_dir);

    if results.total == 0 {
        eprintln!("WARNING: No .maxpat files found in {}", real_dir);
        return;
    }

    // Known mismatches in real patches (edge cases in object naming)
    let real_known = vec![
        // gbr.wind= — object name contains '=' (not in grammar)
        "slegroux_ambispat.maxpat",
        "slegroux_slg.addanpca~.maxpat",
        "slegroux_slg.addan~.maxpat",
        "slegroux_slg.peaksynth~.maxpat",
        // #2Controls — template-prefixed abstraction name
        "slegroux_spadTranche.maxpat",
    ];
    let unexpected: Vec<_> = results
        .mismatch_details
        .iter()
        .filter(|(path, _)| !real_known.iter().any(|k| path.contains(k)))
        .collect();

    if !unexpected.is_empty() {
        eprintln!("\n--- Unexpected Real Patch Mismatches ---");
        for (path, reason) in &unexpected {
            eprintln!("  {}: {}", path, reason);
        }
    }

    assert_eq!(
        unexpected.len(),
        0,
        "{} unexpected real patch mismatches:\n{}",
        unexpected.len(),
        unexpected
            .iter()
            .map(|(p, r)| format!("  {}: {}", p, r))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

// ---------------------------------------------------------------------------
// E43 Phase 4: Parser migration AST equivalence
// ---------------------------------------------------------------------------

/// E43 Phase 4: Verify hand-written parser produces identical AST to tree-sitter parser.
///
/// For each .maxpat file: decompile -> .flutmax -> parse with BOTH parsers -> compare ASTs.
/// This validates the hand-written parser against real-world patches.
#[test]
fn parser_migration_ast_equivalence() {
    // Test against Max reference patches
    if let Some(max_dir) = flutmax_validate::find_max_c74_dir() {
        run_parser_comparison("Max Reference", max_dir.to_str().unwrap());
    }

    // Test against real project patches
    let real_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/real_patches/");
    if std::path::Path::new(real_dir).exists() {
        run_parser_comparison("Real Patches", real_dir);
    }
}

fn run_parser_comparison(label: &str, dir: &str) {
    let maxpat_files = find_all_maxpat_files(dir);
    let total = maxpat_files.len();

    let mut compared = 0usize;
    let mut skipped = 0usize;
    let mut match_count = 0usize;
    let mut mismatch_count = 0usize;
    let mut parse_new_fail = 0usize;
    let mut mismatch_details: Vec<String> = Vec::new();
    let mut parse_fail_details: Vec<String> = Vec::new();

    for (i, path) in maxpat_files.iter().enumerate() {
        if (i + 1) % 200 == 0 || i + 1 == total {
            eprint!("\r  [{}/{}] comparing parsers...", i + 1, total);
        }

        // Read and decompile
        let json_str = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };

        let flutmax_source = match flutmax_decompile::decompile(&json_str) {
            Ok(s) => s,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };

        if flutmax_source.trim().is_empty() {
            skipped += 1;
            continue;
        }

        // Parse with tree-sitter
        let legacy_ast = match flutmax_parser::parse(&flutmax_source) {
            Ok(ast) => ast,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };

        // Parse with hand-written parser
        let new_ast = match flutmax_parser::parse_new(&flutmax_source) {
            Ok(ast) => ast,
            Err(e) => {
                parse_new_fail += 1;
                let display_path = path.strip_prefix(dir).unwrap_or(path);
                if parse_fail_details.len() < 20 {
                    parse_fail_details.push(format!("{}: {}", display_path, e));
                }
                continue;
            }
        };

        compared += 1;

        // Compare ASTs
        if legacy_ast == new_ast {
            match_count += 1;
        } else {
            mismatch_count += 1;
            let display_path = path.strip_prefix(dir).unwrap_or(path);
            if mismatch_details.len() < 20 {
                let diff = find_ast_diff(&legacy_ast, &new_ast);
                mismatch_details.push(format!("{}: {}", display_path, diff));
            }
        }
    }

    eprintln!();
    eprintln!("\n=== {} Parser Comparison ===", label);
    eprintln!("Total files:    {}", total);
    eprintln!("Compared:       {}", compared);
    eprintln!("Skipped:        {}", skipped);
    eprintln!(
        "AST match:      {} ({:.1}%)",
        match_count,
        if compared > 0 {
            match_count as f64 / compared as f64 * 100.0
        } else {
            0.0
        }
    );
    eprintln!("AST mismatch:   {}", mismatch_count);
    eprintln!("New parse fail: {}", parse_new_fail);

    if !parse_fail_details.is_empty() {
        eprintln!(
            "\n--- New Parser Failures (first {}) ---",
            parse_fail_details.len()
        );
        for detail in &parse_fail_details {
            eprintln!("  {}", detail);
        }
    }

    if !mismatch_details.is_empty() {
        eprintln!(
            "\n--- AST Mismatches (first {}) ---",
            mismatch_details.len()
        );
        for detail in &mismatch_details {
            eprintln!("  {}", detail);
        }
    }

    // Report-only: don't assert yet. Once the new parser handles all cases, uncomment:
    // assert_eq!(mismatch_count, 0, "AST mismatches found");
    // assert_eq!(parse_new_fail, 0, "New parser failures found");
}

/// Compare two ASTs and return a human-readable description of the first difference found.
fn find_ast_diff(legacy: &flutmax_ast::Program, new: &flutmax_ast::Program) -> String {
    // Compare in_decls
    if legacy.in_decls.len() != new.in_decls.len() {
        return format!(
            "in_decls count: {} vs {}",
            legacy.in_decls.len(),
            new.in_decls.len()
        );
    }
    for (i, (ld, nd)) in legacy.in_decls.iter().zip(new.in_decls.iter()).enumerate() {
        if ld != nd {
            return format!("in_decls[{}]: {:?} vs {:?}", i, ld, nd);
        }
    }

    // Compare out_decls
    if legacy.out_decls.len() != new.out_decls.len() {
        return format!(
            "out_decls count: {} vs {}",
            legacy.out_decls.len(),
            new.out_decls.len()
        );
    }
    for (i, (ld, nd)) in legacy
        .out_decls
        .iter()
        .zip(new.out_decls.iter())
        .enumerate()
    {
        if ld != nd {
            return format!("out_decls[{}]: {:?} vs {:?}", i, ld, nd);
        }
    }

    // Compare wires
    if legacy.wires.len() != new.wires.len() {
        return format!("wires count: {} vs {}", legacy.wires.len(), new.wires.len());
    }
    for (i, (lw, nw)) in legacy.wires.iter().zip(new.wires.iter()).enumerate() {
        if lw.name != nw.name {
            return format!("wire[{}] name: {:?} vs {:?}", i, lw.name, nw.name);
        }
        if lw.value != nw.value {
            return format!(
                "wire[{}] '{}' value: {:?} vs {:?}",
                i, lw.name, lw.value, nw.value
            );
        }
        if lw.attrs != nw.attrs {
            return format!(
                "wire[{}] '{}' attrs: {:?} vs {:?}",
                i, lw.name, lw.attrs, nw.attrs
            );
        }
        if lw.span != nw.span {
            return format!(
                "wire[{}] '{}' span: {:?} vs {:?}",
                i, lw.name, lw.span, nw.span
            );
        }
    }

    // Compare destructuring_wires
    if legacy.destructuring_wires != new.destructuring_wires {
        return format!(
            "destructuring_wires: {} vs {}",
            legacy.destructuring_wires.len(),
            new.destructuring_wires.len()
        );
    }

    // Compare msg_decls
    if legacy.msg_decls != new.msg_decls {
        return format!(
            "msg_decls: {} vs {}",
            legacy.msg_decls.len(),
            new.msg_decls.len()
        );
    }

    // Compare out_assignments
    if legacy.out_assignments != new.out_assignments {
        return format!(
            "out_assignments: {} vs {}",
            legacy.out_assignments.len(),
            new.out_assignments.len()
        );
    }

    // Compare direct_connections
    if legacy.direct_connections != new.direct_connections {
        return format!(
            "direct_connections: {} vs {}",
            legacy.direct_connections.len(),
            new.direct_connections.len()
        );
    }

    // Compare feedback_decls
    if legacy.feedback_decls != new.feedback_decls {
        return format!(
            "feedback_decls: {} vs {}",
            legacy.feedback_decls.len(),
            new.feedback_decls.len()
        );
    }

    // Compare feedback_assignments
    if legacy.feedback_assignments != new.feedback_assignments {
        return format!(
            "feedback_assignments: {} vs {}",
            legacy.feedback_assignments.len(),
            new.feedback_assignments.len()
        );
    }

    // Compare state_decls
    if legacy.state_decls != new.state_decls {
        return format!(
            "state_decls: {} vs {}",
            legacy.state_decls.len(),
            new.state_decls.len()
        );
    }

    // Compare state_assignments
    if legacy.state_assignments != new.state_assignments {
        return format!(
            "state_assignments: {} vs {}",
            legacy.state_assignments.len(),
            new.state_assignments.len()
        );
    }

    // Fallback: use Debug output to find first character difference
    let legacy_dbg = format!("{:?}", legacy);
    let new_dbg = format!("{:?}", new);
    for (i, (lc, nc)) in legacy_dbg.chars().zip(new_dbg.chars()).enumerate() {
        if lc != nc {
            let start = i.saturating_sub(20);
            let end_l = (i + 40).min(legacy_dbg.len());
            let end_n = (i + 40).min(new_dbg.len());
            return format!(
                "diff at char {}: legacy=...{}... new=...{}...",
                i,
                &legacy_dbg[start..end_l],
                &new_dbg[start..end_n]
            );
        }
    }
    if legacy_dbg.len() != new_dbg.len() {
        return format!(
            "debug output length differs: {} vs {}",
            legacy_dbg.len(),
            new_dbg.len()
        );
    }
    "unknown diff (PartialEq disagrees with Debug)".to_string()
}
