use crate::alias;
use crate::parser::{DecompileError, MaxBox, MaxLine, MaxPat};
use flutmax_objdb::{InletSpec, ObjectDb};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};

/// An `in` port declaration.
#[derive(Debug, Clone)]
pub struct InDeclInfo {
    pub index: u32,
    pub name: String,
    pub port_type: String,
}

/// An `out` port declaration.
#[derive(Debug, Clone)]
pub struct OutDeclInfo {
    pub index: u32,
    pub name: String,
    pub port_type: String,
}

/// A wire declaration (e.g., `wire osc = cycle~(440);`).
#[derive(Debug, Clone)]
pub struct WireInfo {
    pub name: String,
    pub expr: String,
    /// Object attributes as key-value pairs for `.attr()` chain output.
    pub attrs: Vec<(String, String)>,
}

/// A message box declaration (e.g., `msg click = "bang";`).
#[derive(Debug, Clone)]
pub struct MsgInfo {
    pub name: String,
    pub content: String,
    /// Object attributes as key-value pairs for `.attr()` chain output.
    pub attrs: Vec<(String, String)>,
}

/// An output assignment (e.g., `out[0] = osc;`).
#[derive(Debug, Clone)]
pub struct OutAssignInfo {
    pub index: u32,
    pub wire_name: String,
}

/// A direct connection for fanin (e.g., `c.in[0] = b;`).
#[derive(Debug, Clone)]
pub struct DirectConnectionInfo {
    pub target_wire: String,
    pub inlet: u32,
    pub source_wire: String,
}

/// A UI entry for the .uiflutmax sidecar file.
///
/// Contains the position (patching_rect) and decorative attributes for a
/// single wire/message/inlet/outlet in the decompiled patch.
#[derive(Debug, Clone)]
pub struct UiEntryInfo {
    /// Wire name (key in the .uiflutmax JSON object).
    pub name: String,
    /// Position: [x, y, width, height] from patching_rect.
    pub rect: [f64; 4],
    /// Decorative attributes (e.g., bgcolor, fontsize, bordercolor).
    pub decorative_attrs: Vec<(String, String)>,
}

/// A comment box with text and Y position for proximity placement.
#[derive(Debug, Clone)]
pub struct CommentInfo {
    pub text: String,
    /// Full rect [x, y, w, h] for .uiflutmax position data.
    pub rect: [f64; 4],
    /// Y coordinate from patching_rect for proximity matching with nearby wires.
    pub y_position: f64,
}

/// A visual-only panel box with position and decorative attributes.
#[derive(Debug, Clone)]
pub struct PanelInfo {
    pub rect: [f64; 4],
    pub attrs: Vec<(String, String)>,
}

/// A visual-only image box (fpic) with position and filename.
#[derive(Debug, Clone)]
pub struct ImageInfo {
    pub rect: [f64; 4],
    pub pic: String,
}

/// Check if a box is a visual-only element (no logic, purely decorative).
fn is_visual_only_box(b: &MaxBox) -> bool {
    matches!(b.maxclass.as_str(), "comment" | "panel" | "fpic" | "swatch")
}

/// Subpatcher info: inlet/outlet counts + inlet names from `in` declarations.
struct SubpatcherInfo {
    inlet_count: u32,
    #[allow(dead_code)]
    outlet_count: u32,
    /// Inlet names from the subpatcher's `in` declarations (e.g., ["carrier_freq", "harmonicity"]).
    inlet_names: Vec<String>,
}

/// The fully analyzed decompiled patch.
#[derive(Debug)]
pub struct DecompiledPatch {
    pub in_decls: Vec<InDeclInfo>,
    pub out_decls: Vec<OutDeclInfo>,
    pub comments: Vec<CommentInfo>,
    pub messages: Vec<MsgInfo>,
    pub wires: Vec<WireInfo>,
    pub out_assignments: Vec<OutAssignInfo>,
    pub direct_connections: Vec<DirectConnectionInfo>,
    /// Code files extracted from codebox objects: (filename, code_content).
    /// v8.codebox → `.js`, gen~ codebox → `.genexpr`.
    pub code_files: Vec<(String, String)>,
    /// UI data for .uiflutmax sidecar: positions + decorative attrs per wire.
    pub ui_entries: Vec<UiEntryInfo>,
    /// Patcher-level UI settings (e.g., window rect).
    pub patcher_rect: Option<[f64; 4]>,
    /// Visual-only panel boxes (no logic connections).
    pub panels: Vec<PanelInfo>,
    /// Visual-only image boxes (fpic, no logic connections).
    pub images: Vec<ImageInfo>,
}

/// Incoming connection to a specific inlet of a node.
#[derive(Debug, Clone)]
struct IncomingConnection {
    source_id: String,
    #[allow(dead_code)]
    source_outlet: u32,
}

/// Check if a box is an RNBO inlet (inport or in~).
fn is_rnbo_inlet(b: &MaxBox, is_rnbo: bool) -> bool {
    if !is_rnbo {
        return false;
    }
    let first = b
        .text
        .as_deref()
        .and_then(|t| t.split_whitespace().next())
        .unwrap_or("");
    matches!(first, "inport" | "in~")
}

/// Check if a box is an RNBO outlet (outport or out~).
fn is_rnbo_outlet(b: &MaxBox, is_rnbo: bool) -> bool {
    if !is_rnbo {
        return false;
    }
    let first = b
        .text
        .as_deref()
        .and_then(|t| t.split_whitespace().next())
        .unwrap_or("");
    matches!(first, "outport" | "out~")
}

/// Check if a box is a gen~ inlet (`in N`).
fn is_gen_inlet(b: &MaxBox, is_gen: bool) -> bool {
    if !is_gen {
        return false;
    }
    if b.maxclass != "newobj" {
        return false;
    }
    let first = b
        .text
        .as_deref()
        .and_then(|t| t.split_whitespace().next())
        .unwrap_or("");
    first == "in"
}

/// Check if a box is a gen~ outlet (`out N`).
fn is_gen_outlet(b: &MaxBox, is_gen: bool) -> bool {
    if !is_gen {
        return false;
    }
    if b.maxclass != "newobj" {
        return false;
    }
    let first = b
        .text
        .as_deref()
        .and_then(|t| t.split_whitespace().next())
        .unwrap_or("");
    first == "out"
}

/// Analyze a parsed MaxPat and produce a DecompiledPatch.
///
/// When `objdb` is provided, wire expressions use named arguments from
/// inlet descriptions (e.g., `biquad~(input: osc, frequency: cutoff)`).
pub fn analyze(
    maxpat: &MaxPat,
    objdb: Option<&ObjectDb>,
) -> Result<DecompiledPatch, DecompileError> {
    let box_map: HashMap<&str, &MaxBox> = maxpat.boxes.iter().map(|b| (b.id.as_str(), b)).collect();

    // Step 1: Remove trigger nodes and rewire connections
    let trigger_result = remove_triggers(maxpat, &box_map);
    let mut effective_lines = trigger_result.lines;
    let trigger_ids = trigger_result.trigger_ids;

    // Step 1b: Sort fan-out destinations by Max execution order.
    // When multiple connections share the same source outlet, Max evaluates them
    // by destination box coordinates: X descending (right first), Y descending
    // (bottom first) as tiebreaker. Skip fan-outs whose order was already defined
    // by explicit trigger outlet indices.
    sort_fanout_lines(
        &mut effective_lines,
        &box_map,
        &trigger_result.trigger_ordered_sources,
    );

    // Step 2: Classify boxes
    let is_rnbo = maxpat.classnamespace.as_deref() == Some("rnbo");
    let is_gen = maxpat.classnamespace.as_deref() == Some("dsp.gen");
    let mut inlet_boxes: Vec<&MaxBox> = Vec::new();
    let mut outlet_boxes: Vec<&MaxBox> = Vec::new();
    let mut comment_boxes: Vec<&MaxBox> = Vec::new();
    let mut message_boxes: Vec<&MaxBox> = Vec::new();
    let mut wire_candidate_ids: Vec<&str> = Vec::new();

    for b in &maxpat.boxes {
        match b.maxclass.as_str() {
            "inlet" | "inlet~" => inlet_boxes.push(b),
            "outlet" | "outlet~" => outlet_boxes.push(b),
            "comment" => comment_boxes.push(b),
            "message" => message_boxes.push(b),
            _ if is_rnbo_inlet(b, is_rnbo) => inlet_boxes.push(b),
            _ if is_rnbo_outlet(b, is_rnbo) => outlet_boxes.push(b),
            _ if is_gen_inlet(b, is_gen) => inlet_boxes.push(b),
            _ if is_gen_outlet(b, is_gen) => outlet_boxes.push(b),
            _ => {
                if !trigger_ids.contains(b.id.as_str()) {
                    wire_candidate_ids.push(&b.id);
                }
            }
        }
    }

    // Step 3: Build in/out declarations
    // For gen~/RNBO, sort by text number (in 1, in 2, out 1, out 2).
    // For standard Max, sort by X coordinate (left to right = port index).
    if is_gen || is_rnbo {
        let text_num = |b: &&MaxBox| -> u32 {
            b.text
                .as_deref()
                .and_then(|t| t.split_whitespace().nth(1))
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or(0)
        };
        inlet_boxes.sort_by_key(text_num);
        outlet_boxes.sort_by_key(text_num);
    } else {
        inlet_boxes.sort_by(|a, b| {
            a.patching_rect_x()
                .partial_cmp(&b.patching_rect_x())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        outlet_boxes.sort_by(|a, b| {
            a.patching_rect_x()
                .partial_cmp(&b.patching_rect_x())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    let in_decls = build_in_decls(&inlet_boxes);
    let mut out_decls = build_out_decls(&outlet_boxes);

    // Step 4: Build connection map (dest_id -> Vec<(inlet_index, source_id, source_outlet)>)
    let mut incoming_map: HashMap<&str, Vec<(u32, IncomingConnection)>> = HashMap::new();
    let mut outgoing_map: HashMap<(&str, u32), Vec<(&str, u32)>> = HashMap::new();

    for line in &effective_lines {
        incoming_map
            .entry(line.dest_id.as_str())
            .or_default()
            .push((
                line.dest_inlet,
                IncomingConnection {
                    source_id: line.source_id.clone(),
                    source_outlet: line.source_outlet,
                },
            ));
        outgoing_map
            .entry((line.source_id.as_str(), line.source_outlet))
            .or_default()
            .push((line.dest_id.as_str(), line.dest_inlet));
    }

    // Step 4b: Build comment list with Y positions for proximity placement
    let comments: Vec<CommentInfo> = comment_boxes
        .iter()
        .filter_map(|b| {
            let text = b.text.as_deref()?;
            if text.is_empty() {
                return None;
            }
            Some(CommentInfo {
                text: text.to_string(),
                rect: b.patching_rect,
                y_position: b.patching_rect[1],
            })
        })
        .collect();

    // Step 4b2: Filter visual-only boxes from wire candidates and collect them
    let connected_ids: HashSet<&str> = effective_lines
        .iter()
        .flat_map(|l| vec![l.source_id.as_str(), l.dest_id.as_str()])
        .collect();

    let mut panels: Vec<PanelInfo> = Vec::new();
    let mut images: Vec<ImageInfo> = Vec::new();

    wire_candidate_ids.retain(|id| {
        if let Some(b) = box_map.get(id) {
            if is_visual_only_box(b) && !connected_ids.contains(id) {
                // Collect visual element data before filtering out
                match b.maxclass.as_str() {
                    "panel" => {
                        let attrs = build_box_attrs(b);
                        panels.push(PanelInfo {
                            rect: b.patching_rect,
                            attrs,
                        });
                    }
                    "fpic" => {
                        let pic = b.text.clone().unwrap_or_default();
                        images.push(ImageInfo {
                            rect: b.patching_rect,
                            pic,
                        });
                    }
                    "swatch" => {
                        // Treat swatch like panel (decorative)
                        let attrs = build_box_attrs(b);
                        panels.push(PanelInfo {
                            rect: b.patching_rect,
                            attrs,
                        });
                    }
                    _ => {}
                }
                return false; // Exclude from wires
            }
        }
        true
    });

    // Step 4c: Build message declarations and register message box wire names
    // Message boxes participate in connections (they have inlets and outlets),
    // so they need wire names for reference by other nodes.
    let mut msg_wire_names: HashMap<&str, String> = HashMap::new();
    let mut msg_used_names: HashSet<String> = HashSet::new();
    let mut messages: Vec<MsgInfo> = Vec::new();
    let mut ui_entries: Vec<UiEntryInfo> = Vec::new();

    for b in &message_boxes {
        let content = b.text.as_deref().unwrap_or("").to_string();
        let mut name = if let Some(ref vn) = b.varname {
            sanitize_name(vn)
        } else {
            let n = infer_msg_name(&content);
            sanitize_name_lower(&n)
        };
        // Deduplicate: append _2, _3 etc. if name already used
        if msg_used_names.contains(&name) {
            let base = name.clone();
            let mut suffix = 2u32;
            loop {
                name = format!("{}_{}", base, suffix);
                if !msg_used_names.contains(&name) {
                    break;
                }
                suffix += 1;
            }
        }
        msg_used_names.insert(name.clone());
        msg_wire_names.insert(b.id.as_str(), name.clone());
        let (functional_attrs, decorative_attrs) = build_box_attrs_split(b);
        messages.push(MsgInfo {
            name: name.clone(),
            content,
            attrs: functional_attrs,
        });
        ui_entries.push(UiEntryInfo {
            name: name.clone(),
            rect: b.patching_rect,
            decorative_attrs,
        });
    }

    // Step 5: Topological sort of wire candidates
    let sorted_ids = topological_sort(
        &wire_candidate_ids,
        &incoming_map,
        &trigger_ids,
        &effective_lines,
        &box_map,
    )?;

    // Step 6: Build wire names
    // Use the same names as build_in_decls() to ensure consistency between
    // port declarations and wire expression references.
    let inlet_id_to_name: HashMap<&str, String> = inlet_boxes
        .iter()
        .enumerate()
        .map(|(i, b)| (b.id.as_str(), in_decls[i].name.clone()))
        .collect();

    let mut wire_names: HashMap<&str, String> = HashMap::new();
    let mut used_names: HashSet<String> = HashSet::new();
    // Pre-reserve inlet and message names to avoid collisions
    for name in inlet_id_to_name.values() {
        used_names.insert(name.clone());
    }
    for name in msg_wire_names.values() {
        used_names.insert(name.clone());
    }

    for id in &sorted_ids {
        if let Some(b) = box_map.get(id) {
            let mut name = if let Some(ref vn) = b.varname {
                sanitize_name(vn)
            } else {
                // Use object name as wire name (strip ~ suffix, lowercase)
                let obj_name = extract_object_name_for_wire(b);
                sanitize_name_lower(&obj_name)
            };
            // Deduplicate: if name is already used, append a numeric suffix
            if used_names.contains(&name) {
                let base = name.clone();
                let mut suffix = 2u32;
                loop {
                    name = format!("{}_{}", base, suffix);
                    if !used_names.contains(&name) {
                        break;
                    }
                    suffix += 1;
                }
            }
            used_names.insert(name.clone());
            wire_names.insert(id, name);
        }
    }

    // Include message box wire names so they can be resolved as sources
    for (id, name) in &msg_wire_names {
        wire_names.insert(id, name.clone());
    }

    // Collect inlet names for $N template argument resolution
    let _in_port_names: Vec<String> = in_decls.iter().map(|d| d.name.clone()).collect();

    // Step 7: Build wire expressions, tracking defined names to handle cycles.
    // When a wire references a source that hasn't been defined yet (back-edge
    // from cycle breaking), substitute with a literal default to avoid E002.
    let mut wires = Vec::new();
    let mut defined_names: HashSet<String> = HashSet::new();
    // Pre-populate with inlet names (always available)
    for name in inlet_id_to_name.values() {
        defined_names.insert(name.clone());
    }
    // Message box names are also always available
    for name in msg_wire_names.values() {
        defined_names.insert(name.clone());
    }

    let mut direct_connections = Vec::new();

    // Build a map of wire name -> numoutlets for multi-outlet qualification
    let mut wire_numoutlets: HashMap<&str, u32> = HashMap::new();
    for id in &sorted_ids {
        if let Some(b) = box_map.get(id) {
            if let Some(name) = wire_names.get(id) {
                wire_numoutlets.insert(name.as_str(), b.numoutlets);
            }
        }
    }
    // Also include message boxes (they have 1 outlet by default)
    for name in msg_wire_names.values() {
        wire_numoutlets.insert(name.as_str(), 1);
    }

    let mut deferred_edges: Vec<(String, String, u32, u32)> = Vec::new(); // (target_wire, source_id, source_outlet, inlet)
    let mut code_files: Vec<(String, String)> = Vec::new();

    for id in &sorted_ids {
        let b = box_map.get(id).unwrap();
        let result = build_wire_expr(
            b,
            &incoming_map,
            &wire_names,
            &inlet_id_to_name,
            &defined_names,
            &wire_numoutlets,
            objdb,
        )?;
        let name = wire_names.get(id).unwrap().clone();
        defined_names.insert(name.clone());

        // Collect fanin extra connections as DirectConnectionInfo
        // (already qualified inside build_wire_expr)
        for (inlet, source_wire) in &result.extra_connections {
            direct_connections.push(DirectConnectionInfo {
                target_wire: name.clone(),
                inlet: *inlet,
                source_wire: source_wire.clone(),
            });
        }

        // Collect deferred back-edges for later resolution
        for (source_id, source_outlet, inlet) in &result.deferred_back_edges {
            deferred_edges.push((name.clone(), source_id.clone(), *source_outlet, *inlet));
        }

        // Extract codebox code to external files
        let (expr, cb_code_file) = extract_codebox_code(b, &name, &result.expr);
        if let Some(cf) = cb_code_file {
            code_files.push(cf);
        }

        let (functional_attrs, decorative_attrs) = build_box_attrs_split(b);
        wires.push(WireInfo {
            name: name.clone(),
            expr,
            attrs: functional_attrs,
        });
        ui_entries.push(UiEntryInfo {
            name,
            rect: b.patching_rect,
            decorative_attrs,
        });
    }

    // Collect UI entries for inlet and outlet boxes
    for (i, b) in inlet_boxes.iter().enumerate() {
        ui_entries.push(UiEntryInfo {
            name: in_decls[i].name.clone(),
            rect: b.patching_rect,
            decorative_attrs: vec![],
        });
    }
    for (i, b) in outlet_boxes.iter().enumerate() {
        ui_entries.push(UiEntryInfo {
            name: out_decls[i].name.clone(),
            rect: b.patching_rect,
            decorative_attrs: vec![],
        });
    }

    // Resolve deferred back-edges: now that all wires are defined,
    // emit them as direct_connection statements.
    for (target_wire, source_id, source_outlet, inlet) in &deferred_edges {
        let source_name = resolve_source_name(source_id, &wire_names, &inlet_id_to_name);
        if source_name.starts_with("unknown_") {
            continue;
        }
        let qualified_name = if *source_outlet > 0 {
            format!("{}.out[{}]", source_name, source_outlet)
        } else {
            source_name
        };
        let qualified_name = qualify_multi_outlet_source(&qualified_name, &wire_numoutlets);
        direct_connections.push(DirectConnectionInfo {
            target_wire: target_wire.clone(),
            inlet: *inlet,
            source_wire: qualified_name,
        });
    }

    // Step 7c: Emit incoming connections to message boxes as direct connections.
    // Message boxes receive messages on their inlets (e.g., bang triggers output,
    // set changes content). These must be emitted as direct_connection statements.
    for b in &message_boxes {
        if let Some(msg_name) = msg_wire_names.get(b.id.as_str()) {
            if let Some(conns) = incoming_map.get(b.id.as_str()) {
                for (inlet_idx, conn) in conns {
                    let source_name =
                        resolve_source_name(&conn.source_id, &wire_names, &inlet_id_to_name);
                    // Skip unknown/undefined sources
                    if source_name.starts_with("unknown_") {
                        continue;
                    }
                    let base_name = source_name.split('.').next().unwrap_or(&source_name);
                    if !defined_names.contains(base_name) {
                        continue;
                    }
                    let qualified_source = if conn.source_outlet > 0 {
                        format!("{}.out[{}]", source_name, conn.source_outlet)
                    } else {
                        source_name
                    };
                    let qualified_source =
                        qualify_multi_outlet_source(&qualified_source, &wire_numoutlets);
                    direct_connections.push(DirectConnectionInfo {
                        target_wire: msg_name.clone(),
                        inlet: *inlet_idx,
                        source_wire: qualified_source,
                    });
                }
            }
        }
    }

    // Step 7b: Refine outlet port types based on connected source.
    // Skip RNBO outlet boxes (outport/out~) and gen~ outlet boxes (out N)
    // where type is already determined from text prefix.
    for (i, ob) in outlet_boxes.iter().enumerate() {
        // RNBO outlets have explicit types from text; don't override them
        let text_prefix = ob
            .text
            .as_deref()
            .and_then(|t| t.split_whitespace().next())
            .unwrap_or("");
        if matches!(text_prefix, "outport" | "out~") {
            continue;
        }
        // gen~ outlets: all I/O is signal rate; type already set in build_out_decls
        if is_gen && text_prefix == "out" {
            continue;
        }
        if let Some(conns) = incoming_map.get(ob.id.as_str()) {
            for (inlet, conn) in conns {
                if *inlet == 0 {
                    // Check if source is a signal object
                    if let Some(src_box) = box_map.get(conn.source_id.as_str()) {
                        let obj_name = src_box
                            .text
                            .as_deref()
                            .unwrap_or("")
                            .split_whitespace()
                            .next()
                            .unwrap_or(&src_box.maxclass);
                        if (obj_name.ends_with('~') || src_box.maxclass.ends_with('~'))
                            && !is_signal_to_control_object(obj_name)
                            && i < out_decls.len()
                        {
                            out_decls[i].port_type = "signal".to_string();
                        }
                    }
                }
            }
        }
    }

    // Step 8: Identify out assignments
    let mut out_assignments = Vec::new();
    for (i, ob) in outlet_boxes.iter().enumerate() {
        if let Some(conns) = incoming_map.get(ob.id.as_str()) {
            for (inlet, conn) in conns {
                if *inlet == 0 {
                    let source_name =
                        resolve_source_name(&conn.source_id, &wire_names, &inlet_id_to_name);
                    // Skip back-edge references from cycle breaking
                    if !defined_names.contains(&source_name) {
                        continue;
                    }
                    // For non-zero source outlets, use .out[N] syntax
                    let qualified_name = if conn.source_outlet > 0 {
                        format!("{}.out[{}]", source_name, conn.source_outlet)
                    } else {
                        source_name
                    };
                    // Qualify with .out[0] if the source has multiple outlets to avoid E020
                    let qualified_name =
                        qualify_multi_outlet_source(&qualified_name, &wire_numoutlets);
                    out_assignments.push(OutAssignInfo {
                        index: i as u32,
                        wire_name: qualified_name,
                    });
                }
            }
        }
    }

    let patcher_rect = maxpat.rect;

    Ok(DecompiledPatch {
        in_decls,
        out_decls,
        comments,
        messages,
        wires,
        out_assignments,
        direct_connections,
        code_files,
        ui_entries,
        patcher_rect,
        panels,
        images,
    })
}

/// If source_wire is a bare name (no `.out[N]` suffix) and the source has multiple
/// outlets, qualify it with `.out[0]` to avoid E020 bare multi-outlet reference errors.
fn qualify_multi_outlet_source(source_wire: &str, wire_numoutlets: &HashMap<&str, u32>) -> String {
    // Already qualified with .out[N]
    if source_wire.contains(".out[") {
        return source_wire.to_string();
    }
    // Check if the source wire name maps to a multi-outlet node
    if let Some(&num_outlets) = wire_numoutlets.get(source_wire) {
        if num_outlets > 1 {
            return format!("{}.out[0]", source_wire);
        }
    }
    source_wire.to_string()
}

/// Recursively analyze a MaxPat, extracting embedded subpatchers as separate files.
///
/// Returns the analyzed main patch and a list of `(name, DecompiledPatch)` pairs
/// for each subpatcher found (including deeply nested ones).
pub fn analyze_recursive(
    maxpat: &MaxPat,
    name: &str,
    objdb: Option<&ObjectDb>,
) -> Result<(DecompiledPatch, Vec<(String, DecompiledPatch)>), DecompileError> {
    static SUB_COUNTER: AtomicU32 = AtomicU32::new(1);

    let mut subpatchers: Vec<(String, DecompiledPatch)> = Vec::new();
    // Map from box id → subpatcher name (for replacing in parent)
    let mut subpatcher_names: HashMap<String, String> = HashMap::new();
    // Map from box id → subpatcher info (counts + inlet names) derived from embedded patcher
    let mut subpatcher_io: HashMap<String, SubpatcherInfo> = HashMap::new();

    // Find subpatcher boxes and recursively analyze them
    for b in &maxpat.boxes {
        if let Some(ref embedded) = b.embedded_patcher {
            let sub_name = extract_subpatcher_name(b, name, &SUB_COUNTER);

            // Count inlets and outlets inside the subpatcher (including RNBO and gen~ ports)
            let is_rnbo_patcher = embedded.classnamespace.as_deref() == Some("rnbo");
            let is_gen_patcher = embedded.classnamespace.as_deref() == Some("dsp.gen");
            let inlet_count = embedded
                .boxes
                .iter()
                .filter(|sb| {
                    sb.maxclass == "inlet"
                        || sb.maxclass == "inlet~"
                        || is_rnbo_inlet(sb, is_rnbo_patcher)
                        || is_gen_inlet(sb, is_gen_patcher)
                })
                .count() as u32;
            let outlet_count = embedded
                .boxes
                .iter()
                .filter(|sb| {
                    sb.maxclass == "outlet"
                        || sb.maxclass == "outlet~"
                        || is_rnbo_outlet(sb, is_rnbo_patcher)
                        || is_gen_outlet(sb, is_gen_patcher)
                })
                .count() as u32;

            // Recursive call for nested subpatchers
            let (sub_patch, nested_subs) = analyze_recursive(embedded, &sub_name, objdb)?;
            subpatcher_names.insert(b.id.clone(), sub_name.clone());
            let inlet_names: Vec<String> =
                sub_patch.in_decls.iter().map(|d| d.name.clone()).collect();
            subpatcher_io.insert(
                b.id.clone(),
                SubpatcherInfo {
                    inlet_count,
                    outlet_count,
                    inlet_names,
                },
            );
            subpatchers.push((sub_name, sub_patch));
            subpatchers.extend(nested_subs);
        }
    }

    // Analyze the main patcher with subpatcher boxes treated as abstraction refs
    let patch = analyze_with_subpatchers(maxpat, &subpatcher_names, &subpatcher_io, objdb)?;

    Ok((patch, subpatchers))
}

/// Extract a name for a subpatcher from its box definition.
///
/// - `text: "p myname"` → `"myname"`
/// - `text: "poly~ myname 4"` → `"myname"`
/// - `text: "pfft~ myname 1024"` → `"myname"`
/// - `maxclass: "bpatcher"` with `name: "foo.maxpat"` → `"foo"`
/// - Fallback: `"sub_N"` with incrementing counter
fn extract_subpatcher_name(b: &MaxBox, parent_name: &str, counter: &AtomicU32) -> String {
    // Sanitize parent name to ensure valid identifier characters
    let safe_parent = sanitize_name(parent_name);

    // Try to get name from text field
    if let Some(ref text) = b.text {
        let parts: Vec<&str> = text.split_whitespace().collect();
        if parts.len() >= 2 {
            let prefix = parts[0];
            if prefix == "p"
                || prefix == "patcher"
                || prefix == "poly~"
                || prefix == "pfft~"
                || prefix == "rnbo~"
            {
                let raw = parts[1];
                // Strip file extension if present
                let name = raw.strip_suffix(".maxpat").unwrap_or(raw);
                return format!("{}_{}", safe_parent, sanitize_name(name));
            }
        }
    }

    // Try varname
    if let Some(ref vn) = b.varname {
        return format!("{}_{}", safe_parent, sanitize_name(vn));
    }

    // For bpatcher, check if there's a name in the text
    if b.maxclass == "bpatcher" {
        if let Some(ref text) = b.text {
            let name = text.strip_suffix(".maxpat").unwrap_or(text);
            if !name.is_empty() {
                return format!("{}_{}", safe_parent, sanitize_name(name));
            }
        }
    }

    // Fallback: generate a numbered name
    let n = counter.fetch_add(1, Ordering::SeqCst);
    format!("{}_sub_{}", safe_parent, n)
}

/// Analyze a MaxPat where subpatcher boxes are treated as abstraction references.
///
/// Subpatcher boxes (identified by having entries in `subpatcher_names`) are included
/// as wire candidates with their object name set to the extracted subpatcher name.
fn analyze_with_subpatchers(
    maxpat: &MaxPat,
    subpatcher_names: &HashMap<String, String>,
    subpatcher_io: &HashMap<String, SubpatcherInfo>,
    objdb: Option<&ObjectDb>,
) -> Result<DecompiledPatch, DecompileError> {
    let box_map: HashMap<&str, &MaxBox> = maxpat.boxes.iter().map(|b| (b.id.as_str(), b)).collect();

    // Step 1: Remove trigger nodes and rewire connections
    let trigger_result = remove_triggers(maxpat, &box_map);
    let mut effective_lines = trigger_result.lines;
    let trigger_ids = trigger_result.trigger_ids;

    // Step 1b: Sort fan-out destinations by Max execution order.
    sort_fanout_lines(
        &mut effective_lines,
        &box_map,
        &trigger_result.trigger_ordered_sources,
    );

    // Step 2: Classify boxes
    let is_rnbo = maxpat.classnamespace.as_deref() == Some("rnbo");
    let is_gen = maxpat.classnamespace.as_deref() == Some("dsp.gen");
    let mut inlet_boxes: Vec<&MaxBox> = Vec::new();
    let mut outlet_boxes: Vec<&MaxBox> = Vec::new();
    let mut comment_boxes: Vec<&MaxBox> = Vec::new();
    let mut message_boxes: Vec<&MaxBox> = Vec::new();
    let mut wire_candidate_ids: Vec<&str> = Vec::new();

    for b in &maxpat.boxes {
        match b.maxclass.as_str() {
            "inlet" | "inlet~" => inlet_boxes.push(b),
            "outlet" | "outlet~" => outlet_boxes.push(b),
            "comment" => comment_boxes.push(b),
            "message" => message_boxes.push(b),
            _ if is_rnbo_inlet(b, is_rnbo) => inlet_boxes.push(b),
            _ if is_rnbo_outlet(b, is_rnbo) => outlet_boxes.push(b),
            _ if is_gen_inlet(b, is_gen) => inlet_boxes.push(b),
            _ if is_gen_outlet(b, is_gen) => outlet_boxes.push(b),
            _ => {
                if trigger_ids.contains(b.id.as_str()) {
                    // skip triggers
                } else if subpatcher_names.contains_key(&b.id) {
                    // Subpatcher box → treat as wire candidate (abstraction ref)
                    wire_candidate_ids.push(&b.id);
                } else {
                    wire_candidate_ids.push(&b.id);
                }
            }
        }
    }

    // Step 3: Build in/out declarations
    // For gen~/RNBO, sort by text number (in 1, in 2, out 1, out 2).
    // For standard Max, sort by X coordinate (left to right = port index).
    if is_gen || is_rnbo {
        let text_num = |b: &&MaxBox| -> u32 {
            b.text
                .as_deref()
                .and_then(|t| t.split_whitespace().nth(1))
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or(0)
        };
        inlet_boxes.sort_by_key(text_num);
        outlet_boxes.sort_by_key(text_num);
    } else {
        inlet_boxes.sort_by(|a, b| {
            a.patching_rect_x()
                .partial_cmp(&b.patching_rect_x())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        outlet_boxes.sort_by(|a, b| {
            a.patching_rect_x()
                .partial_cmp(&b.patching_rect_x())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    let in_decls = build_in_decls(&inlet_boxes);
    let mut out_decls = build_out_decls(&outlet_boxes);

    // Step 4: Build connection maps
    let mut incoming_map: HashMap<&str, Vec<(u32, IncomingConnection)>> = HashMap::new();
    let mut outgoing_map: HashMap<(&str, u32), Vec<(&str, u32)>> = HashMap::new();

    for line in &effective_lines {
        incoming_map
            .entry(line.dest_id.as_str())
            .or_default()
            .push((
                line.dest_inlet,
                IncomingConnection {
                    source_id: line.source_id.clone(),
                    source_outlet: line.source_outlet,
                },
            ));
        outgoing_map
            .entry((line.source_id.as_str(), line.source_outlet))
            .or_default()
            .push((line.dest_id.as_str(), line.dest_inlet));
    }

    // Step 4b: Build comment list with Y positions for proximity placement
    let comments: Vec<CommentInfo> = comment_boxes
        .iter()
        .filter_map(|b| {
            let text = b.text.as_deref()?;
            if text.is_empty() {
                return None;
            }
            Some(CommentInfo {
                text: text.to_string(),
                rect: b.patching_rect,
                y_position: b.patching_rect[1],
            })
        })
        .collect();

    // Step 4b2: Filter visual-only boxes from wire candidates and collect them
    let connected_ids: HashSet<&str> = effective_lines
        .iter()
        .flat_map(|l| vec![l.source_id.as_str(), l.dest_id.as_str()])
        .collect();

    let mut panels: Vec<PanelInfo> = Vec::new();
    let mut images: Vec<ImageInfo> = Vec::new();

    wire_candidate_ids.retain(|id| {
        if let Some(b) = box_map.get(id) {
            if is_visual_only_box(b) && !connected_ids.contains(id) {
                match b.maxclass.as_str() {
                    "panel" => {
                        let attrs = build_box_attrs(b);
                        panels.push(PanelInfo {
                            rect: b.patching_rect,
                            attrs,
                        });
                    }
                    "fpic" => {
                        let pic = b.text.clone().unwrap_or_default();
                        images.push(ImageInfo {
                            rect: b.patching_rect,
                            pic,
                        });
                    }
                    "swatch" => {
                        let attrs = build_box_attrs(b);
                        panels.push(PanelInfo {
                            rect: b.patching_rect,
                            attrs,
                        });
                    }
                    _ => {}
                }
                return false;
            }
        }
        true
    });

    // Step 4c: Build message declarations
    let mut msg_wire_names: HashMap<&str, String> = HashMap::new();
    let mut msg_used_names: HashSet<String> = HashSet::new();
    let mut messages: Vec<MsgInfo> = Vec::new();
    let mut ui_entries: Vec<UiEntryInfo> = Vec::new();

    for b in &message_boxes {
        let content = b.text.as_deref().unwrap_or("").to_string();
        let mut name = if let Some(ref vn) = b.varname {
            sanitize_name(vn)
        } else {
            let n = infer_msg_name(&content);
            sanitize_name_lower(&n)
        };
        // Deduplicate: append _2, _3 etc. if name already used
        if msg_used_names.contains(&name) {
            let base = name.clone();
            let mut suffix = 2u32;
            loop {
                name = format!("{}_{}", base, suffix);
                if !msg_used_names.contains(&name) {
                    break;
                }
                suffix += 1;
            }
        }
        msg_used_names.insert(name.clone());
        msg_wire_names.insert(b.id.as_str(), name.clone());
        let (functional_attrs, decorative_attrs) = build_box_attrs_split(b);
        messages.push(MsgInfo {
            name: name.clone(),
            content,
            attrs: functional_attrs,
        });
        ui_entries.push(UiEntryInfo {
            name: name.clone(),
            rect: b.patching_rect,
            decorative_attrs,
        });
    }

    // Step 5: Topological sort of wire candidates
    let sorted_ids = topological_sort(
        &wire_candidate_ids,
        &incoming_map,
        &trigger_ids,
        &effective_lines,
        &box_map,
    )?;

    // Step 6: Build wire names
    // Use the same names as build_in_decls() to ensure consistency between
    // port declarations and wire expression references.
    let inlet_id_to_name: HashMap<&str, String> = inlet_boxes
        .iter()
        .enumerate()
        .map(|(i, b)| (b.id.as_str(), in_decls[i].name.clone()))
        .collect();

    let mut wire_names: HashMap<&str, String> = HashMap::new();
    let mut used_names: HashSet<String> = HashSet::new();
    for name in inlet_id_to_name.values() {
        used_names.insert(name.clone());
    }
    for name in msg_wire_names.values() {
        used_names.insert(name.clone());
    }

    for id in &sorted_ids {
        if let Some(b) = box_map.get(id) {
            let mut name = if let Some(ref vn) = b.varname {
                sanitize_name(vn)
            } else {
                // Use object name as wire name (strip ~ suffix, lowercase)
                let obj_name = extract_object_name_for_wire(b);
                sanitize_name_lower(&obj_name)
            };
            // Deduplicate: if name is already used, append a numeric suffix
            if used_names.contains(&name) {
                let base = name.clone();
                let mut suffix = 2u32;
                loop {
                    name = format!("{}_{}", base, suffix);
                    if !used_names.contains(&name) {
                        break;
                    }
                    suffix += 1;
                }
            }
            used_names.insert(name.clone());
            wire_names.insert(id, name);
        }
    }

    // Include message box wire names
    for (id, name) in &msg_wire_names {
        wire_names.insert(id, name.clone());
    }

    let _in_port_names: Vec<String> = in_decls.iter().map(|d| d.name.clone()).collect();

    // Step 7: Build wire expressions, tracking defined names to handle cycles.
    let mut wires = Vec::new();
    let mut defined_names: HashSet<String> = HashSet::new();
    for name in inlet_id_to_name.values() {
        defined_names.insert(name.clone());
    }
    for name in msg_wire_names.values() {
        defined_names.insert(name.clone());
    }

    let mut direct_connections = Vec::new();

    // Build a map of wire name -> numoutlets for multi-outlet qualification
    let mut wire_numoutlets: HashMap<&str, u32> = HashMap::new();
    for id in &sorted_ids {
        if let Some(b) = box_map.get(id) {
            if let Some(name) = wire_names.get(id) {
                wire_numoutlets.insert(name.as_str(), b.numoutlets);
            }
        }
    }
    for name in msg_wire_names.values() {
        wire_numoutlets.insert(name.as_str(), 1);
    }

    // For subpatcher boxes, override the object name with the subpatcher name
    let mut deferred_edges: Vec<(String, String, u32, u32)> = Vec::new();
    let mut code_files: Vec<(String, String)> = Vec::new();

    for id in &sorted_ids {
        let b = box_map.get(id).unwrap();

        let (expr, extra_conns, deferred) = if let Some(sub_name) = subpatcher_names.get(*id) {
            // Build expression using the subpatcher name as the object name
            let (e, sub_extra) = build_subpatcher_wire_expr(
                b,
                sub_name,
                subpatcher_io.get(*id),
                &incoming_map,
                &wire_names,
                &inlet_id_to_name,
                &defined_names,
            )?;
            (e, sub_extra, Vec::new())
        } else {
            let result = build_wire_expr(
                b,
                &incoming_map,
                &wire_names,
                &inlet_id_to_name,
                &defined_names,
                &wire_numoutlets,
                objdb,
            )?;
            (
                result.expr,
                result.extra_connections,
                result.deferred_back_edges,
            )
        };

        let name = wire_names.get(id).unwrap().clone();
        defined_names.insert(name.clone());

        // Collect fanin extra connections as DirectConnectionInfo
        // (already qualified inside build_wire_expr)
        for (inlet, source_wire) in &extra_conns {
            direct_connections.push(DirectConnectionInfo {
                target_wire: name.clone(),
                inlet: *inlet,
                source_wire: source_wire.clone(),
            });
        }

        // Collect deferred back-edges for later resolution
        for (source_id, source_outlet, inlet) in &deferred {
            deferred_edges.push((name.clone(), source_id.clone(), *source_outlet, *inlet));
        }

        // Extract codebox code to external files
        let (expr, cb_code_file) = extract_codebox_code(b, &name, &expr);
        if let Some(cf) = cb_code_file {
            code_files.push(cf);
        }

        let (functional_attrs, decorative_attrs) = build_box_attrs_split(b);
        wires.push(WireInfo {
            name: name.clone(),
            expr,
            attrs: functional_attrs,
        });
        ui_entries.push(UiEntryInfo {
            name,
            rect: b.patching_rect,
            decorative_attrs,
        });
    }

    // Collect UI entries for inlet and outlet boxes
    for (i, b) in inlet_boxes.iter().enumerate() {
        ui_entries.push(UiEntryInfo {
            name: in_decls[i].name.clone(),
            rect: b.patching_rect,
            decorative_attrs: vec![],
        });
    }
    for (i, b) in outlet_boxes.iter().enumerate() {
        ui_entries.push(UiEntryInfo {
            name: out_decls[i].name.clone(),
            rect: b.patching_rect,
            decorative_attrs: vec![],
        });
    }

    // Resolve deferred back-edges
    for (target_wire, source_id, source_outlet, inlet) in &deferred_edges {
        let source_name = resolve_source_name(source_id, &wire_names, &inlet_id_to_name);
        if source_name.starts_with("unknown_") {
            continue;
        }
        let qualified_name = if *source_outlet > 0 {
            format!("{}.out[{}]", source_name, source_outlet)
        } else {
            source_name
        };
        let qualified_name = qualify_multi_outlet_source(&qualified_name, &wire_numoutlets);
        direct_connections.push(DirectConnectionInfo {
            target_wire: target_wire.clone(),
            inlet: *inlet,
            source_wire: qualified_name,
        });
    }

    // Step 7c: Emit incoming connections to message boxes as direct connections.
    for b in &message_boxes {
        if let Some(msg_name) = msg_wire_names.get(b.id.as_str()) {
            if let Some(conns) = incoming_map.get(b.id.as_str()) {
                for (inlet_idx, conn) in conns {
                    let source_name =
                        resolve_source_name(&conn.source_id, &wire_names, &inlet_id_to_name);
                    if source_name.starts_with("unknown_") {
                        continue;
                    }
                    let base_name = source_name.split('.').next().unwrap_or(&source_name);
                    if !defined_names.contains(base_name) {
                        continue;
                    }
                    let qualified_source = if conn.source_outlet > 0 {
                        format!("{}.out[{}]", source_name, conn.source_outlet)
                    } else {
                        source_name
                    };
                    let qualified_source =
                        qualify_multi_outlet_source(&qualified_source, &wire_numoutlets);
                    direct_connections.push(DirectConnectionInfo {
                        target_wire: msg_name.clone(),
                        inlet: *inlet_idx,
                        source_wire: qualified_source,
                    });
                }
            }
        }
    }

    // Step 7b: Refine outlet port types based on connected source.
    // Skip RNBO outlet boxes (outport/out~) and gen~ outlet boxes (out N)
    // where type is already determined from text prefix.
    for (i, ob) in outlet_boxes.iter().enumerate() {
        let text_prefix = ob
            .text
            .as_deref()
            .and_then(|t| t.split_whitespace().next())
            .unwrap_or("");
        if matches!(text_prefix, "outport" | "out~") {
            continue;
        }
        // gen~ outlets: all I/O is signal rate; type already set in build_out_decls
        if is_gen && text_prefix == "out" {
            continue;
        }
        if let Some(conns) = incoming_map.get(ob.id.as_str()) {
            for (inlet, conn) in conns {
                if *inlet == 0 {
                    if let Some(src_box) = box_map.get(conn.source_id.as_str()) {
                        let obj_name = src_box
                            .text
                            .as_deref()
                            .unwrap_or("")
                            .split_whitespace()
                            .next()
                            .unwrap_or(&src_box.maxclass);
                        if (obj_name.ends_with('~') || src_box.maxclass.ends_with('~'))
                            && !is_signal_to_control_object(obj_name)
                            && i < out_decls.len()
                        {
                            out_decls[i].port_type = "signal".to_string();
                        }
                    }
                }
            }
        }
    }

    // Step 8: Identify out assignments
    let mut out_assignments = Vec::new();
    for (i, ob) in outlet_boxes.iter().enumerate() {
        if let Some(conns) = incoming_map.get(ob.id.as_str()) {
            for (inlet, conn) in conns {
                if *inlet == 0 {
                    let source_name =
                        resolve_source_name(&conn.source_id, &wire_names, &inlet_id_to_name);
                    if !defined_names.contains(&source_name) {
                        continue;
                    }
                    let qualified_name = if conn.source_outlet > 0 {
                        format!("{}.out[{}]", source_name, conn.source_outlet)
                    } else {
                        source_name
                    };
                    // Qualify with .out[0] if the source has multiple outlets to avoid E020
                    let qualified_name =
                        qualify_multi_outlet_source(&qualified_name, &wire_numoutlets);
                    out_assignments.push(OutAssignInfo {
                        index: i as u32,
                        wire_name: qualified_name,
                    });
                }
            }
        }
    }

    let patcher_rect = maxpat.rect;

    Ok(DecompiledPatch {
        in_decls,
        out_decls,
        comments,
        messages,
        wires,
        out_assignments,
        direct_connections,
        code_files,
        ui_entries,
        patcher_rect,
        panels,
        images,
    })
}

/// Build a wire expression for a subpatcher box (treated as an abstraction call).
///
/// Uses the extracted subpatcher name as the object name and builds the argument
/// list from incoming connections (same as regular wire expressions).
fn build_subpatcher_wire_expr(
    node: &MaxBox,
    sub_name: &str,
    sub_info: Option<&SubpatcherInfo>,
    incoming_map: &HashMap<&str, Vec<(u32, IncomingConnection)>>,
    wire_names: &HashMap<&str, String>,
    inlet_names: &HashMap<&str, String>,
    defined_names: &HashSet<String>,
) -> Result<(String, Vec<(u32, String)>), DecompileError> {
    // Get incoming connections indexed by inlet number.
    // Skip connections whose source wire hasn't been defined yet (back-edges
    // from cycle breaking) to prevent E002 undefined reference errors.
    let mut inlet_connections: HashMap<u32, String> = HashMap::new();
    if let Some(conns) = incoming_map.get(node.id.as_str()) {
        for (inlet_idx, conn) in conns {
            let source_name = resolve_source_name(&conn.source_id, wire_names, inlet_names);
            let base_name = source_name.split('.').next().unwrap_or(&source_name);
            if !defined_names.contains(base_name) && !source_name.starts_with("unknown_") {
                // Back-edge from cycle breaking — skip this connection
                continue;
            }
            // Skip unknown sources entirely — they reference removed nodes
            if source_name.starts_with("unknown_") {
                continue;
            }
            let qualified_name = if conn.source_outlet > 0 {
                format!("{}.out[{}]", source_name, conn.source_outlet)
            } else {
                source_name
            };
            inlet_connections.insert(*inlet_idx, qualified_name);
        }
    }

    // Use the subpatcher's actual inlet count if available, else fall back to numinlets
    let _num_inlets = sub_info.map_or(node.numinlets as usize, |info| info.inlet_count as usize);

    // Extract any text arguments (e.g., from "poly~ name 4" → ["4"])
    let text_args: Vec<String> = if let Some(ref text) = node.text {
        let parts: Vec<&str> = text.split_whitespace().collect();
        // Skip the object name and subpatcher name; stop at @attributes
        if parts.len() > 2 {
            parts[2..]
                .iter()
                .take_while(|p| !p.starts_with('@'))
                .filter(|p| !p.starts_with('$') && !p.starts_with('#'))
                .map(|s| sanitize_arg(s))
                .collect()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    let mut extra_connections: Vec<(u32, String)> = Vec::new();

    // Default value preservation: when text args exist AND any of their
    // original positions also has a connection, preserve the literals as
    // function args and move ALL connections to extra_connections.
    if !text_args.is_empty() {
        let has_overlap = (0..text_args.len()).any(|i| inlet_connections.contains_key(&(i as u32)));
        if has_overlap {
            let expr = format!("{}({})", sub_name, text_args.join(", "));
            for (inlet_idx, connected_name) in inlet_connections.drain() {
                extra_connections.push((inlet_idx, connected_name));
            }
            return Ok((expr, extra_connections));
        }
    }

    // Build args with strict positional mapping (same as build_wire_expr)
    let max_connected_inlet = inlet_connections.keys().copied().max().unwrap_or(0) as usize;
    let mut args: Vec<String> = Vec::new();

    let fill_to = if inlet_connections.is_empty() {
        0
    } else {
        max_connected_inlet + 1
    };

    for i in 0..fill_to {
        if let Some(connected_name) = inlet_connections.get(&(i as u32)) {
            args.push(connected_name.clone());
        } else {
            // Gap before last connected inlet — fill with default
            args.push("0".to_string());
        }
    }

    // Number of inlet-mapped args (before text args)
    let inlet_arg_count = args.len();

    // Append text arguments (e.g., voice count for poly~)
    args.extend(text_args);

    // Annotate with subpatcher inlet names
    if let Some(info) = sub_info {
        annotate_subpatcher_named_args(&mut args, &info.inlet_names, inlet_arg_count);
    }

    let expr = if args.is_empty() {
        format!("{}()", sub_name)
    } else {
        format!("{}({})", sub_name, args.join(", "))
    };

    Ok((expr, extra_connections))
}

/// Remove trigger nodes and rewire their connections.
///
/// For a trigger node: its input source is connected directly to each of its
/// output destinations. The ordering follows the trigger's outlet numbering
/// in reverse (outlet N-1 first, outlet 0 last) to match Max's right-to-left
/// execution order, which maps to flutmax's top-to-bottom order.
///
/// Chained triggers (trigger A → trigger B → dest) are handled by tracing
/// back through the trigger chain to find the original non-trigger source.
/// Result of trigger removal, including the set of (source_id, source_outlet) pairs
/// whose fan-out order was explicitly defined by trigger outlet indices.
/// These pairs should NOT be re-sorted by coordinates since their order is already correct.
struct TriggerRemovalResult<'a> {
    lines: Vec<MaxLine>,
    trigger_ids: HashSet<&'a str>,
    /// Fan-out source outlets whose order was determined by trigger outlet indices.
    trigger_ordered_sources: HashSet<(String, u32)>,
}

fn remove_triggers<'a>(
    maxpat: &'a MaxPat,
    box_map: &HashMap<&str, &MaxBox>,
) -> TriggerRemovalResult<'a> {
    // Identify value-preserving trigger node IDs (safe to remove).
    // Triggers with bang outlets or literal args are kept as regular objects.
    let trigger_ids: HashSet<&str> = maxpat
        .boxes
        .iter()
        .filter(|b| is_value_preserving_trigger(b))
        .map(|b| b.id.as_str())
        .collect();

    if trigger_ids.is_empty() {
        return TriggerRemovalResult {
            lines: maxpat.lines.clone(),
            trigger_ids,
            trigger_ordered_sources: HashSet::new(),
        };
    }

    // Build maps for trigger inputs and outputs from ALL connections
    // trigger_input_sources: trigger_id -> Vec<(source_id, source_outlet)>
    let mut trigger_input_sources: HashMap<&str, Vec<(String, u32)>> = HashMap::new();
    // trigger_output_dests: (trigger_id, outlet) -> Vec<(dest_id, dest_inlet)>
    let mut trigger_output_dests: HashMap<(&str, u32), Vec<(String, u32)>> = HashMap::new();
    let mut non_trigger_lines: Vec<MaxLine> = Vec::new();

    for line in &maxpat.lines {
        let dest_is_trigger = trigger_ids.contains(line.dest_id.as_str());
        let source_is_trigger = trigger_ids.contains(line.source_id.as_str());

        if dest_is_trigger && line.dest_inlet == 0 {
            // Only inlet 0 is the "hot" input for triggers
            trigger_input_sources
                .entry(line.dest_id.as_str())
                .or_default()
                .push((line.source_id.clone(), line.source_outlet));
        }
        if source_is_trigger {
            trigger_output_dests
                .entry((line.source_id.as_str(), line.source_outlet))
                .or_default()
                .push((line.dest_id.clone(), line.dest_inlet));
        }
        if !dest_is_trigger && !source_is_trigger {
            non_trigger_lines.push(line.clone());
        }
    }

    // Resolve the ultimate non-trigger source(s) for a trigger by following
    // the chain of trigger inputs. Guard against cycles with a depth limit.
    fn resolve_trigger_sources(
        trigger_id: &str,
        trigger_input_sources: &HashMap<&str, Vec<(String, u32)>>,
        trigger_ids: &HashSet<&str>,
        depth: usize,
    ) -> Vec<(String, u32)> {
        if depth > 20 {
            return Vec::new(); // Safety: avoid infinite loops
        }
        let mut results = Vec::new();
        if let Some(inputs) = trigger_input_sources.get(trigger_id) {
            for (src_id, src_outlet) in inputs {
                if trigger_ids.contains(src_id.as_str()) {
                    // Source is another trigger — recurse
                    results.extend(resolve_trigger_sources(
                        src_id,
                        trigger_input_sources,
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

    // For each trigger, create direct connections from ultimate source to destinations.
    // Max trigger outlets fire right-to-left (highest outlet index first).
    // This maps to flutmax's top-to-bottom execution order.
    let mut rewired_lines = non_trigger_lines;
    let mut trigger_ordered_sources: HashSet<(String, u32)> = HashSet::new();

    for trigger_id in &trigger_ids {
        // Get all non-trigger source(s) by chaining through trigger inputs
        let ultimate_sources =
            resolve_trigger_sources(trigger_id, &trigger_input_sources, &trigger_ids, 0);

        // Collect all outlets for this trigger
        let trigger_box = box_map.get(trigger_id);
        let num_outlets = trigger_box.map_or(2, |b| b.numoutlets);

        for (src_id, src_outlet) in &ultimate_sources {
            // Collect and sort outputs by outlet index in REVERSE order (highest first)
            // because Max trigger fires right-to-left (outlet N-1, N-2, ..., 0)
            let mut all_outputs: Vec<(u32, String, u32)> = Vec::new();
            for outlet_idx in 0..num_outlets {
                if let Some(dests) = trigger_output_dests.get(&(*trigger_id, outlet_idx)) {
                    for (dest_id, dest_inlet) in dests {
                        // Skip destinations that are also triggers — they will be
                        // resolved in their own iteration
                        if trigger_ids.contains(dest_id.as_str()) {
                            continue;
                        }
                        all_outputs.push((outlet_idx, dest_id.clone(), *dest_inlet));
                    }
                }
            }
            all_outputs.sort_by(|a, b| b.0.cmp(&a.0));

            // Track that this source outlet has trigger-defined ordering
            if all_outputs.len() > 1 {
                trigger_ordered_sources.insert((src_id.clone(), *src_outlet));
            }

            for (_outlet_idx, dest_id, dest_inlet) in all_outputs {
                rewired_lines.push(MaxLine {
                    source_id: src_id.clone(),
                    source_outlet: *src_outlet,
                    dest_id,
                    dest_inlet,
                    order: None,
                });
            }
        }
    }

    // Deduplicate rewired lines: trigger removal can create duplicate connections
    // when multiple trigger outlets go to the same destination (e.g., `t 1 l 0`
    // with outlets 0 and 2 both going to gate:0). Also deduplicate any overlaps
    // between original non-trigger lines and rewired trigger bypass lines.
    {
        let mut seen = HashSet::new();
        rewired_lines.retain(|line| {
            seen.insert((
                line.source_id.clone(),
                line.source_outlet,
                line.dest_id.clone(),
                line.dest_inlet,
            ))
        });
    }

    TriggerRemovalResult {
        lines: rewired_lines,
        trigger_ids,
        trigger_ordered_sources,
    }
}

/// Sort fan-out connections by Max's evaluation order.
///
/// When multiple connections share the same source (same source_id + source_outlet),
/// Max evaluates destinations in coordinate order:
///   1. X descending (right first — "Right to Left")
///   2. Y descending (bottom first — "Bottom to Top") as tiebreaker
///
/// Connections from trigger-ordered sources (whose order was already determined by
/// trigger outlet indices) are NOT re-sorted.
///
/// This stable sort preserves the relative order of connections from different sources.
fn sort_fanout_lines(
    lines: &mut [MaxLine],
    box_map: &HashMap<&str, &MaxBox>,
    trigger_ordered_sources: &HashSet<(String, u32)>,
) {
    lines.sort_by(|a, b| {
        // Only reorder lines that share the same source outlet
        let source_cmp = a
            .source_id
            .cmp(&b.source_id)
            .then(a.source_outlet.cmp(&b.source_outlet));
        if source_cmp != std::cmp::Ordering::Equal {
            return source_cmp;
        }
        // Skip sorting for fan-outs whose order was defined by trigger outlet indices
        let key = (a.source_id.clone(), a.source_outlet);
        if trigger_ordered_sources.contains(&key) {
            return std::cmp::Ordering::Equal;
        }
        // Same source outlet (implicit fan-out): sort by destination box coordinates
        let a_box = box_map.get(a.dest_id.as_str());
        let b_box = box_map.get(b.dest_id.as_str());
        match (a_box, b_box) {
            (Some(ab), Some(bb)) => {
                // X descending (right first), then Y descending (bottom first)
                bb.patching_rect[0]
                    .partial_cmp(&ab.patching_rect[0])
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(
                        bb.patching_rect[1]
                            .partial_cmp(&ab.patching_rect[1])
                            .unwrap_or(std::cmp::Ordering::Equal),
                    )
            }
            _ => std::cmp::Ordering::Equal,
        }
    });
}

fn is_trigger_node(b: &MaxBox) -> bool {
    // Only newobj boxes can be trigger nodes.  Non-newobj boxes (e.g. textbutton
    // with display label "t") must not be mis-classified as triggers.
    if b.maxclass != "newobj" {
        return false;
    }
    b.text.as_deref().is_some_and(|t| {
        t == "trigger" || t.starts_with("trigger ") || t == "t" || t.starts_with("t ")
    })
}

/// Check if a trigger node is value-preserving (all outlets use non-bang types).
///
/// Value-preserving triggers (e.g., `t f f`, `t i a`, `trigger f i l`) can be
/// safely removed during decompilation because they only enforce execution order
/// without altering values. The flutmax compiler will re-insert equivalent
/// `trigger a a...` nodes for fanout ordering.
///
/// Triggers with `b` (bang) outlets, literal-value outlets (`t 5`, `t clear`),
/// or no arguments (default single `b`) are semantically meaningful and must
/// be preserved as regular objects.
fn is_value_preserving_trigger(b: &MaxBox) -> bool {
    if !is_trigger_node(b) {
        return false;
    }
    let text = b.text.as_deref().unwrap_or("");
    let parts: Vec<&str> = text.split_whitespace().collect();
    // No arguments → default single `b` outlet → not value-preserving
    if parts.len() <= 1 {
        return false;
    }
    // All argument types must be value-preserving (non-bang)
    parts[1..]
        .iter()
        .all(|arg| matches!(*arg, "f" | "i" | "l" | "a" | "s"))
}

/// Check if an object name represents a signal-to-control converter.
/// These objects end with ~ (accept signal input) but output control values.
fn is_signal_to_control_object(name: &str) -> bool {
    matches!(
        name,
        "snapshot~"
            | "peakamp~"
            | "zerox~"
            | "thresh~"
            | "edge~"
            | "capture~"
            | "spike~"
            | "fiddle~"
            | "pitch~"
            | "bonk~"
            | "sigmund~"
    )
}

fn build_in_decls(inlet_boxes: &[&MaxBox]) -> Vec<InDeclInfo> {
    inlet_boxes
        .iter()
        .enumerate()
        .map(|(i, b)| {
            // Check for RNBO/gen~ port types from text prefix
            let text_prefix = b
                .text
                .as_deref()
                .and_then(|t| t.split_whitespace().next())
                .unwrap_or("");

            let (port_type, special_name) = match text_prefix {
                "in~" => ("signal".to_string(), None),
                "inport" => {
                    // Extract port name from text (second word)
                    let name = b
                        .text
                        .as_deref()
                        .and_then(|t| t.split_whitespace().nth(1))
                        .map(sanitize_name);
                    ("float".to_string(), name)
                }
                // gen~ inlet: `in N` — all I/O is signal rate
                "in" if b.maxclass == "newobj" => {
                    let idx = b
                        .text
                        .as_deref()
                        .and_then(|t| t.split_whitespace().nth(1))
                        .and_then(|n| n.parse::<u32>().ok())
                        .unwrap_or((i as u32) + 1);
                    let name = format!("gen_in_{}", idx);
                    ("signal".to_string(), Some(name))
                }
                _ => {
                    // Standard inlet: detect type from outlettype
                    let pt = if b.outlettype.iter().any(|t| t == "signal") {
                        "signal".to_string()
                    } else {
                        "float".to_string()
                    };
                    (pt, None)
                }
            };

            let fallback = format!("port_{}", i);
            let name = special_name
                .filter(|n| !n.is_empty())
                .or_else(|| {
                    b.comment.as_deref().and_then(|c| {
                        // Use fallback when the raw comment is empty/whitespace-only.
                        // sanitize_name("") returns "w_unnamed" which would cause
                        // all empty-comment inlets to collide.
                        if c.trim().is_empty() {
                            None
                        } else {
                            let s = sanitize_name(c);
                            if s.is_empty() {
                                None
                            } else {
                                Some(s)
                            }
                        }
                    })
                })
                .unwrap_or(fallback);
            InDeclInfo {
                index: i as u32,
                name,
                port_type,
            }
        })
        .collect()
}

fn build_out_decls(outlet_boxes: &[&MaxBox]) -> Vec<OutDeclInfo> {
    outlet_boxes
        .iter()
        .enumerate()
        .map(|(i, b)| {
            // Check for RNBO/gen~ port types from text prefix
            let text_prefix = b
                .text
                .as_deref()
                .and_then(|t| t.split_whitespace().next())
                .unwrap_or("");

            let (port_type, special_name) = match text_prefix {
                "out~" => ("signal".to_string(), None),
                "outport" => {
                    // Extract port name from text (second word)
                    let name = b
                        .text
                        .as_deref()
                        .and_then(|t| t.split_whitespace().nth(1))
                        .map(sanitize_name);
                    ("float".to_string(), name)
                }
                // gen~ outlet: `out N` — all I/O is signal rate
                "out" if b.maxclass == "newobj" => {
                    let idx = b
                        .text
                        .as_deref()
                        .and_then(|t| t.split_whitespace().nth(1))
                        .and_then(|n| n.parse::<u32>().ok())
                        .unwrap_or((i as u32) + 1);
                    let name = format!("gen_out_{}", idx);
                    ("signal".to_string(), Some(name))
                }
                _ => {
                    // Standard outlet: type defaults to "float".
                    // Will be upgraded to "signal" if a signal-rate source is connected (Phase 2).
                    ("float".to_string(), None)
                }
            };

            let fallback = format!("out_{}", i);
            let name = special_name
                .filter(|n| !n.is_empty())
                .or_else(|| {
                    b.comment.as_deref().and_then(|c| {
                        if c.trim().is_empty() {
                            None
                        } else {
                            let s = sanitize_name(c);
                            if s.is_empty() {
                                None
                            } else {
                                Some(s)
                            }
                        }
                    })
                })
                .unwrap_or(fallback);
            OutDeclInfo {
                index: i as u32,
                name,
                port_type,
            }
        })
        .collect()
}

/// Topological sort of wire candidate nodes based on their dependencies.
///
/// When cycles are detected, back-edges are broken so that all nodes can still
/// be sorted. The broken edge source IDs are returned so the caller can handle
/// them (e.g. by omitting the connection from the wire expression).
///
/// Within the same topological level (nodes that become available simultaneously),
/// nodes are sorted using a fan-out position map derived from `effective_lines`.
/// This respects both trigger-defined order and coordinate-based order for implicit
/// fan-outs, since `effective_lines` has already been sorted appropriately.
fn topological_sort<'a>(
    candidate_ids: &[&'a str],
    incoming_map: &HashMap<&str, Vec<(u32, IncomingConnection)>>,
    _trigger_ids: &HashSet<&str>,
    effective_lines: &[MaxLine],
    box_map: &HashMap<&str, &MaxBox>,
) -> Result<Vec<&'a str>, DecompileError> {
    let candidates: HashSet<&str> = candidate_ids.iter().copied().collect();

    // Build in-degree count for candidates only
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();

    for &id in candidate_ids {
        in_degree.entry(id).or_insert(0);
    }

    for &id in candidate_ids {
        if let Some(conns) = incoming_map.get(id) {
            for (_, conn) in conns {
                let src = conn.source_id.as_str();
                if candidates.contains(src) {
                    *in_degree.entry(id).or_insert(0) += 1;
                    adj.entry(src).or_default().push(id);
                }
            }
        }
    }

    // Build a fan-out position map from effective_lines.
    // For each destination node, record the position (index) in effective_lines where
    // it first appears as a destination. This captures the correct fan-out order
    // (trigger-defined or coordinate-sorted) for tiebreaking in the topological sort.
    let mut fanout_position: HashMap<&str, usize> = HashMap::new();
    for (idx, line) in effective_lines.iter().enumerate() {
        fanout_position.entry(line.dest_id.as_str()).or_insert(idx);
    }

    // Helper: sort node IDs by their fan-out position, then by coordinates as fallback.
    // This ensures that fan-out destinations appear in the order defined by
    // effective_lines (which respects trigger outlet order and coordinate sorting).
    let sort_by_fanout_order = |ids: &mut Vec<&str>| {
        ids.sort_by(|a, b| {
            let a_pos = fanout_position.get(a).copied().unwrap_or(usize::MAX);
            let b_pos = fanout_position.get(b).copied().unwrap_or(usize::MAX);
            a_pos.cmp(&b_pos).then_with(|| {
                // Fallback: coordinate-based order for nodes not in effective_lines
                let a_box = box_map.get(a);
                let b_box = box_map.get(b);
                match (a_box, b_box) {
                    (Some(ab), Some(bb)) => bb.patching_rect[0]
                        .partial_cmp(&ab.patching_rect[0])
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then(
                            bb.patching_rect[1]
                                .partial_cmp(&ab.patching_rect[1])
                                .unwrap_or(std::cmp::Ordering::Equal),
                        ),
                    _ => std::cmp::Ordering::Equal,
                }
            })
        });
    };

    // Kahn's algorithm with fan-out-order tiebreaking
    let mut initial_roots: Vec<&str> = candidate_ids
        .iter()
        .copied()
        .filter(|id| *in_degree.get(id).unwrap_or(&0) == 0)
        .collect();
    sort_by_fanout_order(&mut initial_roots);
    let mut queue: VecDeque<&str> = initial_roots.into_iter().collect();

    let mut sorted = Vec::new();
    while let Some(id) = queue.pop_front() {
        sorted.push(id);
        if let Some(neighbors) = adj.get(id) {
            let mut newly_free: Vec<&str> = Vec::new();
            for &neighbor in neighbors {
                let deg = in_degree.get_mut(neighbor).unwrap();
                *deg -= 1;
                if *deg == 0 {
                    newly_free.push(neighbor);
                }
            }
            // Sort newly freed nodes by fan-out order before adding to queue
            sort_by_fanout_order(&mut newly_free);
            for nid in newly_free {
                queue.push_back(nid);
            }
        }
    }

    if sorted.len() != candidate_ids.len() {
        // Cycle detected — break cycles by repeatedly removing the node with
        // the smallest in-degree among remaining nodes (greedy heuristic).
        let sorted_set: HashSet<&str> = sorted.iter().copied().collect();
        let mut remaining: Vec<&str> = candidate_ids
            .iter()
            .copied()
            .filter(|id| !sorted_set.contains(*id))
            .collect();

        while !remaining.is_empty() {
            // Pick the node with smallest remaining in-degree (break ties by order)
            let mut best_idx = 0;
            let mut best_deg = usize::MAX;
            for (i, &id) in remaining.iter().enumerate() {
                let deg = *in_degree.get(id).unwrap_or(&0);
                if deg < best_deg {
                    best_deg = deg;
                    best_idx = i;
                }
            }

            let id = remaining.remove(best_idx);
            sorted.push(id);

            // Update in-degrees as if this node is now processed
            if let Some(neighbors) = adj.get(id) {
                for &neighbor in neighbors {
                    if let Some(deg) = in_degree.get_mut(neighbor) {
                        if *deg > 0 {
                            *deg -= 1;
                        }
                    }
                }
            }

            // Check if any remaining nodes now have in-degree 0
            let mut newly_free = Vec::new();
            remaining.retain(|&rid| {
                if *in_degree.get(rid).unwrap_or(&0) == 0 {
                    newly_free.push(rid);
                    false
                } else {
                    true
                }
            });

            // Sort newly freed nodes by fan-out order before BFS
            sort_by_fanout_order(&mut newly_free);

            // Process newly freed nodes via normal BFS
            let mut bfs_queue: VecDeque<&str> = newly_free.into_iter().collect();
            while let Some(nid) = bfs_queue.pop_front() {
                sorted.push(nid);
                if let Some(neighbors) = adj.get(nid) {
                    for &neighbor in neighbors {
                        if let Some(deg) = in_degree.get_mut(neighbor) {
                            if *deg > 0 {
                                *deg -= 1;
                            }
                            if *deg == 0 && remaining.contains(&neighbor) {
                                remaining.retain(|&r| r != neighbor);
                                bfs_queue.push_back(neighbor);
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(sorted)
}

/// Normalize an objdb inlet description to a valid flutmax identifier for named args.
///
/// Max refpage digests often include type prefixes like "(signal)" or "(signal/float)".
/// These are stripped before normalization. Long descriptions (> 20 chars after normalization)
/// are rejected to avoid unwieldy argument names.
///
/// Examples:
///   "Frequency" → "frequency"
///   "(signal/float) Cutoff Frequency" → "cutoff_frequency"
///   "Phase (0-1)" → "phase_0_1"
///   "Input Gain (Filter coefficient a0)" → None (too long)
///   "" → None
fn normalize_inlet_name(description: &str) -> Option<String> {
    let trimmed = description.trim();
    // Strip leading type prefix like "(signal)", "(signal/float)", "(float)", "(int)"
    let stripped = if trimmed.starts_with('(') {
        if let Some(end) = trimmed.find(')') {
            trimmed[end + 1..].trim()
        } else {
            trimmed
        }
    } else {
        trimmed
    };

    let s: String = stripped
        .to_lowercase()
        .chars()
        .map(|c| if c == ' ' { '_' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    // Collapse consecutive underscores and strip leading/trailing
    let parts: Vec<&str> = s.split('_').filter(|p| !p.is_empty()).collect();
    let result = parts.join("_");
    // Strip leading digits (identifiers must start with a letter or _)
    let result = result
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .to_string();
    if result.is_empty() || result.len() > 20 {
        None
    } else {
        Some(result)
    }
}

/// Annotate positional args with named arg prefixes from objdb inlet descriptions.
///
/// Only annotates args at positions 0..inlet_arg_count (not structural args).
/// Skips annotation if the object is not in objdb or if any inlet lacks a description.
fn annotate_named_args(args: &mut [String], obj_name: &str, inlet_arg_count: usize, db: &ObjectDb) {
    let def = match db.lookup(obj_name) {
        Some(d) => d,
        None => return,
    };
    let inlets = match &def.inlets {
        InletSpec::Fixed(ports) => ports.as_slice(),
        InletSpec::Variable { .. } => return, // Variable inlets — names unreliable
    };

    // Collect normalized names for all positions we'd annotate.
    // If any inlet lacks a usable name, fall back to all-positional
    // to avoid confusing mixed output.
    let names: Vec<Option<String>> = (0..inlet_arg_count)
        .map(|i| {
            inlets
                .get(i)
                .and_then(|p| normalize_inlet_name(&p.description))
        })
        .collect();

    if names.iter().any(|n| n.is_none()) {
        return; // Some inlet has no description — keep all positional
    }

    // Check for duplicate names (e.g., multiple inlets with "Input")
    let name_strs: Vec<&str> = names.iter().map(|n| n.as_deref().unwrap()).collect();
    let unique: HashSet<&str> = name_strs.iter().copied().collect();
    if unique.len() < name_strs.len() {
        return; // Duplicate names — keep positional to avoid ambiguity
    }

    // All names valid and unique — annotate
    for (i, name) in names.into_iter().enumerate() {
        if i < args.len() {
            if let Some(n) = name {
                args[i] = format!("{}: {}", n, args[i]);
            }
        }
    }
}

/// Annotate positional args with subpatcher inlet names from `in` declarations.
///
/// Only annotates args at positions 0..inlet_arg_count (not text args like voice count).
/// Skips `port_N` fallback names since they carry no semantic meaning.
fn annotate_subpatcher_named_args(
    args: &mut [String],
    inlet_names: &[String],
    inlet_arg_count: usize,
) {
    if inlet_arg_count == 0 || inlet_arg_count > inlet_names.len() {
        return;
    }
    let names = &inlet_names[..inlet_arg_count];

    // Skip if any name is the generic fallback (port_N)
    if names.iter().any(|n| n.starts_with("port_")) {
        return;
    }

    // Check for duplicate names
    let unique: HashSet<&str> = names.iter().map(|s| s.as_str()).collect();
    if unique.len() < names.len() {
        return;
    }

    for (i, name) in names.iter().enumerate() {
        if i < args.len() {
            args[i] = format!("{}: {}", name, args[i]);
        }
    }
}

/// Result of building a wire expression, including extra fanin connections.
struct WireExprResult {
    /// The main wire expression (e.g., `cycle~(440)`)
    expr: String,
    /// Extra connections for fanin (multiple wires to the same inlet)
    extra_connections: Vec<(u32, String)>, // (inlet_index, source_wire_name)
    /// Deferred back-edge connections where the source wire hasn't been defined yet.
    /// These should be emitted as direct_connection statements after all wires are defined.
    /// Format: (source_id, source_outlet, inlet_index)
    deferred_back_edges: Vec<(String, u32, u32)>,
}

/// Build the expression for a wire node.
///
/// `defined_names` contains wire/port names that have been defined so far in the
/// output (i.e., wires that appear earlier in the topological order). References
/// to names NOT in this set are back-edges from cycle breaking and are replaced
/// with literal `0` to avoid E002 undefined reference errors.
///
/// When multiple connections target the same inlet (fanin), the FIRST connection
/// is used in the function call args and the rest are returned as extra connections
/// to be emitted as `direct_connection` statements.
fn build_wire_expr(
    node: &MaxBox,
    incoming_map: &HashMap<&str, Vec<(u32, IncomingConnection)>>,
    wire_names: &HashMap<&str, String>,
    inlet_names: &HashMap<&str, String>,
    defined_names: &HashSet<String>,
    wire_numoutlets: &HashMap<&str, u32>,
    objdb: Option<&ObjectDb>,
) -> Result<WireExprResult, DecompileError> {
    let (obj_name, literal_args) = extract_object_parts(node);
    let flutmax_name = alias::reverse_alias(&obj_name);

    // Process literal args from the Max object text:
    // Template arguments ($1, $f1, $i1, #0, #1, etc.) are preserved as quoted
    // string literals so they survive the roundtrip. The compiler emits them
    // unquoted in the .maxpat text field (LitValue::Str → s.clone()).
    let dummy_names: HashSet<&str> = HashSet::new();
    let literal_args: Vec<String> = literal_args
        .into_iter()
        .map(|arg| sanitize_literal_arg(&arg, &dummy_names))
        .collect();

    // Get incoming connections indexed by inlet number.
    // When source wire hasn't been defined yet (back-edge from cycle breaking),
    // collect it as a deferred back-edge to emit later as a direct_connection.
    //
    // When multiple connections target the same inlet (fanin), the first VALID
    // (non-unknown) connection is used in the function call and the rest are
    // collected as extra_connections.
    let mut inlet_connections: HashMap<u32, String> = HashMap::new();
    let mut extra_connections: Vec<(u32, String)> = Vec::new();
    let mut deferred_back_edges: Vec<(String, u32, u32)> = Vec::new();
    if let Some(conns) = incoming_map.get(node.id.as_str()) {
        for (inlet_idx, conn) in conns {
            let source_name = resolve_source_name(&conn.source_id, wire_names, inlet_names);
            // Check if the source is defined (or is an inlet port, which is always available)
            let base_name = source_name.split('.').next().unwrap_or(&source_name);
            if !defined_names.contains(base_name) && !source_name.starts_with("unknown_") {
                // Back-edge from cycle breaking — defer as direct_connection
                deferred_back_edges.push((conn.source_id.clone(), conn.source_outlet, *inlet_idx));
                continue;
            }
            // Skip unknown sources entirely — they reference removed nodes
            // (e.g., chained triggers not fully rewired)
            if source_name.starts_with("unknown_") {
                continue;
            }
            // For non-zero source outlets, use .out[N] syntax to select the correct outlet
            let qualified_name = if conn.source_outlet > 0 {
                format!("{}.out[{}]", source_name, conn.source_outlet)
            } else {
                source_name
            };
            // Qualify with .out[0] if the source has multiple outlets to avoid E020
            let qualified_name = qualify_multi_outlet_source(&qualified_name, wire_numoutlets);
            // Check for fanin: if this inlet already has a connection, store the extra
            if inlet_connections.contains_key(inlet_idx) {
                extra_connections.push((*inlet_idx, qualified_name));
            } else {
                inlet_connections.insert(*inlet_idx, qualified_name);
            }
        }
    }

    // Codebox early return: if this is a codebox with inline code, generate a
    // placeholder expression. All connections become extra_connections (emitted
    // as .in[N] direct connections) since the codebox expression only takes
    // the filename reference. The actual code extraction happens in the caller.
    if is_codebox_with_code(node) {
        let expr = format!("{}()", flutmax_name);
        for (inlet_idx, connected_name) in inlet_connections.drain() {
            extra_connections.push((inlet_idx, connected_name));
        }
        return Ok(WireExprResult {
            expr,
            extra_connections,
            deferred_back_edges,
        });
    }

    // Determine if this is a pak/pack object whose text args define inlet types
    // (not default values). For pak/pack, args like "f", "i", "s", or numeric
    // defaults are structural: they define the number and type of inlets, and
    // must be preserved in the output text even when inlets are connected.
    let is_pak_object = matches!(obj_name.as_str(), "pak" | "pack");

    // Build argument list with strict positional mapping.
    //
    // In Max, text args (e.g., "0.5" in `*~ 0.5`) set default values for inlets.
    // When inlet 0 has a connection, the text args shift to fill cold inlets.
    // The critical invariant: argument position N in the generated flutmax code
    // maps to inlet N in the Max object. If inlet 0 is unconnected and inlet 1
    // is connected, we must emit a value at position 0 (literal default or "0")
    // so the connection appears at position 1.
    //
    // Exception: pak/pack objects have structural type args that define inlet types.
    // All their literal args stay at their positions, and connections become
    // direct_connection statements instead of replacing args.
    let num_inlets = node.numinlets as usize;

    if is_pak_object && !literal_args.is_empty() {
        // pak/pack: preserve ALL literal args and move ALL connections to extra_connections
        let mut args: Vec<String> = literal_args.clone();
        // Pad to cover all inlets if needed
        while args.len() < num_inlets {
            args.push("0".to_string());
        }
        for (inlet_idx, connected_name) in inlet_connections.drain() {
            extra_connections.push((inlet_idx, connected_name));
        }
        let expr = format!("{}({})", flutmax_name, args.join(", "));
        return Ok(WireExprResult {
            expr,
            extra_connections,
            deferred_back_edges,
        });
    }

    // Default value preservation: when text args exist AND any of their
    // original positions (0..literal_args.len()) also has a connection,
    // preserve the literal defaults as function args and move ALL connections
    // to extra_connections (emitted as .in[N] direct connections).
    // This ensures e.g. `[*~ 0.5]` with both inlets connected produces:
    //   wire w = mul~(0.5);
    //   w.in[0] = osc;
    //   w.in[1] = env;
    //
    // Default value preservation: when text args and connections overlap on the
    // same inlet positions, preserve literals as defaults and emit connections
    // as .in[N].
    // Heuristic to skip Abstraction calls: if literal_args + connections
    // exactly fill all inlets without overlap, the literals are positional
    // call args (not defaults). This catches `simpleFM~(w_5, 1.0, w_10)`.
    let has_real_overlap = !literal_args.is_empty()
        && (0..literal_args.len()).any(|i| inlet_connections.contains_key(&(i as u32)));
    let literals_plus_conns_exact =
        literal_args.len() + inlet_connections.len() == num_inlets && !has_real_overlap;
    if has_real_overlap && !literals_plus_conns_exact {
        let has_overlap =
            (0..literal_args.len()).any(|i| inlet_connections.contains_key(&(i as u32)));
        if has_overlap {
            let expr = format!("{}({})", flutmax_name, literal_args.join(", "));
            // Move ALL connections to extra_connections
            for (inlet_idx, connected_name) in inlet_connections.drain() {
                extra_connections.push((inlet_idx, connected_name));
            }
            return Ok(WireExprResult {
                expr,
                extra_connections,
                deferred_back_edges,
            });
        }
    }

    // Determine the highest inlet index that needs a value (either connected or
    // has a literal), so we know how many positions to fill.
    let max_connected_inlet = inlet_connections.keys().copied().max().unwrap_or(0) as usize;
    let max_needed = if inlet_connections.is_empty() && literal_args.is_empty() {
        0
    } else {
        // We need at least enough positions to cover all connections
        std::cmp::max(
            if inlet_connections.is_empty() {
                0
            } else {
                max_connected_inlet + 1
            },
            // ... and enough to cover literal args for unconnected inlets
            literal_args.len()
                + inlet_connections
                    .keys()
                    .filter(|&&k| (k as usize) < literal_args.len())
                    .count()
                    .min(
                        if inlet_connections.contains_key(&0) && !literal_args.is_empty() {
                            1
                        } else {
                            0
                        },
                    ),
        )
    };

    // Build positional literal defaults: map text args to inlet positions.
    // When inlet 0 has a connection, text args shift to fill cold inlets
    // starting from inlet 1.
    let mut positional_literals: Vec<Option<String>> = vec![None; num_inlets.max(max_needed)];
    let inlet0_connected = inlet_connections.contains_key(&0);

    if inlet0_connected && !literal_args.is_empty() {
        // Text args fill cold inlets (starting from inlet 1).
        // When a cold inlet already has a connection, the literal arg takes
        // priority (it defines the initial/default value in Max) and the
        // connection is moved to extra_connections (direct_connection).
        let mut lit_idx = 0;
        #[allow(clippy::needless_range_loop)]
        for i in 1..positional_literals.len() {
            if lit_idx >= literal_args.len() {
                break;
            }
            positional_literals[i] = Some(literal_args[lit_idx].clone());
            lit_idx += 1;
            // If this inlet also has a connection, move it to extra_connections
            // so the literal arg is preserved in the function call text.
            if let Some(conn) = inlet_connections.remove(&(i as u32)) {
                extra_connections.push((i as u32, conn));
            }
        }
        // Any remaining literals are structural args (appended after inlets)
        // Store them starting from first None position at the end
        for lit in &literal_args[lit_idx..] {
            positional_literals.push(Some(lit.clone()));
        }
    } else {
        // Text args fill inlets from position 0 onwards
        for (i, lit) in literal_args.iter().enumerate() {
            if i < positional_literals.len() {
                positional_literals[i] = Some(lit.clone());
            } else {
                positional_literals.push(Some(lit.clone()));
            }
        }
    }

    // Now build the args array with strict positional mapping
    let mut args: Vec<String> = Vec::new();
    let fill_to = std::cmp::max(
        max_needed,
        positional_literals
            .iter()
            .rposition(|l| l.is_some())
            .map_or(0, |p| p + 1),
    );

    for i in 0..fill_to {
        let inlet_idx = i as u32;
        let has_literal = matches!(positional_literals.get(i), Some(Some(_)));
        let has_connection = inlet_connections.contains_key(&inlet_idx);

        if has_literal && has_connection {
            // Both literal and connection at same position:
            // Prefer the literal (preserves original text arg) and
            // move the connection to extra_connections.
            let lit = positional_literals[i].as_ref().unwrap().clone();
            let conn = inlet_connections.remove(&inlet_idx).unwrap();
            args.push(lit);
            extra_connections.push((inlet_idx, conn));
        } else if let Some(connected_name) = inlet_connections.get(&inlet_idx) {
            args.push(connected_name.clone());
        } else if let Some(Some(lit)) = positional_literals.get(i) {
            args.push(lit.clone());
        } else if i < max_connected_inlet {
            // Gap before the last connected inlet — fill with default
            args.push("0".to_string());
        }
        // After the last connection, if no literal, stop (don't pad)
    }

    // Number of inlet-mapped args (before structural args)
    let inlet_arg_count = args.len();

    // Append structural literal args beyond inlet range
    for i in fill_to..positional_literals.len() {
        if let Some(Some(lit)) = positional_literals.get(i) {
            args.push(lit.clone());
        }
    }

    // Add named arg prefixes from objdb inlet descriptions
    if let Some(db) = objdb {
        annotate_named_args(&mut args, &obj_name, inlet_arg_count, db);
    }

    let expr = if args.is_empty() {
        format!("{}()", flutmax_name)
    } else {
        format!("{}({})", flutmax_name, args.join(", "))
    };

    Ok(WireExprResult {
        expr,
        extra_connections,
        deferred_back_edges,
    })
}

/// Extract the object name and literal arguments from a box's text field.
///
/// - Stops at the first `@` token (Max attributes like `@phase 0.5` are
///   not part of the object's positional arguments).
/// - Normalises trailing-dot floats like `127.` → `127.0`.
/// - Normalises negative numbers like `-50.` → `-50.0`.
///
/// Example: "cycle~ 440" → ("cycle~", ["440"])
/// Example: "*~ 0.5" → ("*~", ["0.5"])
/// Example: "cycle~" → ("cycle~", [])
/// Example: "param foobar @min 0" → ("param", ["foobar"])
/// Example: "pack 0. 100" → ("pack", ["0.0", "100"])
fn extract_object_parts(node: &MaxBox) -> (String, Vec<String>) {
    // For non-newobj boxes (UI elements like textbutton, live.text, live.dial, etc.),
    // the `text` field is a display label, NOT an object specification.
    // The object identity is the maxclass itself.
    if node.maxclass != "newobj" {
        return (node.maxclass.clone(), Vec::new());
    }

    let text = match &node.text {
        Some(t) => t.as_str(),
        None => return (node.maxclass.clone(), Vec::new()),
    };

    // Split respecting quoted strings: expr "out1 = in1 * in2" stays as one arg
    let parts = split_respecting_quotes(text);
    if parts.is_empty() {
        return (node.maxclass.clone(), Vec::new());
    }

    let raw_name = &parts[0];

    // Handle operator-number fusion: "/2" → ("/", ["2"]), "*~0.5" → ("*~", ["0.5"])
    // An operator char(s) followed directly by a digit means the operator is the name
    // and the digit portion is the first argument.
    let (obj_name, extra_arg) = split_operator_number(raw_name);

    // If the text doesn't look like a valid object name, fall back to maxclass.
    // For bare numeric names (e.g., "1", "44100", "1.1666"), use "newobj" as the
    // object name and include the number as a literal argument, matching Max
    // behavior where these are integer/float constant objects.
    let (obj_name, numeric_arg) = if is_valid_object_name(&obj_name) {
        (obj_name, None)
    } else if is_valid_number_token(&obj_name)
        || is_valid_number_token(obj_name.trim_start_matches('-'))
    {
        // Bare numeric object name → newobj + number as arg
        (
            node.maxclass.clone(),
            Some(normalize_number_literal(&obj_name)),
        )
    } else {
        (node.maxclass.clone(), None)
    };

    let mut args: Vec<String> = Vec::new();
    if let Some(num_arg) = numeric_arg {
        args.push(num_arg);
    }
    if let Some(arg) = extra_arg {
        args.push(normalize_number_literal(&arg));
    }
    args.extend(
        parts[1..]
            .iter()
            .take_while(|p| !p.starts_with('@'))
            .map(|s| normalize_number_literal(s.as_str())),
    );
    (obj_name, args)
}

/// Split text by whitespace, but keep quoted strings intact.
/// `expr "out1 = in1 * in2"` → `["expr", "\"out1 = in1 * in2\""]`
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

/// Split an operator-number fusion token like "/2" into ("/" , Some("2")).
///
/// Max allows writing `/ 2` or `/2` — both mean the `/` operator with a default
/// argument of `2`.  This function detects when an operator prefix is fused with
/// a numeric suffix and splits them.
///
/// Returns `(name, optional_arg)`.  When no split is needed, `optional_arg` is
/// `None`.
fn split_operator_number(token: &str) -> (String, Option<String>) {
    // Find the boundary between operator chars and the numeric suffix.
    // Operator chars: * / % + - ! < > = & | ^
    // Also handle tilde: *~0.5 → ("*~", "0.5")
    let op_chars = |c: char| "*/%+-!<>=&|^".contains(c);

    let chars: Vec<char> = token.chars().collect();
    if chars.is_empty() || !op_chars(chars[0]) {
        return (token.to_string(), None);
    }

    // Find where operator chars end
    let mut op_end = 0;
    for (i, &c) in chars.iter().enumerate() {
        if op_chars(c) || c == '~' {
            op_end = i + 1;
        } else {
            break;
        }
    }

    // If operator consumed the whole token, no split needed
    if op_end >= chars.len() {
        return (token.to_string(), None);
    }

    // The rest should start with a digit or '-' (negative) or '.' to be a number
    let rest_start = chars[op_end];
    if rest_start.is_ascii_digit() || rest_start == '.' || rest_start == '-' {
        let op: String = chars[..op_end].iter().collect();
        let arg: String = chars[op_end..].iter().collect();
        (op, Some(arg))
    } else {
        (token.to_string(), None)
    }
}

/// Check if a string looks like a valid flutmax object name.
///
/// Valid names match the grammar: identifiers (including dotted, tilde, hyphenated)
/// and operator names like `*`, `+`, `-`, `/`, `%`, `!`, `<`, `>`, `==`, etc.
/// Dotted names must have valid segments between dots (no `...` or trailing dots).
///
/// Max allows many object name patterns:
/// - Standard identifiers: `cycle~`, `phasor~`, `loadbang`
/// - Dotted names: `jit.gl.render`, `jit.3m`, `jit.*`, `jit.-`
/// - Names starting with digits: `2input-router`
/// - Pure operator names: `*`, `+`, `-`, `/`, `%`, `==`, `!=`
fn is_valid_object_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let is_tilde = name.ends_with('~');
    let check = name.trim_end_matches('~');
    if check.is_empty() {
        return false; // just "~" is not valid
    }

    // Pure operator names: *, +, -, /, %, ==, !=, ?, etc.
    let first = check.chars().next().unwrap();
    if "*/%!<>=+-&|^?".contains(first) {
        return check.chars().all(|c| "*/%!<>=+-&|^?".contains(c));
    }

    // Identifier-style names: must start with letter or underscore
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }

    // Validate dotted segments
    let segments: Vec<&str> = check.split('.').collect();
    for (i, segment) in segments.iter().enumerate() {
        if segment.is_empty() {
            return false; // empty segment (consecutive dots or trailing dot)
        }
        let seg_first = segment.chars().next().unwrap();

        if i == 0 {
            // First segment: must be a plain identifier
            if !seg_first.is_ascii_alphabetic() && seg_first != '_' {
                return false;
            }
            if !segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return false;
            }
        } else {
            // Subsequent dotted segments
            if is_tilde && "*/%!<>=+-&|^".contains(seg_first) {
                // Tilde identifiers allow operator segments (e.g., mc.+~, mc.*~)
                if !segment.chars().all(|c| {
                    c.is_ascii_alphanumeric() || c == '_' || c == '-' || "*/%!<>=+-&|^".contains(c)
                }) {
                    return false;
                }
            } else if seg_first.is_ascii_alphanumeric() || seg_first == '_' {
                // Regular identifier or digit-starting segment (e.g., jit.3m, jit.gl, gbr.wind=)
                if !segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '=')
                {
                    return false;
                }
            } else {
                return false;
            }
        }
    }

    true
}

/// Sanitize a literal argument from Max object text for use in flutmax.
///
/// Text args from Max objects are ALWAYS literal values (symbol names, numbers,
/// type designators), never wire references. A Max object's text args come from
/// the box's text field and set initial/default values.
///
/// - Numbers pass through unchanged.
/// - Already-quoted strings pass through.
/// - Everything else becomes a string literal (Max symbol values like "set", "bang",
///   "invert", type designators like "f", "i", "s").
///
/// Note: This no longer checks known wire/port names. Even if a literal arg
/// happens to match a wire name (e.g., `prepend invert` where `invert` is also
/// a pattr wire name), the text arg is always a literal value, not a reference.
fn sanitize_literal_arg(s: &str, _known_names: &HashSet<&str>) -> String {
    let normalized = normalize_number_literal(s);

    // Already a string literal
    if normalized.starts_with('"') && normalized.ends_with('"') {
        return normalized;
    }

    // Number (with optional leading negative, at most one dot)
    if is_valid_number_token(&normalized) {
        return normalized;
    }

    // Always wrap non-numeric values as string literals.
    // Max text args are symbol values, not wire references.
    format!(
        "\"{}\"",
        normalized.replace('\\', "\\\\").replace('"', "\\\"")
    )
}

/// Sanitize an argument token so it is valid flutmax syntax (context-free version).
///
/// Tokens that are valid identifiers or numbers pass through unchanged.
/// Anything else is wrapped in double quotes to become a string literal.
fn sanitize_arg(s: &str) -> String {
    let normalized = normalize_number_literal(s);
    if is_valid_arg_token(&normalized) {
        normalized
    } else {
        format!(
            "\"{}\"",
            normalized.replace('\\', "\\\\").replace('"', "\\\"")
        )
    }
}

/// Check if a token is a valid flutmax argument (number, identifier, or string).
fn is_valid_arg_token(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // Already a string literal
    if s.starts_with('"') && s.ends_with('"') {
        return true;
    }
    // Number (with optional leading negative, at most one dot)
    if is_valid_number_token(s) {
        return true;
    }
    // Identifier (plain or dotted or tilde)
    is_valid_object_name(s)
}

/// Check if a token is a valid numeric literal for flutmax.
///
/// Accepts integers (`42`), floats (`3.14`), negative numbers (`-1.5`),
/// and leading-dot floats (`0.5`).
/// Rejects multi-dot tokens like `4.0.0` (Max tempo notation) or `127.0.0.1` (IP addresses).
fn is_valid_number_token(s: &str) -> bool {
    let num = s.trim_start_matches('-');
    if num.is_empty() {
        return false;
    }
    let first = num.chars().next().unwrap();
    if !first.is_ascii_digit() && first != '.' {
        return false;
    }
    if !num.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return false;
    }
    // At most one dot
    num.chars().filter(|&c| c == '.').count() <= 1
}

/// Normalise a numeric literal for flutmax syntax compatibility.
///
/// - Trailing-dot floats: `127.` → `127.0`, `-50.` → `-50.0`
/// - Standalone negative sign before a number is kept as-is
/// - Non-numeric tokens are passed through unchanged.
fn normalize_number_literal(s: &str) -> String {
    let trimmed = s.trim_start_matches('-');
    let is_neg = s.starts_with('-') && !trimmed.is_empty();

    // Check if it looks like a number (possibly with trailing dot)
    if trimmed.is_empty() {
        return s.to_string();
    }
    let first_char = trimmed.chars().next().unwrap();
    if !first_char.is_ascii_digit() && first_char != '.' {
        return s.to_string();
    }

    // Trailing-dot float: "127." → "127.0", "-50." → "-50.0"
    if s.ends_with('.') && s.len() > 1 {
        let has_digits = trimmed
            .trim_end_matches('.')
            .chars()
            .all(|c| c.is_ascii_digit());
        if has_digits {
            return format!("{}0", s);
        }
    }

    // Leading-dot float: ".5" → "0.5", "-.5" → "-0.5"
    if let Some(rest) = trimmed.strip_prefix('.') {
        if rest.chars().all(|c| c.is_ascii_digit()) {
            if is_neg {
                return format!("-0.{}", rest);
            } else {
                return format!("0.{}", rest);
            }
        }
    }

    s.to_string()
}

/// Resolve the name for a source node (wire name or inlet name).
fn resolve_source_name(
    source_id: &str,
    wire_names: &HashMap<&str, String>,
    inlet_names: &HashMap<&str, String>,
) -> String {
    if let Some(name) = wire_names.get(source_id) {
        name.clone()
    } else if let Some(name) = inlet_names.get(source_id) {
        name.clone()
    } else {
        format!("unknown_{}", source_id)
    }
}

/// Extract a wire name from a MaxBox based on its object name.
///
/// Uses the first word of the box text (the object name), strips the `~` suffix
/// for cleaner names, and for dotted names (e.g., `jit.gl.render`) uses the last
/// segment. Falls back to the `maxclass` if text is absent or empty.
/// Infer a meaningful name for a message box from its text content.
///
/// Examples:
/// - `"0 100"` → `"msg_0_100"` (numeric content gets msg_ prefix)
/// - `"setdomain $1"` → `"setdomain"`
/// - `"setrange 0 $1"` → `"setrange"`
/// - `"bang"` → `"msg_bang"`
/// - `""` → `"msg"`
fn infer_msg_name(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "msg".to_string();
    }
    let first_word = trimmed.split_whitespace().next().unwrap_or("msg");
    // If first word is purely numeric, prefix with msg_
    if first_word
        .chars()
        .all(|c| c.is_ascii_digit() || c == '.' || c == '-')
    {
        // Use the full content but shortened: "0 100" → "msg_0_100"
        let short: String = trimmed
            .split_whitespace()
            .take(3)
            .collect::<Vec<_>>()
            .join("_");
        return format!("msg_{}", short);
    }
    // Use first word as name (setdomain, setrange, bang, etc.)
    first_word.to_string()
}

fn extract_object_name_for_wire(b: &MaxBox) -> String {
    let text = match &b.text {
        Some(t) => t.as_str(),
        None => return b.maxclass.clone(),
    };
    // Get first word (object name)
    let first = text.split_whitespace().next().unwrap_or(&b.maxclass);
    // Use reverse alias to convert operators to named forms:
    // *~ → mul~, +~ → add~, /~ → dvd~, etc.
    let aliased = alias::reverse_alias(first);
    // Strip ~ suffix for cleaner names
    let base = aliased.trim_end_matches('~');
    if base.is_empty() {
        return b.maxclass.clone();
    }
    // For dotted names like jit.gl.render, use last segment
    if let Some(last_dot) = base.rfind('.') {
        base[last_dot + 1..].to_string()
    } else {
        base.to_string()
    }
}

/// Replace known Unicode characters with ASCII equivalents for identifier safety.
///
/// Max objects and patch names may contain Unicode characters like `µ` (micro)
/// or `ƒ` (function) that are not valid in flutmax identifiers.
fn sanitize_unicode(name: &str) -> String {
    name.replace(['\u{00B5}', '\u{03BC}'], "micro") // μ (Greek small letter mu)
        .replace('\u{0192}', "f") // ƒ (Latin small f with hook)
        .replace('\u{00B0}', "deg") // ° (degree sign)
        .replace('\u{2126}', "ohm") // Ω (ohm sign)
        .replace('\u{00D7}', "x") // × (multiplication sign)
        .replace('\u{00F7}', "div") // ÷ (division sign)
        .replace('\u{2192}', "to") // → (rightwards arrow)
        .replace('\u{2190}', "from") // ← (leftwards arrow)
}

/// Check if a name is a reserved keyword in flutmax.
///
/// Wire names that collide with these keywords cause parse errors.
fn is_reserved_keyword(name: &str) -> bool {
    matches!(
        name,
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
    )
}

/// Sanitize a name for use as a flutmax identifier (case-preserving).
///
/// Replaces runs of non-alphanumeric characters with a single `_`,
/// strips leading/trailing underscores, and ensures the result is a valid
/// identifier. Preserves case for varname round-tripping.
///
/// - Prefixes with `_` if the name starts with a digit
/// - Falls back to `"obj"` if the name is empty after sanitization
/// - Prefixes with `w_` if the name collides with a reserved keyword
fn sanitize_name(name: &str) -> String {
    sanitize_name_impl(name, false)
}

/// Sanitize a name for use as a generated wire name (lowercased).
///
/// Same as `sanitize_name` but also converts to lowercase for cleaner
/// generated wire names (e.g., `cycle~` -> `cycle`, `MIDi key` -> `midi_key`).
fn sanitize_name_lower(name: &str) -> String {
    sanitize_name_impl(name, true)
}

fn sanitize_name_impl(name: &str, lowercase: bool) -> String {
    // First, replace known Unicode characters with ASCII equivalents
    let ascii_name = sanitize_unicode(name);

    let mut result = String::new();
    let mut prev_separator = false;

    for ch in ascii_name.chars() {
        if ch.is_ascii_alphanumeric() {
            if lowercase {
                result.push(ch.to_ascii_lowercase());
            } else {
                result.push(ch);
            }
            prev_separator = false;
        } else if ch == '_' {
            if lowercase {
                // In lowercase mode, collapse consecutive separators
                if !prev_separator && !result.is_empty() {
                    result.push('_');
                    prev_separator = true;
                }
            } else {
                // In case-preserving mode, keep underscores (including leading)
                result.push('_');
                prev_separator = true;
            }
        } else if ch == '-' {
            if lowercase {
                // In lowercase mode (generated names), treat hyphens as underscores
                if !prev_separator && !result.is_empty() {
                    result.push('_');
                    prev_separator = true;
                }
            } else {
                // In case-preserving mode (varnames), keep valid hyphens
                if !prev_separator && !result.is_empty() {
                    result.push('-');
                    prev_separator = true;
                }
            }
        } else if (ch == ' ' || ch == '#') && !prev_separator && !result.is_empty() {
            result.push('_');
            prev_separator = true;
        }
        // Other chars (., ~, etc.) are dropped
    }

    if lowercase {
        // Strip trailing separator in lowercase mode
        while result.ends_with('_') || result.ends_with('-') {
            result.pop();
        }
    } else {
        // In case-preserving mode, only strip trailing hyphens (underscores are valid)
        while result.ends_with('-') {
            result.pop();
        }
    }

    // Normalize hyphens for grammar compliance: the grammar requires
    // `-[a-zA-Z0-9_]+` pattern (no consecutive hyphens, no leading hyphen).
    while result.contains("--") {
        result = result.replace("--", "-");
    }
    while result.starts_with('-') {
        result = result[1..].to_string();
    }

    // If empty, use a fallback
    if result.is_empty() {
        result = "obj".to_string();
    }

    // Ensure starts with letter or underscore (not digit, not hyphen)
    if result
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_digit() || c == '-')
    {
        result = format!("_{}", result);
    }

    // Avoid reserved keyword collisions — check lowercase for case-insensitive matching
    let lower_result = result.to_ascii_lowercase();
    if is_reserved_keyword(&lower_result) {
        result = format!("w_{}", result);
    }

    result
}

/// Format a serde_json::Value into a string suitable for `.attr()` output.
///
/// - Numbers are printed as-is
/// - Strings are quoted with double quotes
/// - Booleans are mapped to 1/0 (Max convention)
/// - Arrays are space-separated (e.g., `[1, 2, 3]` → `1 2 3`)
fn format_attr_value(v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::Number(n) => n.to_string(),
        Value::String(s) => {
            let escaped = s
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
                .replace('\r', "\\r")
                .replace('\t', "\\t");
            format!("\"{}\"", escaped)
        }
        Value::Bool(b) => {
            if *b {
                "1".to_string()
            } else {
                "0".to_string()
            }
        }
        Value::Array(arr) => {
            // Quote arrays as strings to avoid parse errors
            let inner: Vec<String> = arr
                .iter()
                .map(|item| match item {
                    Value::Number(n) => n.to_string(),
                    Value::String(s) => s.clone(),
                    other => format!("{}", other),
                })
                .collect();
            format!("\"{}\"", inner.join(" "))
        }
        Value::Object(_) => {
            // Quote objects as JSON strings
            let json_str = serde_json::to_string(v).unwrap_or_default();
            let escaped = json_str.replace('\\', "\\\\").replace('"', "\\\"");
            format!("\"{}\"", escaped)
        }
        Value::Null => "0".to_string(),
    }
}

/// Extract `@key value` attribute pairs from a Max object's text field.
///
/// For `newobj` boxes, the text field may contain inline attributes like
/// `cycle~ 440 @phase 0.5 @frequency 220`. This function extracts those
/// as `(key, value)` string pairs.
fn extract_text_attrs(text: &str) -> Vec<(String, String)> {
    let parts: Vec<&str> = text.split_whitespace().collect();
    let mut attrs = Vec::new();

    // Find the first @-prefixed token
    let attr_start = parts.iter().position(|p| p.starts_with('@'));
    if let Some(start) = attr_start {
        let mut i = start;
        while i < parts.len() {
            if parts[i].starts_with('@') {
                let key = parts[i].trim_start_matches('@').to_string();
                // Collect all following tokens until the next @-prefixed token as the value
                let mut value_parts = Vec::new();
                i += 1;
                while i < parts.len() && !parts[i].starts_with('@') {
                    value_parts.push(parts[i]);
                    i += 1;
                }
                let value = if value_parts.is_empty() {
                    "1".to_string() // Boolean-style attribute with no value
                } else if value_parts.len() == 1 {
                    let v = value_parts[0];
                    if v.parse::<f64>().is_ok() {
                        v.to_string()
                    } else if v
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
                        && v.chars()
                            .next()
                            .is_some_and(|c| c.is_alphabetic() || c == '_')
                    {
                        // Simple identifier — safe without quoting
                        v.to_string()
                    } else {
                        format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
                    }
                } else {
                    // Multi-token value: quote as string
                    let joined = value_parts.join(" ");
                    format!("\"{}\"", joined.replace('\\', "\\\\").replace('"', "\\\""))
                };
                if !key.is_empty() {
                    attrs.push((key, value));
                }
            } else {
                i += 1;
            }
        }
    }

    attrs
}

/// Check if a MaxBox is a codebox with inline code.
fn is_codebox_with_code(node: &MaxBox) -> bool {
    matches!(node.maxclass.as_str(), "v8.codebox" | "codebox")
        && node.code.as_ref().is_some_and(|c| !c.is_empty())
}

/// Extract codebox code content to an external file reference.
///
/// When a node is a `v8.codebox` or `codebox` with inline `code`, this function:
/// - Generates an external filename (`{wire_name}.js` or `{wire_name}.genexpr`)
/// - Returns a modified expression referencing the file
/// - Returns the `(filename, code_content)` pair for the code file
///
/// If the node is not a codebox or has no inline code, returns the original expression unchanged.
fn extract_codebox_code(
    node: &MaxBox,
    wire_name: &str,
    original_expr: &str,
) -> (String, Option<(String, String)>) {
    let code = match &node.code {
        Some(c) if !c.is_empty() => c,
        _ => return (original_expr.to_string(), None),
    };

    match node.maxclass.as_str() {
        "v8.codebox" => {
            let filename = format!("{}.js", wire_name);
            let expr = format!("v8.codebox(\"{}\")", filename);
            (expr, Some((filename, code.clone())))
        }
        "codebox" => {
            let filename = format!("{}.genexpr", wire_name);
            let expr = format!("codebox(\"{}\")", filename);
            (expr, Some((filename, code.clone())))
        }
        _ => (original_expr.to_string(), None),
    }
}

/// Check if an attribute key is decorative (visual/layout only, not functional).
///
/// Decorative attributes belong in the `.uiflutmax` sidecar file, not in `.flutmax`.
/// Functional attributes (min, max, domain, mode, parameter_enable, etc.) stay in `.flutmax`.
pub fn is_decorative_attr(key: &str) -> bool {
    // Prefix-based: bgfillcolor_*, active*, inactive*
    if key.starts_with("bgfillcolor_")
        || key.starts_with("activebg")
        || key.starts_with("activeline")
        || key.starts_with("activeslider")
        || key.starts_with("activetri")
        || key.starts_with("inactivelcd")
    {
        return true;
    }

    matches!(
        key,
        // Background / color
        "background" | "bgcolor" | "bgcolor2" | "textcolor" | "textcolor2" | "color"
        | "bgoncolor" | "bgovercolor" | "textoncolor" | "textovercolor"
        | "blinkcolor" | "checkedcolor" | "knobcolor" | "outlinecolor"
        | "focusbordercolor" | "lcdbgcolor" | "lcdcolor" | "tricolor" | "tricolor2"
        // Font
        | "fontname" | "fontsize" | "fontface"
        // Border / line / point
        | "bordercolor" | "linecolor" | "pointcolor" | "sustaincolor"
        // Layout / display
        | "gradient" | "legacytextcolor" | "usebgoncolor"
        | "textjustification" | "align" | "legend"
        | "presentation" | "presentation_rect"
        // bpatcher / panel display
        | "angle" | "rounded" | "proportion" | "border"
        | "bgmode" | "clickthrough" | "lockeddragscroll" | "lockedsize"
        | "enablehscroll" | "enablevscroll" | "viewvisibility" | "offset"
        // Interaction style (not functional behavior)
        | "allowdrag" | "smooth"
    )
}

/// Build the combined attribute list for a box, separating functional from decorative.
///
/// Key-value attribute list.
type AttrList = Vec<(String, String)>;

/// Returns (functional_attrs, decorative_attrs).
/// Functional attrs go into `.flutmax` `.attr()` chains.
/// Decorative attrs go into `.uiflutmax` sidecar file.
fn build_box_attrs_split(b: &MaxBox) -> (AttrList, AttrList) {
    let all_attrs = build_box_attrs(b);
    let mut functional = Vec::new();
    let mut decorative = Vec::new();
    for (k, v) in all_attrs {
        if is_decorative_attr(&k) {
            decorative.push((k, v));
        } else {
            functional.push((k, v));
        }
    }
    (functional, decorative)
}

///
/// For non-newobj boxes (UI elements), only JSON extra_attrs are used.
/// For newobj boxes, text-based `@` attrs are used (they are the canonical source
/// since Max stores newobj attributes in the text field).
fn build_box_attrs(b: &MaxBox) -> Vec<(String, String)> {
    let mut attrs: Vec<(String, String)> = Vec::new();

    // For newobj, extract @attrs from text (canonical location)
    if b.maxclass == "newobj" {
        if let Some(ref text) = b.text {
            attrs.extend(extract_text_attrs(text));
        }
    }

    // For non-newobj boxes (UI elements), use JSON extra_attrs
    if b.maxclass != "newobj" {
        for (k, v) in &b.extra_attrs {
            attrs.push((k.clone(), format_attr_value(v)));
        }
    }

    // For newobj boxes that also have JSON extra_attrs beyond what's in text,
    // add those too (avoiding duplicates with text-based attrs)
    if b.maxclass == "newobj" {
        let text_keys: std::collections::HashSet<String> =
            attrs.iter().map(|(k, _)| k.clone()).collect();
        for (k, v) in &b.extra_attrs {
            if !text_keys.contains(k) {
                attrs.push((k.clone(), format_attr_value(v)));
            }
        }
    }

    // Filter out attrs that would cause parse errors in flutmax.
    // Key must be a valid plain identifier, value must be parseable.
    attrs.retain(|(k, v)| is_safe_attr_key(k) && is_safe_attr_value(v));

    attrs
}

/// Check if an attr key is a valid flutmax identifier (no slashes, dots at end, etc.).
fn is_safe_attr_key(k: &str) -> bool {
    if k.is_empty() {
        return false;
    }
    // Must start with letter or underscore
    let first = k.chars().next().unwrap();
    if !first.is_alphabetic() && first != '_' {
        return false;
    }
    // Must contain only alphanumeric, underscore, hyphen
    if !k
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return false;
    }
    true
}

/// Check if an attr value can be safely parsed by the flutmax grammar.
fn is_safe_attr_value(v: &str) -> bool {
    if v.is_empty() {
        return false;
    }
    // Quoted string — safe if content is simple
    if v.starts_with('"') && v.ends_with('"') {
        let inner = &v[1..v.len() - 1];
        // Reject strings with nested quotes, newlines, or JSON-like content
        if inner.contains('\n') || inner.contains('\r') {
            return false;
        }
        if inner.contains("\"") || inner.contains("\\\"") {
            return false;
        }
        // Reject JSON objects/arrays leaked into string
        if inner.contains("\":[") || inner.contains("\":{") || inner.contains("[\"") {
            return false;
        }
        return true;
    }
    // Number — safe
    if v.parse::<f64>().is_ok() {
        return true;
    }
    // Simple identifier — safe (no slashes, colons, or other special chars)
    if v.chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')
        && v.chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
        && !v.contains('/')
        && !v.ends_with('.')
    {
        return true;
    }
    false
}

/// Resolve a $N template argument to the corresponding inlet port name.
///
/// Max uses `$1`, `$2` etc. as abstraction argument placeholders.
/// Typed variants `$f1`, `$i1`, `$s1` are also resolved (the prefix is stripped).
/// Unresolved `$N` and `#0` are emitted as string literals.
///
/// Note: No longer used in production code (template args are now stripped).
/// Kept for tests that verify the resolution logic.
#[cfg(test)]
fn resolve_template_arg(arg: &str, in_port_names: &[String]) -> String {
    if arg.starts_with('$') {
        let num_part = arg[1..].trim_start_matches(|c: char| c.is_ascii_alphabetic());
        if let Ok(n) = num_part.parse::<usize>() {
            if n > 0 && n <= in_port_names.len() {
                return in_port_names[n - 1].clone();
            }
        }
        // Unresolvable $N / $fN — emit as string literal
        return format!("\"{}\"", arg);
    }
    // #0 and other # args are Max runtime features, keep as string literal
    if arg.starts_with('#') {
        return format!("\"{}\"", arg);
    }
    arg.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_maxpat;

    const L1_JSON: &str = include_str!("../../../tests/e2e/expected/L1_sine.maxpat");
    const L2_JSON: &str = include_str!("../../../tests/e2e/expected/L2_simple_synth.maxpat");
    const L3B_JSON: &str = include_str!("../../../tests/e2e/expected/L3b_control_fanout.maxpat");

    #[test]
    fn analyze_l1_sine() {
        let pat = parse_maxpat(L1_JSON).unwrap();
        let result = analyze(&pat, None).unwrap();

        // L1: cycle~ 440 → outlet
        // No inlets
        assert_eq!(result.in_decls.len(), 0);
        // 1 outlet
        assert_eq!(result.out_decls.len(), 1);
        // 1 wire: cycle~(440)
        assert_eq!(result.wires.len(), 1);
        assert_eq!(result.wires[0].expr, "cycle~(440)");
        // 1 out assignment
        assert_eq!(result.out_assignments.len(), 1);
        assert_eq!(result.out_assignments[0].index, 0);
    }

    #[test]
    fn analyze_l2_simple_synth() {
        let pat = parse_maxpat(L2_JSON).unwrap();
        let result = analyze(&pat, None).unwrap();

        // 1 inlet (freq), 1 outlet
        assert_eq!(result.in_decls.len(), 1);
        assert_eq!(result.out_decls.len(), 1);

        // 2 wires: cycle~(port_0) and mul~(w_1, 0.5)
        assert_eq!(result.wires.len(), 2);

        // First wire should be cycle~
        assert!(result.wires[0].expr.contains("cycle~"));
        // Second wire should be mul~ with literal 0.5
        assert!(result.wires[1].expr.contains("mul~"));
        assert!(result.wires[1].expr.contains("0.5"));

        // 1 out assignment
        assert_eq!(result.out_assignments.len(), 1);
    }

    #[test]
    fn analyze_l3b_trigger_removal() {
        let pat = parse_maxpat(L3B_JSON).unwrap();
        let result = analyze(&pat, None).unwrap();

        // 1 inlet, 2 outlets
        assert_eq!(result.in_decls.len(), 1);
        assert_eq!(result.out_decls.len(), 2);

        // trigger node should be removed
        // Remaining wire candidates: obj-2 (* 2), obj-3 (+ 100), obj-4 (- 100) = 3 wires
        assert_eq!(result.wires.len(), 3);

        // No wire expression should mention "trigger"
        for wire in &result.wires {
            assert!(
                !wire.expr.contains("trigger"),
                "trigger should be removed: {}",
                wire.expr
            );
        }

        // 2 out assignments
        assert_eq!(result.out_assignments.len(), 2);
    }

    #[test]
    fn test_extract_object_parts() {
        let b = MaxBox {
            id: "obj-1".into(),
            maxclass: "newobj".into(),
            text: Some("cycle~ 440".into()),
            numinlets: 2,
            numoutlets: 1,
            outlettype: vec!["signal".into()],
            comment: None,
            varname: None,
            patching_rect: [0.0; 4],
            embedded_patcher: None,
            extra_attrs: vec![],
            code: None,
        };
        let (name, args) = extract_object_parts(&b);
        assert_eq!(name, "cycle~");
        assert_eq!(args, vec!["440"]);
    }

    #[test]
    fn test_extract_object_parts_no_args() {
        let b = MaxBox {
            id: "obj-1".into(),
            maxclass: "newobj".into(),
            text: Some("cycle~".into()),
            numinlets: 2,
            numoutlets: 1,
            outlettype: vec!["signal".into()],
            comment: None,
            varname: None,
            patching_rect: [0.0; 4],
            embedded_patcher: None,
            extra_attrs: vec![],
            code: None,
        };
        let (name, args) = extract_object_parts(&b);
        assert_eq!(name, "cycle~");
        assert!(args.is_empty());
    }

    #[test]
    fn test_is_trigger_node() {
        let make_box = |text: &str| MaxBox {
            id: "t".into(),
            maxclass: "newobj".into(),
            text: Some(text.into()),
            numinlets: 1,
            numoutlets: 2,
            outlettype: vec![],
            comment: None,
            varname: None,
            patching_rect: [0.0; 4],
            embedded_patcher: None,
            extra_attrs: vec![],
            code: None,
        };

        assert!(is_trigger_node(&make_box("trigger b b")));
        assert!(is_trigger_node(&make_box("trigger")));
        assert!(is_trigger_node(&make_box("t b b")));
        assert!(is_trigger_node(&make_box("t")));
        assert!(!is_trigger_node(&make_box("toggle")));
        assert!(!is_trigger_node(&make_box("timer")));
    }

    #[test]
    fn test_sanitize_name() {
        // Case-preserving sanitizer (for varnames)
        assert_eq!(sanitize_name("hello world"), "hello_world");
        assert_eq!(sanitize_name("freq-hz"), "freq-hz");
        assert_eq!(sanitize_name("my_var"), "my_var");
        assert_eq!(
            sanitize_name("MIDi key # and velocity"),
            "MIDi_key_and_velocity"
        );
        assert_eq!(sanitize_name("myMessage"), "myMessage");
        assert_eq!(sanitize_name("cycle~"), "cycle");
        // Hyphens preserved for varname round-tripping
        assert_eq!(sanitize_name("module-onoff"), "module-onoff");
        assert_eq!(sanitize_name("zoom-valH"), "zoom-valH");
    }

    #[test]
    fn test_sanitize_name_lower() {
        // Lowercasing sanitizer (for generated wire names)
        assert_eq!(
            sanitize_name_lower("MIDi key # and velocity"),
            "midi_key_and_velocity"
        );
        assert_eq!(sanitize_name_lower("myMessage"), "mymessage");
        assert_eq!(sanitize_name_lower("cycle~"), "cycle");
        assert_eq!(sanitize_name_lower("Hello World"), "hello_world");
    }

    #[test]
    fn test_sanitize_name_digit_start() {
        // Names starting with digits get _ prefix
        assert_eq!(sanitize_name("1abc"), "_1abc");
        assert_eq!(sanitize_name("42"), "_42");
        assert_eq!(sanitize_name("0x_val"), "_0x_val");
    }

    #[test]
    fn test_sanitize_name_reserved_keywords() {
        // Reserved keywords get w_ prefix
        assert_eq!(sanitize_name("wire"), "w_wire");
        assert_eq!(sanitize_name("in"), "w_in");
        assert_eq!(sanitize_name("out"), "w_out");
        assert_eq!(sanitize_name("feedback"), "w_feedback");
        assert_eq!(sanitize_name("state"), "w_state");
        assert_eq!(sanitize_name("msg"), "w_msg");
        assert_eq!(sanitize_name("signal"), "w_signal");
        assert_eq!(sanitize_name("float"), "w_float");
        assert_eq!(sanitize_name("int"), "w_int");
        assert_eq!(sanitize_name("bang"), "w_bang");
        assert_eq!(sanitize_name("list"), "w_list");
        assert_eq!(sanitize_name("symbol"), "w_symbol");
    }

    #[test]
    fn test_sanitize_name_hyphen_start() {
        // Names starting with hyphen: leading hyphen is stripped
        assert_eq!(sanitize_name("-val"), "val");
        // Multiple leading hyphens: all leading hyphens stripped
        assert_eq!(sanitize_name("---val"), "val");
        // All hyphens: empty result, falls back to "obj"
        assert_eq!(sanitize_name("---"), "obj");
    }

    #[test]
    fn test_sanitize_name_non_reserved_passthrough() {
        // Non-reserved identifiers pass through unchanged
        assert_eq!(sanitize_name("my_wire"), "my_wire");
        assert_eq!(sanitize_name("output"), "output");
        assert_eq!(sanitize_name("input"), "input");
    }

    #[test]
    fn test_reverse_alias_in_expr() {
        let pat = parse_maxpat(L2_JSON).unwrap();
        let result = analyze(&pat, None).unwrap();

        // *~ 0.5 should become mul~(...)
        let mul_wire = result.wires.iter().find(|w| w.expr.contains("mul~"));
        assert!(mul_wire.is_some(), "Expected mul~ alias for *~");
    }

    #[test]
    fn test_l3b_alias_for_operators() {
        let pat = parse_maxpat(L3B_JSON).unwrap();
        let result = analyze(&pat, None).unwrap();

        // * 2 → mul(...), + 100 → add(...), - 100 → sub(...)
        let names: Vec<&str> = result.wires.iter().map(|w| w.expr.as_str()).collect();
        assert!(
            names.iter().any(|e| e.contains("mul(")),
            "Expected mul alias for *: {:?}",
            names
        );
        assert!(
            names.iter().any(|e| e.contains("add(")),
            "Expected add alias for +: {:?}",
            names
        );
        assert!(
            names.iter().any(|e| e.contains("sub(")),
            "Expected sub alias for -: {:?}",
            names
        );
    }

    #[test]
    fn test_comment_box_classification() {
        // Patch with a comment box — it should not appear as a wire
        let json = r#"{
            "patcher": {
                "boxes": [
                    { "box": { "id": "obj-1", "maxclass": "newobj", "text": "cycle~ 440", "numinlets": 2, "numoutlets": 1, "outlettype": ["signal"] } },
                    { "box": { "id": "obj-2", "maxclass": "comment", "text": "This is a description", "numinlets": 1, "numoutlets": 0, "outlettype": [] } },
                    { "box": { "id": "obj-3", "maxclass": "outlet", "numinlets": 1, "numoutlets": 0, "outlettype": [] } }
                ],
                "lines": [
                    { "patchline": { "source": ["obj-1", 0], "destination": ["obj-3", 0] } }
                ]
            }
        }"#;
        let pat = parse_maxpat(json).unwrap();
        let result = analyze(&pat, None).unwrap();

        // Comment should not appear as a wire
        assert_eq!(result.wires.len(), 1);
        assert_eq!(result.wires[0].expr, "cycle~(440)");

        // Comment should be captured
        assert_eq!(result.comments.len(), 1);
        assert_eq!(result.comments[0].text, "This is a description");
    }

    #[test]
    fn test_message_box_classification() {
        // Patch with a message box connected to a print object
        let json = r#"{
            "patcher": {
                "boxes": [
                    { "box": { "id": "obj-1", "maxclass": "message", "text": "bang", "numinlets": 2, "numoutlets": 1, "outlettype": [""] } },
                    { "box": { "id": "obj-2", "maxclass": "newobj", "text": "print", "numinlets": 1, "numoutlets": 0, "outlettype": [] } }
                ],
                "lines": [
                    { "patchline": { "source": ["obj-1", 0], "destination": ["obj-2", 0] } }
                ]
            }
        }"#;
        let pat = parse_maxpat(json).unwrap();
        let result = analyze(&pat, None).unwrap();

        // Message should not be a wire
        assert_eq!(result.wires.len(), 1); // print is the only wire
        assert!(result.wires[0].expr.contains("print"));

        // Message should be captured as a MsgInfo
        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].content, "bang");

        // The wire for print should reference the message box name (inferred from content "bang")
        assert!(
            result.wires[0].expr.contains("bang"),
            "print should reference msg name inferred from 'bang': {}",
            result.wires[0].expr
        );
    }

    #[test]
    fn test_inlet_tilde_classification() {
        // Patch with inlet~ (signal inlet) — should be classified as inlet
        let json = r#"{
            "patcher": {
                "boxes": [
                    { "box": { "id": "obj-1", "maxclass": "inlet~", "numinlets": 0, "numoutlets": 1, "outlettype": ["signal"], "comment": "audio_in" } },
                    { "box": { "id": "obj-2", "maxclass": "outlet~", "numinlets": 1, "numoutlets": 0, "outlettype": [] } }
                ],
                "lines": [
                    { "patchline": { "source": ["obj-1", 0], "destination": ["obj-2", 0] } }
                ]
            }
        }"#;
        let pat = parse_maxpat(json).unwrap();
        let result = analyze(&pat, None).unwrap();

        // Should have 1 signal inlet and 1 outlet
        assert_eq!(result.in_decls.len(), 1);
        assert_eq!(result.in_decls[0].port_type, "signal");
        assert_eq!(result.in_decls[0].name, "audio_in");
        assert_eq!(result.out_decls.len(), 1);

        // No wire candidates (only inlet~ and outlet~)
        assert_eq!(result.wires.len(), 0);
    }

    #[test]
    fn test_template_arg_resolution() {
        assert_eq!(
            resolve_template_arg("$1", &["freq".into(), "amp".into()]),
            "freq"
        );
        assert_eq!(
            resolve_template_arg("$2", &["freq".into(), "amp".into()]),
            "amp"
        );
        // Out of range — emit as string literal
        assert_eq!(
            resolve_template_arg("$3", &["freq".into(), "amp".into()]),
            "\"$3\""
        );
        // $0 is not a valid port index (1-based) — emit as string literal
        assert_eq!(resolve_template_arg("$0", &["freq".into()]), "\"$0\"");
        // Non-template args pass through
        assert_eq!(resolve_template_arg("440", &["freq".into()]), "440");
        // #0 becomes a string literal
        assert_eq!(resolve_template_arg("#0", &["freq".into()]), "\"#0\"");
    }

    #[test]
    fn test_template_arg_in_wire_expr() {
        // Patch: inlet → *~ $1 → outlet
        // $1 is a Max abstraction placeholder and should be stripped
        // (not resolved to the inlet port name) because the decompiler
        // produces standalone .flutmax files without abstraction context.
        let json = r#"{
            "patcher": {
                "boxes": [
                    { "box": { "id": "obj-1", "maxclass": "inlet", "numinlets": 0, "numoutlets": 1, "outlettype": [""], "comment": "gain" } },
                    { "box": { "id": "obj-2", "maxclass": "newobj", "text": "*~ $1", "numinlets": 2, "numoutlets": 1, "outlettype": ["signal"] } },
                    { "box": { "id": "obj-3", "maxclass": "outlet", "numinlets": 1, "numoutlets": 0, "outlettype": [] } }
                ],
                "lines": [
                    { "patchline": { "source": ["obj-2", 0], "destination": ["obj-3", 0] } }
                ]
            }
        }"#;
        let pat = parse_maxpat(json).unwrap();
        let result = analyze(&pat, None).unwrap();

        // The wire for *~ should have $1 stripped (template args are not resolved)
        assert_eq!(result.wires.len(), 1);
        assert!(
            result.wires[0].expr.contains("mul~("),
            "Expected *~ with stripped $1: {}",
            result.wires[0].expr
        );
        // Ensure no "gain" (template args should not be resolved to port names)
        assert!(
            !result.wires[0].expr.contains("gain"),
            "Template arg $1 should be stripped, not resolved: {}",
            result.wires[0].expr
        );
    }

    #[test]
    fn test_empty_comment_box_skipped() {
        // Comment with empty text should not appear in comments
        let json = r#"{
            "patcher": {
                "boxes": [
                    { "box": { "id": "obj-1", "maxclass": "comment", "text": "", "numinlets": 1, "numoutlets": 0, "outlettype": [] } },
                    { "box": { "id": "obj-2", "maxclass": "comment", "numinlets": 1, "numoutlets": 0, "outlettype": [] } }
                ],
                "lines": []
            }
        }"#;
        let pat = parse_maxpat(json).unwrap();
        let result = analyze(&pat, None).unwrap();

        // Empty comment text and missing text should not produce comments
        assert_eq!(result.comments.len(), 0);
    }

    #[test]
    fn test_message_box_with_varname() {
        let json = r#"{
            "patcher": {
                "boxes": [
                    { "box": { "id": "obj-1", "maxclass": "message", "text": "set 42", "numinlets": 2, "numoutlets": 1, "outlettype": [""], "varname": "myMessage" } }
                ],
                "lines": []
            }
        }"#;
        let pat = parse_maxpat(json).unwrap();
        let result = analyze(&pat, None).unwrap();

        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].name, "myMessage");
        assert_eq!(result.messages[0].content, "set 42");
    }

    // -----------------------------------------------------------------------
    // Subpatcher tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_extract_subpatcher_name_p() {
        use std::sync::atomic::AtomicU32;
        let counter = AtomicU32::new(1);

        let b = MaxBox {
            id: "obj-1".into(),
            maxclass: "newobj".into(),
            text: Some("p myfilter".into()),
            numinlets: 2,
            numoutlets: 1,
            outlettype: vec![],
            comment: None,
            varname: None,
            patching_rect: [0.0; 4],
            embedded_patcher: None,
            extra_attrs: vec![],
            code: None,
        };
        assert_eq!(
            extract_subpatcher_name(&b, "main", &counter),
            "main_myfilter"
        );
    }

    #[test]
    fn test_extract_subpatcher_name_poly() {
        use std::sync::atomic::AtomicU32;
        let counter = AtomicU32::new(1);

        let b = MaxBox {
            id: "obj-1".into(),
            maxclass: "newobj".into(),
            text: Some("poly~ myvoice 4".into()),
            numinlets: 2,
            numoutlets: 1,
            outlettype: vec![],
            comment: None,
            varname: None,
            patching_rect: [0.0; 4],
            embedded_patcher: None,
            extra_attrs: vec![],
            code: None,
        };
        assert_eq!(
            extract_subpatcher_name(&b, "synth", &counter),
            "synth_myvoice"
        );
    }

    #[test]
    fn test_extract_subpatcher_name_pfft() {
        use std::sync::atomic::AtomicU32;
        let counter = AtomicU32::new(1);

        let b = MaxBox {
            id: "obj-1".into(),
            maxclass: "newobj".into(),
            text: Some("pfft~ myfft 1024".into()),
            numinlets: 1,
            numoutlets: 1,
            outlettype: vec![],
            comment: None,
            varname: None,
            patching_rect: [0.0; 4],
            embedded_patcher: None,
            extra_attrs: vec![],
            code: None,
        };
        assert_eq!(extract_subpatcher_name(&b, "fx", &counter), "fx_myfft");
    }

    #[test]
    fn test_extract_subpatcher_name_varname_fallback() {
        use std::sync::atomic::AtomicU32;
        let counter = AtomicU32::new(1);

        let b = MaxBox {
            id: "obj-1".into(),
            maxclass: "newobj".into(),
            text: Some("p".into()), // unnamed "p" (no second word)
            numinlets: 1,
            numoutlets: 1,
            outlettype: vec![],
            comment: None,
            varname: Some("myVar".into()),
            patching_rect: [0.0; 4],
            embedded_patcher: None,
            extra_attrs: vec![],
            code: None,
        };
        assert_eq!(extract_subpatcher_name(&b, "main", &counter), "main_myVar");
    }

    #[test]
    fn test_extract_subpatcher_name_counter_fallback() {
        use std::sync::atomic::AtomicU32;
        let counter = AtomicU32::new(1);

        let b = MaxBox {
            id: "obj-1".into(),
            maxclass: "newobj".into(),
            text: None,
            numinlets: 1,
            numoutlets: 1,
            outlettype: vec![],
            comment: None,
            varname: None,
            patching_rect: [0.0; 4],
            embedded_patcher: None,
            extra_attrs: vec![],
            code: None,
        };
        let name1 = extract_subpatcher_name(&b, "main", &counter);
        assert!(
            name1.starts_with("main_sub_"),
            "Expected main_sub_N, got: {}",
            name1
        );

        let name2 = extract_subpatcher_name(&b, "main", &counter);
        assert_ne!(name1, name2, "Counter should increment");
    }

    #[test]
    fn test_analyze_recursive_one_subpatcher() {
        // Parent patch with [p myfilter] containing inlet~ → biquad~ → outlet~
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

        let pat = parse_maxpat(json).unwrap();
        let (main_patch, subpatchers) = analyze_recursive(&pat, "test", None).unwrap();

        // Should have 1 subpatcher
        assert_eq!(
            subpatchers.len(),
            1,
            "Expected 1 subpatcher, got {}",
            subpatchers.len()
        );
        assert!(
            subpatchers[0].0.contains("myfilter"),
            "Subpatcher name should contain 'myfilter': {}",
            subpatchers[0].0
        );

        // Main patch should have the subpatcher as a wire candidate
        assert_eq!(
            main_patch.wires.len(),
            2,
            "Expected 2 wires (cycle~ + myfilter)"
        );
        assert!(
            main_patch.wires[0].expr.contains("cycle~"),
            "First wire should be cycle~: {}",
            main_patch.wires[0].expr
        );
        assert!(
            main_patch.wires[1].expr.contains("myfilter"),
            "Second wire should reference myfilter: {}",
            main_patch.wires[1].expr
        );

        // Subpatcher should have its own structure
        let sub_patch = &subpatchers[0].1;
        assert_eq!(
            sub_patch.in_decls.len(),
            1,
            "Subpatcher should have 1 inlet"
        );
        assert_eq!(sub_patch.in_decls[0].port_type, "signal");
        assert_eq!(
            sub_patch.out_decls.len(),
            1,
            "Subpatcher should have 1 outlet"
        );
        assert_eq!(
            sub_patch.wires.len(),
            1,
            "Subpatcher should have 1 wire (biquad~)"
        );
        assert!(sub_patch.wires[0].expr.contains("biquad~"));
    }

    #[test]
    fn test_analyze_recursive_nested_subpatchers() {
        // Parent → [p outer] → [p inner] (3 levels)
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
                                                    { "box": { "id": "deep-2", "maxclass": "newobj", "text": "print hello", "numinlets": 1, "numoutlets": 0, "outlettype": [] } },
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

        let pat = parse_maxpat(json).unwrap();
        let (main_patch, subpatchers) = analyze_recursive(&pat, "root", None).unwrap();

        // Should have 2 subpatchers: outer and inner
        assert_eq!(
            subpatchers.len(),
            2,
            "Expected 2 subpatchers (outer + inner), got {}",
            subpatchers.len()
        );

        // Verify names contain the hierarchy
        let names: Vec<&str> = subpatchers.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            names.iter().any(|n| n.contains("outer")),
            "Should have 'outer' subpatcher: {:?}",
            names
        );
        assert!(
            names.iter().any(|n| n.contains("inner")),
            "Should have 'inner' subpatcher: {:?}",
            names
        );

        // Main patch should reference outer
        assert_eq!(main_patch.wires.len(), 1);
        assert!(
            main_patch.wires[0].expr.contains("outer"),
            "Main wire should reference outer: {}",
            main_patch.wires[0].expr
        );

        // Inner subpatcher should have a print wire
        let inner = subpatchers
            .iter()
            .find(|(n, _)| n.contains("inner"))
            .unwrap();
        assert!(
            inner.1.wires.iter().any(|w| w.expr.contains("print")),
            "Inner patch should contain print: {:?}",
            inner.1.wires
        );
    }

    #[test]
    fn test_analyze_recursive_bpatcher_inline() {
        // bpatcher with an inline embedded patcher
        let json = r#"{
            "patcher": {
                "boxes": [
                    {
                        "box": {
                            "id": "obj-1",
                            "maxclass": "bpatcher",
                            "numinlets": 1,
                            "numoutlets": 1,
                            "outlettype": [""],
                            "varname": "myBpatch",
                            "patcher": {
                                "boxes": [
                                    { "box": { "id": "sub-1", "maxclass": "inlet", "numinlets": 0, "numoutlets": 1, "outlettype": [""] } },
                                    { "box": { "id": "sub-2", "maxclass": "newobj", "text": "prepend set", "numinlets": 1, "numoutlets": 1, "outlettype": [""] } },
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

        let pat = parse_maxpat(json).unwrap();
        let (main_patch, subpatchers) = analyze_recursive(&pat, "test", None).unwrap();

        assert_eq!(subpatchers.len(), 1);
        assert!(
            subpatchers[0].0.contains("myBpatch"),
            "bpatcher should use varname: {}",
            subpatchers[0].0
        );

        // Main patch should have the bpatcher as a wire
        assert_eq!(main_patch.wires.len(), 1);
    }

    #[test]
    fn test_analyze_backward_compat_flat_patch() {
        // Regular analyze() should still work for flat patches
        let pat = parse_maxpat(L1_JSON).unwrap();
        let result = analyze(&pat, None).unwrap();
        assert_eq!(result.wires.len(), 1);
        assert_eq!(result.wires[0].expr, "cycle~(440)");
    }

    #[test]
    fn test_analyze_recursive_flat_patch() {
        // analyze_recursive on a flat patch should return empty subpatchers
        let pat = parse_maxpat(L1_JSON).unwrap();
        let (main_patch, subpatchers) = analyze_recursive(&pat, "sine", None).unwrap();

        assert!(
            subpatchers.is_empty(),
            "Flat patch should have no subpatchers"
        );
        assert_eq!(main_patch.wires.len(), 1);
        assert_eq!(main_patch.wires[0].expr, "cycle~(440)");
    }

    // -----------------------------------------------------------------------
    // .attr() extraction tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_format_attr_value_number() {
        use serde_json::json;
        assert_eq!(format_attr_value(&json!(42)), "42");
        assert_eq!(format_attr_value(&json!(3.14)), "3.14");
        assert_eq!(format_attr_value(&json!(0.0)), "0.0");
    }

    #[test]
    fn test_format_attr_value_string() {
        use serde_json::json;
        assert_eq!(format_attr_value(&json!("hello")), "\"hello\"");
    }

    #[test]
    fn test_format_attr_value_bool() {
        use serde_json::json;
        assert_eq!(format_attr_value(&json!(true)), "1");
        assert_eq!(format_attr_value(&json!(false)), "0");
    }

    #[test]
    fn test_format_attr_value_array() {
        use serde_json::json;
        assert_eq!(format_attr_value(&json!([1, 2, 3])), "\"1 2 3\"");
        assert_eq!(format_attr_value(&json!([0.5, 0.6])), "\"0.5 0.6\"");
    }

    #[test]
    fn test_extract_text_attrs_basic() {
        let attrs = extract_text_attrs("cycle~ 440 @phase 0.5");
        assert_eq!(attrs.len(), 1);
        assert_eq!(attrs[0], ("phase".into(), "0.5".into()));
    }

    #[test]
    fn test_extract_text_attrs_multiple() {
        let attrs = extract_text_attrs("flonum @minimum 0. @maximum 100. @value 50.");
        assert_eq!(attrs.len(), 3);
        assert_eq!(attrs[0], ("minimum".into(), "0.".into()));
        assert_eq!(attrs[1], ("maximum".into(), "100.".into()));
        assert_eq!(attrs[2], ("value".into(), "50.".into()));
    }

    #[test]
    fn test_extract_text_attrs_multi_word_value() {
        let attrs = extract_text_attrs("live.dial @parameter_longname My Cutoff @minimum 20.");
        assert_eq!(attrs.len(), 2);
        assert_eq!(
            attrs[0],
            ("parameter_longname".into(), "\"My Cutoff\"".into())
        );
        assert_eq!(attrs[1], ("minimum".into(), "20.".to_string()));
    }

    #[test]
    fn test_extract_text_attrs_none() {
        let attrs = extract_text_attrs("cycle~ 440");
        assert!(attrs.is_empty());
    }

    #[test]
    fn test_build_box_attrs_newobj_text() {
        let b = MaxBox {
            id: "obj-1".into(),
            maxclass: "newobj".into(),
            text: Some("cycle~ @phase 0.5".into()),
            numinlets: 2,
            numoutlets: 1,
            outlettype: vec!["signal".into()],
            comment: None,
            varname: None,
            patching_rect: [0.0; 4],
            embedded_patcher: None,
            extra_attrs: vec![],
            code: None,
        };
        let attrs = build_box_attrs(&b);
        assert_eq!(attrs.len(), 1);
        assert_eq!(attrs[0], ("phase".into(), "0.5".into()));
    }

    #[test]
    fn test_build_box_attrs_ui_element() {
        use serde_json::json;
        let b = MaxBox {
            id: "obj-1".into(),
            maxclass: "flonum".into(),
            text: None,
            numinlets: 1,
            numoutlets: 2,
            outlettype: vec!["".into(), "bang".into()],
            comment: None,
            varname: None,
            patching_rect: [0.0; 4],
            embedded_patcher: None,
            extra_attrs: vec![
                ("minimum".into(), json!(0.0)),
                ("maximum".into(), json!(100.0)),
            ],
            code: None,
        };
        let attrs = build_box_attrs(&b);
        assert_eq!(attrs.len(), 2);
        assert_eq!(attrs[0].0, "minimum");
        assert_eq!(attrs[1].0, "maximum");
    }

    #[test]
    fn test_build_box_attrs_no_attrs() {
        let b = MaxBox {
            id: "obj-1".into(),
            maxclass: "newobj".into(),
            text: Some("cycle~ 440".into()),
            numinlets: 2,
            numoutlets: 1,
            outlettype: vec!["signal".into()],
            comment: None,
            varname: None,
            patching_rect: [0.0; 4],
            embedded_patcher: None,
            extra_attrs: vec![],
            code: None,
        };
        let attrs = build_box_attrs(&b);
        assert!(attrs.is_empty());
    }

    #[test]
    fn test_analyze_flonum_with_attrs() {
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

        let pat = parse_maxpat(json).unwrap();
        let result = analyze(&pat, None).unwrap();
        assert_eq!(result.wires.len(), 1);
        assert!(
            !result.wires[0].attrs.is_empty(),
            "flonum wire should have attrs"
        );

        let attr_keys: Vec<&str> = result.wires[0]
            .attrs
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert!(attr_keys.contains(&"minimum"), "Should have minimum attr");
        assert!(attr_keys.contains(&"maximum"), "Should have maximum attr");
    }

    #[test]
    fn test_analyze_newobj_text_attrs() {
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

        let pat = parse_maxpat(json).unwrap();
        let result = analyze(&pat, None).unwrap();
        assert_eq!(result.wires.len(), 1);
        assert!(
            !result.wires[0].attrs.is_empty(),
            "cycle~ wire should have attrs from text"
        );

        let phase_attr = result.wires[0].attrs.iter().find(|(k, _)| k == "phase");
        assert!(phase_attr.is_some(), "Should have phase attr");
        assert_eq!(phase_attr.unwrap().1, "0.5");
    }

    #[test]
    fn test_analyze_no_attrs_for_plain_objects() {
        // L1 sine has just cycle~ 440 without any attrs
        let pat = parse_maxpat(L1_JSON).unwrap();
        let result = analyze(&pat, None).unwrap();
        assert_eq!(result.wires.len(), 1);
        assert!(
            result.wires[0].attrs.is_empty(),
            "Plain cycle~(440) should have no attrs, got: {:?}",
            result.wires[0].attrs
        );
    }

    /// Test fan-out with same X coordinate: Y descending (bottom first) tiebreaker.
    ///
    /// Max executes fan-outs bottom-to-top when X coordinates are equal.
    /// Destinations: print_c (Y=300) -> print_b (Y=200) -> print_a (Y=100)
    /// Expected flutmax order: c first (bottom), then b, then a (top).
    #[test]
    fn test_fanout_bottom_to_top_tiebreaker() {
        let json = r#"{
            "patcher": {
                "boxes": [
                    {
                        "box": {
                            "id": "src",
                            "maxclass": "newobj",
                            "text": "button",
                            "numinlets": 1,
                            "numoutlets": 1,
                            "outlettype": ["bang"],
                            "patching_rect": [100.0, 50.0, 50.0, 22.0]
                        }
                    },
                    {
                        "box": {
                            "id": "a",
                            "maxclass": "newobj",
                            "text": "print a",
                            "numinlets": 1,
                            "numoutlets": 0,
                            "outlettype": [],
                            "patching_rect": [100.0, 100.0, 50.0, 22.0]
                        }
                    },
                    {
                        "box": {
                            "id": "b",
                            "maxclass": "newobj",
                            "text": "print b",
                            "numinlets": 1,
                            "numoutlets": 0,
                            "outlettype": [],
                            "patching_rect": [100.0, 200.0, 50.0, 22.0]
                        }
                    },
                    {
                        "box": {
                            "id": "c",
                            "maxclass": "newobj",
                            "text": "print c",
                            "numinlets": 1,
                            "numoutlets": 0,
                            "outlettype": [],
                            "patching_rect": [100.0, 300.0, 50.0, 22.0]
                        }
                    }
                ],
                "lines": [
                    { "patchline": { "source": ["src", 0], "destination": ["a", 0] } },
                    { "patchline": { "source": ["src", 0], "destination": ["b", 0] } },
                    { "patchline": { "source": ["src", 0], "destination": ["c", 0] } }
                ]
            }
        }"#;

        let pat = parse_maxpat(json).unwrap();
        let result = analyze(&pat, None).unwrap();

        // 4 wires: button, print c, print b, print a
        assert_eq!(
            result.wires.len(),
            4,
            "Expected 4 wires, got: {:?}",
            result.wires.iter().map(|w| &w.name).collect::<Vec<_>>()
        );

        // The button (source) should be first
        assert!(
            result.wires[0].expr.contains("button"),
            "First wire should be button, got: {}",
            result.wires[0].expr
        );

        // Fan-out destinations: bottom first (Y descending)
        // c (Y=300) -> b (Y=200) -> a (Y=100)
        assert!(
            result.wires[1].expr.contains("print") && result.wires[1].expr.contains("c"),
            "Second wire should be print(c) [Y=300, bottom], got: {}",
            result.wires[1].expr
        );
        assert!(
            result.wires[2].expr.contains("print") && result.wires[2].expr.contains("b"),
            "Third wire should be print(b) [Y=200, middle], got: {}",
            result.wires[2].expr
        );
        assert!(
            result.wires[3].expr.contains("print") && result.wires[3].expr.contains("a"),
            "Fourth wire should be print(a) [Y=100, top], got: {}",
            result.wires[3].expr
        );
    }

    /// Test fan-out with different X coordinates: X descending (right first).
    #[test]
    fn test_fanout_right_to_left() {
        let json = r#"{
            "patcher": {
                "boxes": [
                    {
                        "box": {
                            "id": "src",
                            "maxclass": "newobj",
                            "text": "button",
                            "numinlets": 1,
                            "numoutlets": 1,
                            "outlettype": ["bang"],
                            "patching_rect": [200.0, 50.0, 50.0, 22.0]
                        }
                    },
                    {
                        "box": {
                            "id": "left",
                            "maxclass": "newobj",
                            "text": "print left",
                            "numinlets": 1,
                            "numoutlets": 0,
                            "outlettype": [],
                            "patching_rect": [50.0, 150.0, 50.0, 22.0]
                        }
                    },
                    {
                        "box": {
                            "id": "right",
                            "maxclass": "newobj",
                            "text": "print right",
                            "numinlets": 1,
                            "numoutlets": 0,
                            "outlettype": [],
                            "patching_rect": [350.0, 150.0, 50.0, 22.0]
                        }
                    }
                ],
                "lines": [
                    { "patchline": { "source": ["src", 0], "destination": ["left", 0] } },
                    { "patchline": { "source": ["src", 0], "destination": ["right", 0] } }
                ]
            }
        }"#;

        let pat = parse_maxpat(json).unwrap();
        let result = analyze(&pat, None).unwrap();

        assert_eq!(result.wires.len(), 3, "Expected 3 wires");

        // button first
        assert!(result.wires[0].expr.contains("button"));

        // Right (X=350) should come before Left (X=50) — X descending
        assert!(
            result.wires[1].expr.contains("right"),
            "Second wire should be print(right) [X=350], got: {}",
            result.wires[1].expr
        );
        assert!(
            result.wires[2].expr.contains("left"),
            "Third wire should be print(left) [X=50], got: {}",
            result.wires[2].expr
        );
    }

    /// Test: `[*~ 0.5]` with both inlets connected should preserve the default
    /// value 0.5 and emit all connections as direct connections.
    /// Expected: `mul~(0.5)` + `.in[0] = osc` + `.in[1] = env`
    #[test]
    fn test_default_preservation_mul_tilde_both_inlets() {
        let json = r#"{
            "patcher": {
                "boxes": [
                    {
                        "box": {
                            "id": "osc",
                            "maxclass": "newobj",
                            "text": "cycle~ 440",
                            "numinlets": 2,
                            "numoutlets": 1,
                            "outlettype": ["signal"],
                            "patching_rect": [100, 50, 80, 22]
                        }
                    },
                    {
                        "box": {
                            "id": "env",
                            "maxclass": "newobj",
                            "text": "line~",
                            "numinlets": 2,
                            "numoutlets": 2,
                            "outlettype": ["signal", "bang"],
                            "patching_rect": [250, 50, 60, 22]
                        }
                    },
                    {
                        "box": {
                            "id": "mul",
                            "maxclass": "newobj",
                            "text": "*~ 0.5",
                            "numinlets": 2,
                            "numoutlets": 1,
                            "outlettype": ["signal"],
                            "patching_rect": [100, 150, 80, 22]
                        }
                    },
                    {
                        "box": {
                            "id": "out",
                            "maxclass": "outlet~",
                            "numinlets": 1,
                            "numoutlets": 0,
                            "outlettype": [],
                            "patching_rect": [100, 250, 60, 22]
                        }
                    }
                ],
                "lines": [
                    { "patchline": { "source": ["osc", 0], "destination": ["mul", 0] } },
                    { "patchline": { "source": ["env", 0], "destination": ["mul", 1] } },
                    { "patchline": { "source": ["mul", 0], "destination": ["out", 0] } }
                ]
            }
        }"#;

        let pat = parse_maxpat(json).unwrap();
        let result = analyze(&pat, None).unwrap();

        // Find the mul~ wire
        let mul_wire = result
            .wires
            .iter()
            .find(|w| w.expr.contains("mul~"))
            .unwrap();
        // Should preserve the literal 0.5 as the only arg
        assert_eq!(
            mul_wire.expr, "mul~(0.5)",
            "Expected mul~(0.5), got: {}",
            mul_wire.expr
        );

        // Both connections should be emitted as direct connections
        let mul_directs: Vec<_> = result
            .direct_connections
            .iter()
            .filter(|dc| dc.target_wire == mul_wire.name)
            .collect();
        assert_eq!(
            mul_directs.len(),
            2,
            "Expected 2 direct connections for mul~, got {}",
            mul_directs.len()
        );

        // Check inlet 0 and inlet 1 are both present
        let has_inlet0 = mul_directs.iter().any(|dc| dc.inlet == 0);
        let has_inlet1 = mul_directs.iter().any(|dc| dc.inlet == 1);
        assert!(has_inlet0, "Expected direct connection on inlet 0");
        assert!(has_inlet1, "Expected direct connection on inlet 1");
    }

    /// Test: `[cycle~ 440]` with inlet 0 connected should preserve 440 and
    /// emit the connection as a direct connection.
    /// Expected: `cycle~(440)` + `.in[0] = freq`
    #[test]
    fn test_default_preservation_cycle_inlet0() {
        let json = r#"{
            "patcher": {
                "boxes": [
                    {
                        "box": {
                            "id": "inlet1",
                            "maxclass": "inlet",
                            "numinlets": 0,
                            "numoutlets": 1,
                            "outlettype": [""],
                            "comment": "freq",
                            "patching_rect": [50, 20, 30, 30]
                        }
                    },
                    {
                        "box": {
                            "id": "osc",
                            "maxclass": "newobj",
                            "text": "cycle~ 440",
                            "numinlets": 2,
                            "numoutlets": 1,
                            "outlettype": ["signal"],
                            "patching_rect": [50, 100, 80, 22]
                        }
                    },
                    {
                        "box": {
                            "id": "out",
                            "maxclass": "outlet~",
                            "numinlets": 1,
                            "numoutlets": 0,
                            "outlettype": [],
                            "patching_rect": [50, 200, 60, 22]
                        }
                    }
                ],
                "lines": [
                    { "patchline": { "source": ["inlet1", 0], "destination": ["osc", 0] } },
                    { "patchline": { "source": ["osc", 0], "destination": ["out", 0] } }
                ]
            }
        }"#;

        let pat = parse_maxpat(json).unwrap();
        let result = analyze(&pat, None).unwrap();

        // Find the cycle~ wire
        let osc_wire = result
            .wires
            .iter()
            .find(|w| w.expr.contains("cycle~"))
            .unwrap();
        // Should preserve the literal 440
        assert_eq!(
            osc_wire.expr, "cycle~(440)",
            "Expected cycle~(440), got: {}",
            osc_wire.expr
        );

        // Connection should be a direct connection
        let osc_directs: Vec<_> = result
            .direct_connections
            .iter()
            .filter(|dc| dc.target_wire == osc_wire.name)
            .collect();
        assert_eq!(
            osc_directs.len(),
            1,
            "Expected 1 direct connection for cycle~"
        );
        assert_eq!(osc_directs[0].inlet, 0);
    }

    /// Test: `[cycle~]` (no text args) with inlet 0 connected should use existing
    /// behavior — connection as inline arg.
    /// Expected: `cycle~(freq)` (no change from existing behavior)
    #[test]
    fn test_no_default_preservation_when_no_literals() {
        let json = r#"{
            "patcher": {
                "boxes": [
                    {
                        "box": {
                            "id": "inlet1",
                            "maxclass": "inlet",
                            "numinlets": 0,
                            "numoutlets": 1,
                            "outlettype": [""],
                            "comment": "freq",
                            "patching_rect": [50, 20, 30, 30]
                        }
                    },
                    {
                        "box": {
                            "id": "osc",
                            "maxclass": "newobj",
                            "text": "cycle~",
                            "numinlets": 2,
                            "numoutlets": 1,
                            "outlettype": ["signal"],
                            "patching_rect": [50, 100, 80, 22]
                        }
                    },
                    {
                        "box": {
                            "id": "out",
                            "maxclass": "outlet~",
                            "numinlets": 1,
                            "numoutlets": 0,
                            "outlettype": [],
                            "patching_rect": [50, 200, 60, 22]
                        }
                    }
                ],
                "lines": [
                    { "patchline": { "source": ["inlet1", 0], "destination": ["osc", 0] } },
                    { "patchline": { "source": ["osc", 0], "destination": ["out", 0] } }
                ]
            }
        }"#;

        let pat = parse_maxpat(json).unwrap();
        let result = analyze(&pat, None).unwrap();

        // Find the cycle~ wire
        let osc_wire = result
            .wires
            .iter()
            .find(|w| w.expr.contains("cycle~"))
            .unwrap();
        // Should use the connection as inline arg (no default preservation)
        assert_eq!(
            osc_wire.expr, "cycle~(freq)",
            "Expected cycle~(freq), got: {}",
            osc_wire.expr
        );

        // No direct connections for cycle~ (connection is inline)
        let osc_directs: Vec<_> = result
            .direct_connections
            .iter()
            .filter(|dc| dc.target_wire == osc_wire.name)
            .collect();
        assert_eq!(
            osc_directs.len(),
            0,
            "Expected 0 direct connections for cycle~ without text args"
        );
    }

    /// Test: `[pack 0 0 0]` with all inlets connected should preserve defaults
    /// and emit all connections as direct connections.
    /// This tests the pak/pack special case which was already correct.
    #[test]
    fn test_default_preservation_pack_all_connected() {
        let json = r#"{
            "patcher": {
                "boxes": [
                    {
                        "box": {
                            "id": "src0",
                            "maxclass": "newobj",
                            "text": "i",
                            "numinlets": 2,
                            "numoutlets": 1,
                            "outlettype": ["int"],
                            "patching_rect": [50, 50, 40, 22]
                        }
                    },
                    {
                        "box": {
                            "id": "src1",
                            "maxclass": "newobj",
                            "text": "i",
                            "numinlets": 2,
                            "numoutlets": 1,
                            "outlettype": ["int"],
                            "patching_rect": [150, 50, 40, 22]
                        }
                    },
                    {
                        "box": {
                            "id": "src2",
                            "maxclass": "newobj",
                            "text": "i",
                            "numinlets": 2,
                            "numoutlets": 1,
                            "outlettype": ["int"],
                            "patching_rect": [250, 50, 40, 22]
                        }
                    },
                    {
                        "box": {
                            "id": "pk",
                            "maxclass": "newobj",
                            "text": "pack 0 0 0",
                            "numinlets": 3,
                            "numoutlets": 1,
                            "outlettype": [""],
                            "patching_rect": [100, 150, 80, 22]
                        }
                    },
                    {
                        "box": {
                            "id": "out",
                            "maxclass": "outlet",
                            "numinlets": 1,
                            "numoutlets": 0,
                            "outlettype": [],
                            "patching_rect": [100, 250, 60, 22]
                        }
                    }
                ],
                "lines": [
                    { "patchline": { "source": ["src0", 0], "destination": ["pk", 0] } },
                    { "patchline": { "source": ["src1", 0], "destination": ["pk", 1] } },
                    { "patchline": { "source": ["src2", 0], "destination": ["pk", 2] } },
                    { "patchline": { "source": ["pk", 0], "destination": ["out", 0] } }
                ]
            }
        }"#;

        let pat = parse_maxpat(json).unwrap();
        let result = analyze(&pat, None).unwrap();

        // Find the pack wire
        let pack_wire = result
            .wires
            .iter()
            .find(|w| w.expr.contains("pack"))
            .unwrap();
        // Should preserve all literal defaults
        assert_eq!(
            pack_wire.expr, "pack(0, 0, 0)",
            "Expected pack(0, 0, 0), got: {}",
            pack_wire.expr
        );

        // All 3 connections should be direct connections
        let pack_directs: Vec<_> = result
            .direct_connections
            .iter()
            .filter(|dc| dc.target_wire == pack_wire.name)
            .collect();
        assert_eq!(
            pack_directs.len(),
            3,
            "Expected 3 direct connections for pack"
        );
    }

    /// Test: connection on an inlet BEYOND the literal arg range should not trigger
    /// default preservation. E.g., `[delay~ 100]` with connection only on inlet 1
    /// (literal "100" at position 0, connection at position 1 — no overlap).
    #[test]
    fn test_no_overlap_when_connection_beyond_literal_range() {
        let json = r#"{
            "patcher": {
                "boxes": [
                    {
                        "box": {
                            "id": "src",
                            "maxclass": "newobj",
                            "text": "sig~",
                            "numinlets": 1,
                            "numoutlets": 1,
                            "outlettype": ["signal"],
                            "patching_rect": [200, 50, 60, 22]
                        }
                    },
                    {
                        "box": {
                            "id": "del",
                            "maxclass": "newobj",
                            "text": "delay~ 100",
                            "numinlets": 2,
                            "numoutlets": 1,
                            "outlettype": ["signal"],
                            "patching_rect": [100, 150, 80, 22]
                        }
                    },
                    {
                        "box": {
                            "id": "out",
                            "maxclass": "outlet~",
                            "numinlets": 1,
                            "numoutlets": 0,
                            "outlettype": [],
                            "patching_rect": [100, 250, 60, 22]
                        }
                    }
                ],
                "lines": [
                    { "patchline": { "source": ["src", 0], "destination": ["del", 1] } },
                    { "patchline": { "source": ["del", 0], "destination": ["out", 0] } }
                ]
            }
        }"#;

        let pat = parse_maxpat(json).unwrap();
        let result = analyze(&pat, None).unwrap();

        // Find the delay~ wire
        let del_wire = result
            .wires
            .iter()
            .find(|w| w.expr.contains("delay~"))
            .unwrap();
        // literal "100" at position 0, connection at position 1 — no overlap
        // Connection should be inline as arg, not a direct connection
        assert!(
            del_wire.expr.contains("delay~(100,") || del_wire.expr.contains("delay~(100, "),
            "Expected delay~ with 100 as first arg and connection inline, got: {}",
            del_wire.expr
        );
    }

    #[test]
    fn test_rnbo_inport_outport_classification() {
        let json = r#"{
            "patcher": {
                "classnamespace": "rnbo",
                "boxes": [
                    { "box": { "id": "obj-1", "maxclass": "newobj", "text": "inport freq", "numinlets": 0, "numoutlets": 1, "outlettype": [""], "patching_rect": [50, 50, 80, 22] } },
                    { "box": { "id": "obj-2", "maxclass": "newobj", "text": "outport audio_out", "numinlets": 1, "numoutlets": 0, "outlettype": [], "patching_rect": [50, 200, 80, 22] } },
                    { "box": { "id": "obj-3", "maxclass": "newobj", "text": "cycle~", "numinlets": 2, "numoutlets": 1, "outlettype": ["signal"], "patching_rect": [50, 120, 80, 22] } }
                ],
                "lines": [
                    { "patchline": { "source": ["obj-1", 0], "destination": ["obj-3", 0] } },
                    { "patchline": { "source": ["obj-3", 0], "destination": ["obj-2", 0] } }
                ]
            }
        }"#;

        let pat = parse_maxpat(json).unwrap();
        let result = analyze(&pat, None).unwrap();

        // inport should become an in declaration
        assert_eq!(result.in_decls.len(), 1);
        assert_eq!(result.in_decls[0].name, "freq");
        assert_eq!(result.in_decls[0].port_type, "float");

        // outport should become an out declaration
        assert_eq!(result.out_decls.len(), 1);
        assert_eq!(result.out_decls[0].name, "audio_out");
        assert_eq!(result.out_decls[0].port_type, "float");

        // cycle~ should be a wire, not classified as inlet/outlet
        assert_eq!(result.wires.len(), 1);
        assert!(result.wires[0].expr.contains("cycle~"));
    }

    #[test]
    fn test_rnbo_signal_ports_classification() {
        let json = r#"{
            "patcher": {
                "classnamespace": "rnbo",
                "boxes": [
                    { "box": { "id": "obj-1", "maxclass": "newobj", "text": "in~ 1", "numinlets": 0, "numoutlets": 1, "outlettype": ["signal"], "patching_rect": [50, 50, 80, 22] } },
                    { "box": { "id": "obj-2", "maxclass": "newobj", "text": "out~ 1", "numinlets": 1, "numoutlets": 0, "outlettype": [], "patching_rect": [50, 200, 80, 22] } },
                    { "box": { "id": "obj-3", "maxclass": "newobj", "text": "gain~", "numinlets": 2, "numoutlets": 1, "outlettype": ["signal"], "patching_rect": [50, 120, 80, 22] } }
                ],
                "lines": [
                    { "patchline": { "source": ["obj-1", 0], "destination": ["obj-3", 0] } },
                    { "patchline": { "source": ["obj-3", 0], "destination": ["obj-2", 0] } }
                ]
            }
        }"#;

        let pat = parse_maxpat(json).unwrap();
        let result = analyze(&pat, None).unwrap();

        // in~ should become a signal in declaration
        assert_eq!(result.in_decls.len(), 1);
        assert_eq!(result.in_decls[0].port_type, "signal");

        // out~ should become a signal out declaration
        assert_eq!(result.out_decls.len(), 1);
        assert_eq!(result.out_decls[0].port_type, "signal");

        // gain~ should be a wire
        assert_eq!(result.wires.len(), 1);
        assert!(result.wires[0].expr.contains("gain~"));
    }

    #[test]
    fn test_rnbo_ports_not_classified_in_standard_patcher() {
        // In a standard (non-RNBO) patcher, "inport" and "outport" are just regular newobj boxes
        let json = r#"{
            "patcher": {
                "boxes": [
                    { "box": { "id": "obj-1", "maxclass": "newobj", "text": "inport freq", "numinlets": 0, "numoutlets": 1, "outlettype": [""], "patching_rect": [50, 50, 80, 22] } },
                    { "box": { "id": "obj-2", "maxclass": "newobj", "text": "outport out", "numinlets": 1, "numoutlets": 0, "outlettype": [], "patching_rect": [50, 200, 80, 22] } }
                ],
                "lines": [
                    { "patchline": { "source": ["obj-1", 0], "destination": ["obj-2", 0] } }
                ]
            }
        }"#;

        let pat = parse_maxpat(json).unwrap();
        let result = analyze(&pat, None).unwrap();

        // Should NOT be classified as inlets/outlets — they should be wires
        assert_eq!(result.in_decls.len(), 0);
        assert_eq!(result.out_decls.len(), 0);
    }

    #[test]
    fn test_rnbo_mixed_ports() {
        // RNBO patcher with both control (inport/outport) and signal (in~/out~) ports
        let json = r#"{
            "patcher": {
                "classnamespace": "rnbo",
                "boxes": [
                    { "box": { "id": "obj-1", "maxclass": "newobj", "text": "inport freq", "numinlets": 0, "numoutlets": 1, "outlettype": [""], "patching_rect": [50, 50, 80, 22] } },
                    { "box": { "id": "obj-2", "maxclass": "newobj", "text": "in~ 1", "numinlets": 0, "numoutlets": 1, "outlettype": ["signal"], "patching_rect": [200, 50, 80, 22] } },
                    { "box": { "id": "obj-3", "maxclass": "newobj", "text": "outport data_out", "numinlets": 1, "numoutlets": 0, "outlettype": [], "patching_rect": [50, 200, 80, 22] } },
                    { "box": { "id": "obj-4", "maxclass": "newobj", "text": "out~ 1", "numinlets": 1, "numoutlets": 0, "outlettype": [], "patching_rect": [200, 200, 80, 22] } },
                    { "box": { "id": "obj-5", "maxclass": "newobj", "text": "cycle~", "numinlets": 2, "numoutlets": 1, "outlettype": ["signal"], "patching_rect": [100, 120, 80, 22] } }
                ],
                "lines": [
                    { "patchline": { "source": ["obj-1", 0], "destination": ["obj-5", 0] } },
                    { "patchline": { "source": ["obj-5", 0], "destination": ["obj-4", 0] } }
                ]
            }
        }"#;

        let pat = parse_maxpat(json).unwrap();
        let result = analyze(&pat, None).unwrap();

        // 2 in declarations: inport (float) and in~ (signal), sorted by X
        assert_eq!(result.in_decls.len(), 2);
        // Sorted by X: inport at x=50 first, in~ at x=200 second
        assert_eq!(result.in_decls[0].name, "freq");
        assert_eq!(result.in_decls[0].port_type, "float");
        assert_eq!(result.in_decls[1].port_type, "signal");

        // 2 out declarations: outport (float) and out~ (signal), sorted by X
        assert_eq!(result.out_decls.len(), 2);
        assert_eq!(result.out_decls[0].name, "data_out");
        assert_eq!(result.out_decls[0].port_type, "float");
        assert_eq!(result.out_decls[1].port_type, "signal");
    }

    #[test]
    fn test_rnbo_subpatcher_name_extraction() {
        use std::sync::atomic::AtomicU32;

        let b = MaxBox {
            id: "obj-1".to_string(),
            maxclass: "newobj".to_string(),
            text: Some("rnbo~ mysynth".to_string()),
            numinlets: 2,
            numoutlets: 1,
            outlettype: vec!["signal".to_string()],
            comment: None,
            varname: None,
            patching_rect: [0.0, 0.0, 100.0, 22.0],
            embedded_patcher: None,
            extra_attrs: vec![],
            code: None,
        };

        let counter = AtomicU32::new(1);
        let name = extract_subpatcher_name(&b, "parent", &counter);
        assert_eq!(name, "parent_mysynth");
    }

    #[test]
    fn test_rnbo_subpatcher_io_counting() {
        let json = r#"{
            "patcher": {
                "boxes": [
                    {
                        "box": {
                            "id": "obj-1",
                            "maxclass": "newobj",
                            "text": "rnbo~ mysynth",
                            "numinlets": 2,
                            "numoutlets": 2,
                            "outlettype": ["signal", ""],
                            "patcher": {
                                "classnamespace": "rnbo",
                                "boxes": [
                                    { "box": { "id": "sub-1", "maxclass": "newobj", "text": "inport freq", "numinlets": 0, "numoutlets": 1, "outlettype": [""], "patching_rect": [50, 50, 80, 22] } },
                                    { "box": { "id": "sub-2", "maxclass": "newobj", "text": "in~ 1", "numinlets": 0, "numoutlets": 1, "outlettype": ["signal"], "patching_rect": [200, 50, 80, 22] } },
                                    { "box": { "id": "sub-3", "maxclass": "newobj", "text": "out~ 1", "numinlets": 1, "numoutlets": 0, "outlettype": [], "patching_rect": [50, 200, 80, 22] } },
                                    { "box": { "id": "sub-4", "maxclass": "newobj", "text": "outport data", "numinlets": 1, "numoutlets": 0, "outlettype": [], "patching_rect": [200, 200, 80, 22] } },
                                    { "box": { "id": "sub-5", "maxclass": "newobj", "text": "cycle~", "numinlets": 2, "numoutlets": 1, "outlettype": ["signal"], "patching_rect": [100, 120, 80, 22] } }
                                ],
                                "lines": [
                                    { "patchline": { "source": ["sub-1", 0], "destination": ["sub-5", 0] } },
                                    { "patchline": { "source": ["sub-5", 0], "destination": ["sub-3", 0] } }
                                ]
                            }
                        }
                    },
                    { "box": { "id": "obj-2", "maxclass": "outlet~", "numinlets": 1, "numoutlets": 0, "outlettype": [], "patching_rect": [50, 300, 60, 22] } }
                ],
                "lines": [
                    { "patchline": { "source": ["obj-1", 0], "destination": ["obj-2", 0] } }
                ]
            }
        }"#;

        let pat = parse_maxpat(json).unwrap();
        let (main_patch, subpatchers) = analyze_recursive(&pat, "test", None).unwrap();

        // Should extract 1 subpatcher
        assert_eq!(subpatchers.len(), 1);
        let (sub_name, sub_patch) = &subpatchers[0];
        assert!(
            sub_name.contains("mysynth"),
            "Expected subpatcher name to contain 'mysynth', got: {}",
            sub_name
        );

        // The subpatcher should have 2 inlets (inport + in~) and 2 outlets (out~ + outport)
        assert_eq!(sub_patch.in_decls.len(), 2);
        assert_eq!(sub_patch.out_decls.len(), 2);

        // Verify RNBO port types
        let signal_in = sub_patch.in_decls.iter().find(|d| d.port_type == "signal");
        assert!(signal_in.is_some(), "Should have a signal inlet from in~");
        let float_in = sub_patch.in_decls.iter().find(|d| d.name == "freq");
        assert!(
            float_in.is_some(),
            "Should have a float inlet named 'freq' from inport"
        );

        let signal_out = sub_patch.out_decls.iter().find(|d| d.port_type == "signal");
        assert!(
            signal_out.is_some(),
            "Should have a signal outlet from out~"
        );
        let float_out = sub_patch.out_decls.iter().find(|d| d.name == "data");
        assert!(
            float_out.is_some(),
            "Should have a float outlet named 'data' from outport"
        );

        // Main patch should reference the subpatcher
        assert_eq!(main_patch.wires.len(), 1);
        assert!(
            main_patch.wires[0].expr.contains("mysynth"),
            "Main wire should reference subpatcher name"
        );
    }

    #[test]
    fn test_rnbo_full_decompile() {
        // Full integration test: decompile a mini RNBO patcher
        let json = r#"{
            "patcher": {
                "classnamespace": "rnbo",
                "boxes": [
                    { "box": { "id": "obj-1", "maxclass": "newobj", "text": "inport freq", "numinlets": 0, "numoutlets": 1, "outlettype": [""], "patching_rect": [50, 50, 80, 22] } },
                    { "box": { "id": "obj-2", "maxclass": "newobj", "text": "out~ 1", "numinlets": 1, "numoutlets": 0, "outlettype": [], "patching_rect": [50, 200, 80, 22] } },
                    { "box": { "id": "obj-3", "maxclass": "newobj", "text": "cycle~", "numinlets": 2, "numoutlets": 1, "outlettype": ["signal"], "patching_rect": [50, 120, 80, 22] } }
                ],
                "lines": [
                    { "patchline": { "source": ["obj-1", 0], "destination": ["obj-3", 0] } },
                    { "patchline": { "source": ["obj-3", 0], "destination": ["obj-2", 0] } }
                ]
            }
        }"#;

        let pat = parse_maxpat(json).unwrap();
        let result = analyze(&pat, None).unwrap();

        // Should have 1 in decl (inport freq → float)
        assert_eq!(result.in_decls.len(), 1);
        assert_eq!(result.in_decls[0].name, "freq");
        assert_eq!(result.in_decls[0].port_type, "float");
        assert_eq!(result.in_decls[0].index, 0);

        // Should have 1 out decl (out~ 1 → signal)
        assert_eq!(result.out_decls.len(), 1);
        assert_eq!(result.out_decls[0].port_type, "signal");
        assert_eq!(result.out_decls[0].index, 0);

        // Should have 1 wire (cycle~)
        assert_eq!(result.wires.len(), 1);
        assert!(result.wires[0].expr.contains("cycle~"));

        // Should have 1 out assignment
        assert_eq!(result.out_assignments.len(), 1);
        assert_eq!(result.out_assignments[0].index, 0);
    }

    // -----------------------------------------------------------------------
    // Codebox tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_v8_codebox_detected_as_wire_candidate() {
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

        let maxpat = crate::parser::parse_maxpat(json).unwrap();
        let result = analyze(&maxpat, None).unwrap();

        // v8.codebox should become a wire with code extracted
        assert_eq!(result.wires.len(), 2, "Expected 2 wires (codebox + print)");

        // First wire should be the codebox with a file reference
        let codebox_wire = &result.wires[0];
        assert!(
            codebox_wire.expr.starts_with("v8.codebox("),
            "Expected v8.codebox(...) expression, got: {}",
            codebox_wire.expr
        );
        assert!(
            codebox_wire.expr.contains(".js"),
            "Expected .js filename in expression, got: {}",
            codebox_wire.expr
        );

        // code_files should contain the extracted code
        assert_eq!(result.code_files.len(), 1, "Expected 1 code file");
        let (filename, content) = &result.code_files[0];
        assert!(
            filename.ends_with(".js"),
            "Filename should end with .js: {}",
            filename
        );
        assert!(
            content.contains("function bang()"),
            "Code content should contain the JS code: {}",
            content
        );
    }

    #[test]
    fn test_codebox_no_code_field() {
        // A v8.codebox without a code field should not generate code_files
        let json = r#"{
            "patcher": {
                "classnamespace": "box",
                "boxes": [
                    {
                        "box": {
                            "id": "obj-1",
                            "maxclass": "v8.codebox",
                            "text": "",
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

        let maxpat = crate::parser::parse_maxpat(json).unwrap();
        let result = analyze(&maxpat, None).unwrap();

        assert!(
            result.code_files.is_empty(),
            "No code_files expected when code field is absent"
        );
    }

    #[test]
    fn test_extract_codebox_code_v8() {
        let b = MaxBox {
            id: "obj-1".into(),
            maxclass: "v8.codebox".into(),
            text: Some("".into()),
            numinlets: 1,
            numoutlets: 1,
            outlettype: vec!["".into()],
            comment: None,
            varname: None,
            patching_rect: [0.0; 4],
            embedded_patcher: None,
            extra_attrs: vec![],
            code: Some("function bang() {\n  outlet(0, 42);\n}\n".into()),
        };

        let (expr, code_file) = extract_codebox_code(&b, "my_script", "v8.codebox()");
        assert_eq!(expr, "v8.codebox(\"my_script.js\")");
        assert!(code_file.is_some());
        let (filename, content) = code_file.unwrap();
        assert_eq!(filename, "my_script.js");
        assert!(content.contains("function bang()"));
    }

    #[test]
    fn test_extract_codebox_code_genexpr() {
        let b = MaxBox {
            id: "obj-1".into(),
            maxclass: "codebox".into(),
            text: Some("".into()),
            numinlets: 1,
            numoutlets: 1,
            outlettype: vec!["".into()],
            comment: None,
            varname: None,
            patching_rect: [0.0; 4],
            embedded_patcher: None,
            extra_attrs: vec![],
            code: Some("out1 = in1 * 0.5;".into()),
        };

        let (expr, code_file) = extract_codebox_code(&b, "my_gen", "codebox()");
        assert_eq!(expr, "codebox(\"my_gen.genexpr\")");
        assert!(code_file.is_some());
        let (filename, content) = code_file.unwrap();
        assert_eq!(filename, "my_gen.genexpr");
        assert_eq!(content, "out1 = in1 * 0.5;");
    }

    #[test]
    fn test_extract_codebox_code_regular_box_passthrough() {
        let b = MaxBox {
            id: "obj-1".into(),
            maxclass: "newobj".into(),
            text: Some("cycle~ 440".into()),
            numinlets: 2,
            numoutlets: 1,
            outlettype: vec!["signal".into()],
            comment: None,
            varname: None,
            patching_rect: [0.0; 4],
            embedded_patcher: None,
            extra_attrs: vec![],
            code: None,
        };

        let (expr, code_file) = extract_codebox_code(&b, "w_1", "cycle~(440)");
        assert_eq!(expr, "cycle~(440)");
        assert!(code_file.is_none());
    }

    #[test]
    fn gen_inlet_recognition() {
        let b = MaxBox {
            id: "obj-1".into(),
            maxclass: "newobj".into(),
            text: Some("in 1".into()),
            numinlets: 0,
            numoutlets: 1,
            outlettype: vec!["".into()],
            comment: None,
            varname: None,
            patching_rect: [50.0, 50.0, 80.0, 22.0],
            embedded_patcher: None,
            extra_attrs: vec![],
            code: None,
        };
        assert!(is_gen_inlet(&b, true));
        assert!(!is_gen_inlet(&b, false));
        assert!(!is_gen_outlet(&b, true));
    }

    #[test]
    fn gen_outlet_recognition() {
        let b = MaxBox {
            id: "obj-2".into(),
            maxclass: "newobj".into(),
            text: Some("out 1".into()),
            numinlets: 1,
            numoutlets: 0,
            outlettype: vec![],
            comment: None,
            varname: None,
            patching_rect: [50.0, 200.0, 80.0, 22.0],
            embedded_patcher: None,
            extra_attrs: vec![],
            code: None,
        };
        assert!(is_gen_outlet(&b, true));
        assert!(!is_gen_outlet(&b, false));
        assert!(!is_gen_inlet(&b, true));
    }

    #[test]
    fn gen_inlet_not_matched_for_non_newobj() {
        let b = MaxBox {
            id: "obj-1".into(),
            maxclass: "inlet".into(),
            text: Some("in 1".into()),
            numinlets: 0,
            numoutlets: 1,
            outlettype: vec!["".into()],
            comment: None,
            varname: None,
            patching_rect: [50.0, 50.0, 80.0, 22.0],
            embedded_patcher: None,
            extra_attrs: vec![],
            code: None,
        };
        // maxclass is "inlet" not "newobj", so should not match
        assert!(!is_gen_inlet(&b, true));
    }

    #[test]
    fn analyze_gen_patcher() {
        let json = r#"{
            "patcher": {
                "classnamespace": "dsp.gen",
                "boxes": [
                    { "box": { "id": "obj-1", "maxclass": "newobj", "text": "in 1", "numinlets": 0, "numoutlets": 1, "outlettype": [""], "patching_rect": [50, 50, 80, 22] } },
                    { "box": { "id": "obj-2", "maxclass": "newobj", "text": "in 2", "numinlets": 0, "numoutlets": 1, "outlettype": [""], "patching_rect": [150, 50, 80, 22] } },
                    { "box": { "id": "obj-3", "maxclass": "newobj", "text": "* 0.5", "numinlets": 2, "numoutlets": 1, "outlettype": [""], "patching_rect": [50, 120, 80, 22] } },
                    { "box": { "id": "obj-4", "maxclass": "newobj", "text": "out 1", "numinlets": 1, "numoutlets": 0, "outlettype": [], "patching_rect": [50, 200, 80, 22] } }
                ],
                "lines": [
                    { "patchline": { "source": ["obj-1", 0], "destination": ["obj-3", 0] } },
                    { "patchline": { "source": ["obj-2", 0], "destination": ["obj-3", 1] } },
                    { "patchline": { "source": ["obj-3", 0], "destination": ["obj-4", 0] } }
                ]
            }
        }"#;

        let maxpat = parse_maxpat(json).unwrap();
        let patch = analyze(&maxpat, None).unwrap();

        // Should have 2 signal inlets
        assert_eq!(patch.in_decls.len(), 2);
        assert_eq!(patch.in_decls[0].port_type, "signal");
        assert_eq!(patch.in_decls[1].port_type, "signal");

        // Should have 1 signal outlet
        assert_eq!(patch.out_decls.len(), 1);
        assert_eq!(patch.out_decls[0].port_type, "signal");

        // Should have 1 wire (the * 0.5 object)
        assert_eq!(patch.wires.len(), 1);
    }

    // -----------------------------------------------------------------------
    // .uiflutmax tests: decorative attr classification and UI entry collection
    // -----------------------------------------------------------------------

    #[test]
    fn test_is_decorative_attr() {
        // Decorative attrs
        assert!(is_decorative_attr("bgcolor"));
        assert!(is_decorative_attr("textcolor"));
        assert!(is_decorative_attr("fontsize"));
        assert!(is_decorative_attr("fontname"));
        assert!(is_decorative_attr("bordercolor"));
        assert!(is_decorative_attr("background"));
        assert!(is_decorative_attr("gradient"));
        assert!(is_decorative_attr("textjustification"));
        assert!(is_decorative_attr("presentation"));
        assert!(is_decorative_attr("presentation_rect"));

        // Functional attrs (should NOT be decorative)
        assert!(!is_decorative_attr("minimum"));
        assert!(!is_decorative_attr("maximum"));
        assert!(!is_decorative_attr("mode"));
        assert!(!is_decorative_attr("parameter_enable"));
        assert!(!is_decorative_attr("domain"));
        assert!(!is_decorative_attr("range"));
    }

    #[test]
    fn test_build_box_attrs_split_separates_decorative() {
        use serde_json::json;
        let b = MaxBox {
            id: "obj-1".into(),
            maxclass: "flonum".into(),
            text: None,
            numinlets: 1,
            numoutlets: 2,
            outlettype: vec!["".into(), "bang".into()],
            comment: None,
            varname: None,
            patching_rect: [100.0, 200.0, 50.0, 22.0],
            embedded_patcher: None,
            extra_attrs: vec![
                ("minimum".into(), json!(0.0)),
                ("maximum".into(), json!(100.0)),
                ("bgcolor".into(), json!([0.0, 0.0, 0.0, 1.0])),
                ("textcolor".into(), json!([1.0, 1.0, 1.0, 1.0])),
            ],
            code: None,
        };
        let (functional, decorative) = build_box_attrs_split(&b);
        // minimum and maximum are functional
        assert_eq!(functional.len(), 2);
        assert!(functional.iter().any(|(k, _)| k == "minimum"));
        assert!(functional.iter().any(|(k, _)| k == "maximum"));
        // bgcolor and textcolor are decorative
        assert_eq!(decorative.len(), 2);
        assert!(decorative.iter().any(|(k, _)| k == "bgcolor"));
        assert!(decorative.iter().any(|(k, _)| k == "textcolor"));
    }

    #[test]
    fn test_analyze_collects_ui_entries() {
        let json = r#"{
            "patcher": {
                "rect": [100, 100, 640, 480],
                "boxes": [
                    {
                        "box": {
                            "id": "obj-1",
                            "maxclass": "newobj",
                            "text": "cycle~ 440",
                            "numinlets": 2,
                            "numoutlets": 1,
                            "outlettype": ["signal"],
                            "patching_rect": [150, 200, 80, 22]
                        }
                    },
                    {
                        "box": {
                            "id": "obj-2",
                            "maxclass": "outlet~",
                            "numinlets": 1,
                            "numoutlets": 0,
                            "outlettype": [],
                            "patching_rect": [150, 300, 30, 30]
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

        let maxpat = parse_maxpat(json).unwrap();
        let patch = analyze(&maxpat, None).unwrap();

        // Should have ui_entries for the wire and the outlet
        assert!(!patch.ui_entries.is_empty(), "Should have UI entries");

        // Check that patcher rect was captured
        assert!(patch.patcher_rect.is_some(), "Should have patcher rect");
        let rect = patch.patcher_rect.unwrap();
        assert_eq!(rect[0], 100.0);
        assert_eq!(rect[2], 640.0);

        // Wire entry should have correct position
        let wire_entry = patch
            .ui_entries
            .iter()
            .find(|e| e.name == patch.wires[0].name);
        assert!(wire_entry.is_some(), "Should have UI entry for the wire");
        let wire_entry = wire_entry.unwrap();
        assert_eq!(wire_entry.rect[0], 150.0);
        assert_eq!(wire_entry.rect[1], 200.0);
    }

    #[test]
    fn test_analyze_decorative_attrs_in_ui_entries_not_in_wire() {
        // A flonum with both functional and decorative attrs
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
                            "outlettype": [],
                            "patching_rect": [100, 200, 30, 30]
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

        let maxpat = parse_maxpat(json).unwrap();
        let patch = analyze(&maxpat, None).unwrap();

        // Wire should only have functional attrs (minimum, maximum)
        assert_eq!(patch.wires.len(), 1);
        let wire = &patch.wires[0];
        assert!(
            wire.attrs.iter().all(|(k, _)| !is_decorative_attr(k)),
            "Wire attrs should not contain decorative: {:?}",
            wire.attrs
        );
        assert!(
            wire.attrs.iter().any(|(k, _)| k == "minimum"),
            "Wire should have minimum attr"
        );

        // UI entry should have decorative attrs
        let ui_entry = patch.ui_entries.iter().find(|e| e.name == wire.name);
        assert!(ui_entry.is_some());
        let ui_entry = ui_entry.unwrap();
        assert!(
            ui_entry
                .decorative_attrs
                .iter()
                .any(|(k, _)| k == "bgcolor"),
            "UI entry should have bgcolor: {:?}",
            ui_entry.decorative_attrs
        );
    }

    #[test]
    fn named_args_from_objdb() {
        use flutmax_objdb::{
            InletSpec, Module, ObjectDb, ObjectDef, OutletSpec, PortDef, PortType as ObjPortType,
        };

        let mut db = ObjectDb::new();
        db.insert(ObjectDef {
            name: "cycle~".to_string(),
            module: Module::Msp,
            category: String::new(),
            digest: String::new(),
            inlets: InletSpec::Fixed(vec![
                PortDef {
                    id: 0,
                    port_type: ObjPortType::SignalFloat,
                    is_hot: true,
                    description: "Frequency".to_string(),
                },
                PortDef {
                    id: 1,
                    port_type: ObjPortType::SignalFloat,
                    is_hot: false,
                    description: "Phase offset".to_string(),
                },
            ]),
            outlets: OutletSpec::Fixed(vec![PortDef {
                id: 0,
                port_type: ObjPortType::Signal,
                is_hot: false,
                description: "Output".to_string(),
            }]),
            args: vec![],
        });

        // cycle~(440) with inlet 0 connected
        let json = r#"{
            "patcher": {
                "boxes": [
                    { "box": { "id": "obj-1", "maxclass": "newobj", "text": "cycle~ 440", "numinlets": 2, "numoutlets": 1 } },
                    { "box": { "id": "obj-2", "maxclass": "outlet~", "numinlets": 1, "numoutlets": 0 } }
                ],
                "lines": [
                    { "patchline": { "source": ["obj-1", 0], "destination": ["obj-2", 0] } }
                ]
            }
        }"#;

        // Without objdb: positional
        let maxpat = parse_maxpat(json).unwrap();
        let patch_no_objdb = analyze(&maxpat, None).unwrap();
        assert_eq!(patch_no_objdb.wires.len(), 1);
        assert_eq!(patch_no_objdb.wires[0].expr, "cycle~(440)");

        // With objdb: named args
        let patch_with_objdb = analyze(&maxpat, Some(&db)).unwrap();
        assert_eq!(patch_with_objdb.wires.len(), 1);
        assert_eq!(patch_with_objdb.wires[0].expr, "cycle~(frequency: 440)");
    }

    #[test]
    fn named_args_with_connections() {
        use flutmax_objdb::{
            InletSpec, Module, ObjectDb, ObjectDef, OutletSpec, PortDef, PortType as ObjPortType,
        };

        let mut db = ObjectDb::new();
        db.insert(ObjectDef {
            name: "biquad~".to_string(),
            module: Module::Msp,
            category: String::new(),
            digest: String::new(),
            inlets: InletSpec::Fixed(vec![
                PortDef {
                    id: 0,
                    port_type: ObjPortType::Signal,
                    is_hot: true,
                    description: "Input".to_string(),
                },
                PortDef {
                    id: 1,
                    port_type: ObjPortType::SignalFloat,
                    is_hot: false,
                    description: "Frequency".to_string(),
                },
                PortDef {
                    id: 2,
                    port_type: ObjPortType::SignalFloat,
                    is_hot: false,
                    description: "Q".to_string(),
                },
            ]),
            outlets: OutletSpec::Fixed(vec![PortDef {
                id: 0,
                port_type: ObjPortType::Signal,
                is_hot: false,
                description: "Output".to_string(),
            }]),
            args: vec![],
        });

        // biquad~ with inlet 0 and 1 connected via wires, inlet 2 as literal
        let json = r#"{
            "patcher": {
                "boxes": [
                    { "box": { "id": "obj-1", "maxclass": "inlet~", "numinlets": 0, "numoutlets": 1 } },
                    { "box": { "id": "obj-2", "maxclass": "inlet", "numinlets": 0, "numoutlets": 1 } },
                    { "box": { "id": "obj-3", "maxclass": "newobj", "text": "biquad~", "numinlets": 3, "numoutlets": 1 } },
                    { "box": { "id": "obj-4", "maxclass": "outlet~", "numinlets": 1, "numoutlets": 0 } }
                ],
                "lines": [
                    { "patchline": { "source": ["obj-1", 0], "destination": ["obj-3", 0] } },
                    { "patchline": { "source": ["obj-2", 0], "destination": ["obj-3", 1] } },
                    { "patchline": { "source": ["obj-3", 0], "destination": ["obj-4", 0] } }
                ]
            }
        }"#;

        let maxpat = parse_maxpat(json).unwrap();
        let patch = analyze(&maxpat, Some(&db)).unwrap();
        assert_eq!(patch.wires.len(), 1);
        // biquad~ with input connected to port_0, frequency to port_1
        assert!(
            patch.wires[0].expr.contains("input:"),
            "Expected named arg 'input:' in expr: {}",
            patch.wires[0].expr
        );
        assert!(
            patch.wires[0].expr.contains("frequency:"),
            "Expected named arg 'frequency:' in expr: {}",
            patch.wires[0].expr
        );
    }

    #[test]
    fn named_args_skipped_for_unknown_objects() {
        // Object not in objdb — should stay positional
        let db = flutmax_objdb::ObjectDb::new();

        let json = r#"{
            "patcher": {
                "boxes": [
                    { "box": { "id": "obj-1", "maxclass": "newobj", "text": "unknown_obj~ 440", "numinlets": 1, "numoutlets": 1 } },
                    { "box": { "id": "obj-2", "maxclass": "outlet~", "numinlets": 1, "numoutlets": 0 } }
                ],
                "lines": [
                    { "patchline": { "source": ["obj-1", 0], "destination": ["obj-2", 0] } }
                ]
            }
        }"#;

        let maxpat = parse_maxpat(json).unwrap();
        let patch = analyze(&maxpat, Some(&db)).unwrap();
        // No named args — object not in db
        assert_eq!(patch.wires[0].expr, "unknown_obj~(440)");
    }

    #[test]
    fn named_args_skipped_for_variable_inlets() {
        use flutmax_objdb::{
            InletSpec, Module, ObjectDb, ObjectDef, OutletSpec, PortDef, PortType as ObjPortType,
        };

        let mut db = ObjectDb::new();
        db.insert(ObjectDef {
            name: "pack".to_string(),
            module: Module::Max,
            category: String::new(),
            digest: String::new(),
            inlets: InletSpec::Variable {
                defaults: vec![
                    PortDef {
                        id: 0,
                        port_type: ObjPortType::Int,
                        is_hot: true,
                        description: "Value 1".to_string(),
                    },
                    PortDef {
                        id: 1,
                        port_type: ObjPortType::Int,
                        is_hot: false,
                        description: "Value 2".to_string(),
                    },
                ],
                min_inlets: 2,
            },
            outlets: OutletSpec::Fixed(vec![PortDef {
                id: 0,
                port_type: ObjPortType::List,
                is_hot: false,
                description: "Output".to_string(),
            }]),
            args: vec![],
        });

        let json = r#"{
            "patcher": {
                "boxes": [
                    { "box": { "id": "obj-1", "maxclass": "newobj", "text": "pack 0 0", "numinlets": 2, "numoutlets": 1 } },
                    { "box": { "id": "obj-2", "maxclass": "outlet", "numinlets": 1, "numoutlets": 0 } }
                ],
                "lines": [
                    { "patchline": { "source": ["obj-1", 0], "destination": ["obj-2", 0] } }
                ]
            }
        }"#;

        let maxpat = parse_maxpat(json).unwrap();
        let patch = analyze(&maxpat, Some(&db)).unwrap();
        // Variable inlets — no named args
        assert!(
            !patch.wires[0].expr.contains(":"),
            "pack should use positional args: {}",
            patch.wires[0].expr
        );
    }

    #[test]
    fn normalize_inlet_name_cases() {
        assert_eq!(
            normalize_inlet_name("Frequency"),
            Some("frequency".to_string())
        );
        assert_eq!(
            normalize_inlet_name("Phase offset"),
            Some("phase_offset".to_string())
        );
        // Type prefix stripped, then too long → None
        assert_eq!(
            normalize_inlet_name("Input Gain (Filter coefficient a0)"),
            None
        );
        assert_eq!(normalize_inlet_name(""), None);
        assert_eq!(normalize_inlet_name("  "), None);
        assert_eq!(normalize_inlet_name("123"), None); // leading digits only
        assert_eq!(normalize_inlet_name("Q"), Some("q".to_string()));
        // Type prefix stripping
        assert_eq!(
            normalize_inlet_name("(signal) Input"),
            Some("input".to_string())
        );
        assert_eq!(
            normalize_inlet_name("(signal/float) Cutoff Frequency"),
            Some("cutoff_frequency".to_string())
        );
        assert_eq!(
            normalize_inlet_name("(Signal/Float) This * Right Inlet"),
            Some("this_right_inlet".to_string())
        );
        // Phase with range
        assert_eq!(
            normalize_inlet_name("Phase (0-1)"),
            Some("phase_01".to_string())
        );
    }

    #[test]
    fn subpatcher_named_args_from_in_decls() {
        // Build a maxpat with an embedded subpatcher that has named inlets
        let json = r#"{
            "patcher": {
                "boxes": [
                    { "box": { "id": "obj-1", "maxclass": "inlet", "numinlets": 0, "numoutlets": 1 } },
                    { "box": { "id": "obj-2", "maxclass": "inlet", "numinlets": 0, "numoutlets": 1 } },
                    {
                        "box": {
                            "id": "obj-sub", "maxclass": "newobj", "text": "p mysynth",
                            "numinlets": 2, "numoutlets": 1,
                            "patcher": {
                                "boxes": [
                                    { "box": { "id": "sub-1", "maxclass": "inlet", "comment": "frequency", "numinlets": 0, "numoutlets": 1, "patching_rect": [50, 10, 25, 25] } },
                                    { "box": { "id": "sub-2", "maxclass": "inlet", "comment": "amplitude", "numinlets": 0, "numoutlets": 1, "patching_rect": [150, 10, 25, 25] } },
                                    { "box": { "id": "sub-3", "maxclass": "newobj", "text": "cycle~", "numinlets": 2, "numoutlets": 1, "patching_rect": [50, 100, 50, 22] } },
                                    { "box": { "id": "sub-4", "maxclass": "outlet~", "numinlets": 1, "numoutlets": 0, "patching_rect": [50, 200, 25, 25] } }
                                ],
                                "lines": [
                                    { "patchline": { "source": ["sub-1", 0], "destination": ["sub-3", 0] } },
                                    { "patchline": { "source": ["sub-3", 0], "destination": ["sub-4", 0] } }
                                ]
                            }
                        }
                    },
                    { "box": { "id": "obj-out", "maxclass": "outlet~", "numinlets": 1, "numoutlets": 0 } }
                ],
                "lines": [
                    { "patchline": { "source": ["obj-1", 0], "destination": ["obj-sub", 0] } },
                    { "patchline": { "source": ["obj-2", 0], "destination": ["obj-sub", 1] } },
                    { "patchline": { "source": ["obj-sub", 0], "destination": ["obj-out", 0] } }
                ]
            }
        }"#;

        let maxpat = parse_maxpat(json).unwrap();
        let (main_patch, subpatchers) = analyze_recursive(&maxpat, "test", None).unwrap();

        // Verify subpatcher was extracted with named inlets
        assert_eq!(subpatchers.len(), 1);
        let sub = &subpatchers[0].1;
        assert_eq!(sub.in_decls.len(), 2);
        assert_eq!(sub.in_decls[0].name, "frequency");
        assert_eq!(sub.in_decls[1].name, "amplitude");

        // Verify parent patch uses named args for the subpatcher call
        let sub_wire = main_patch
            .wires
            .iter()
            .find(|w| w.expr.contains("mysynth"))
            .unwrap();
        assert!(
            sub_wire.expr.contains("frequency:"),
            "Expected named arg 'frequency:' in expr: {}",
            sub_wire.expr
        );
        assert!(
            sub_wire.expr.contains("amplitude:"),
            "Expected named arg 'amplitude:' in expr: {}",
            sub_wire.expr
        );
    }

    #[test]
    fn subpatcher_named_args_skipped_for_port_n() {
        // Subpatcher with default port_N names should NOT get named args
        let json = r#"{
            "patcher": {
                "boxes": [
                    { "box": { "id": "obj-1", "maxclass": "inlet", "numinlets": 0, "numoutlets": 1 } },
                    {
                        "box": {
                            "id": "obj-sub", "maxclass": "newobj", "text": "p mysub",
                            "numinlets": 1, "numoutlets": 1,
                            "patcher": {
                                "boxes": [
                                    { "box": { "id": "sub-1", "maxclass": "inlet", "numinlets": 0, "numoutlets": 1, "patching_rect": [50, 10, 25, 25] } },
                                    { "box": { "id": "sub-2", "maxclass": "outlet", "numinlets": 1, "numoutlets": 0, "patching_rect": [50, 100, 25, 25] } }
                                ],
                                "lines": [
                                    { "patchline": { "source": ["sub-1", 0], "destination": ["sub-2", 0] } }
                                ]
                            }
                        }
                    },
                    { "box": { "id": "obj-out", "maxclass": "outlet", "numinlets": 1, "numoutlets": 0 } }
                ],
                "lines": [
                    { "patchline": { "source": ["obj-1", 0], "destination": ["obj-sub", 0] } },
                    { "patchline": { "source": ["obj-sub", 0], "destination": ["obj-out", 0] } }
                ]
            }
        }"#;

        let maxpat = parse_maxpat(json).unwrap();
        let (main_patch, _) = analyze_recursive(&maxpat, "test", None).unwrap();

        // port_0 should NOT be used as a named arg
        let sub_wire = main_patch
            .wires
            .iter()
            .find(|w| w.expr.contains("mysub"))
            .unwrap();
        assert!(
            !sub_wire.expr.contains(":"),
            "port_N names should be skipped: {}",
            sub_wire.expr
        );
    }

    #[test]
    fn annotate_subpatcher_named_args_basic() {
        let mut args = vec!["osc".to_string(), "1.5".to_string()];
        let names = vec!["carrier_freq".to_string(), "harmonicity".to_string()];
        annotate_subpatcher_named_args(&mut args, &names, 2);
        assert_eq!(args[0], "carrier_freq: osc");
        assert_eq!(args[1], "harmonicity: 1.5");
    }

    #[test]
    fn annotate_subpatcher_named_args_skips_port_n() {
        let mut args = vec!["osc".to_string()];
        let names = vec!["port_0".to_string()];
        annotate_subpatcher_named_args(&mut args, &names, 1);
        assert_eq!(args[0], "osc"); // Unchanged
    }
}
