/// .maxpat JSON generation
///
/// Generate a `.maxpat` JSON string that Max/MSP can load from a `PatchGraph`.
/// Conforms to the schema defined in experiment E01.
use std::collections::HashMap;

use flutmax_sema::graph::{PatchGraph, PatchNode};
use serde_json::{json, Map, Value};

use crate::layout::sugiyama_layout;

/// UI layout and decorative attribute data from .uiflutmax sidecar file.
pub struct UiData {
    /// Patcher-level settings (window rect, etc.)
    pub patcher: HashMap<String, Value>,
    /// Per-wire UI data: wire_name -> { "rect": [...], "background": 0, ... }
    pub entries: HashMap<String, Value>,
    /// Comment boxes with text and position for .maxpat reconstruction.
    pub comments: Vec<Value>,
    /// Visual-only panel boxes for .maxpat reconstruction.
    pub panels: Vec<Value>,
    /// Visual-only image boxes (fpic) for .maxpat reconstruction.
    pub images: Vec<Value>,
}

impl UiData {
    /// Parse a .uiflutmax JSON string into UiData.
    /// Returns None if the JSON is invalid or not an object.
    pub fn from_json(json_str: &str) -> Option<Self> {
        let root: Value = serde_json::from_str(json_str).ok()?;
        let obj = root.as_object()?;

        let mut patcher = HashMap::new();
        let mut entries = HashMap::new();

        let comments = obj
            .get("_comments")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let panels = obj
            .get("_panels")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let images = obj
            .get("_images")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        for (key, value) in obj {
            if key == "_patcher" {
                if let Some(inner) = value.as_object() {
                    for (k, v) in inner {
                        patcher.insert(k.clone(), v.clone());
                    }
                }
            } else if key == "_comments" || key == "_panels" || key == "_images" {
                // Already parsed above
            } else {
                entries.insert(key.clone(), value.clone());
            }
        }

        Some(UiData {
            patcher,
            entries,
            comments,
            panels,
            images,
        })
    }
}

/// Code generation error
#[derive(Debug)]
pub enum CodegenError {
    /// JSON serialization failed
    Serialization(String),
}

impl std::fmt::Display for CodegenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodegenError::Serialization(msg) => write!(f, "codegen error: {}", msg),
        }
    }
}

impl std::error::Error for CodegenError {}

// ─── Layout constants ───

const LAYOUT_X: f64 = 100.0;
const LAYOUT_Y_START: f64 = 50.0;
const LAYOUT_Y_STEP: f64 = 70.0;

const BOX_WIDTH_INLET_OUTLET: f64 = 30.0;
const BOX_HEIGHT_INLET_OUTLET: f64 = 30.0;
const BOX_WIDTH_NEWOBJ: f64 = 80.0;
const BOX_HEIGHT_NEWOBJ: f64 = 22.0;
const BOX_WIDTH_EZDAC: f64 = 45.0;
const BOX_HEIGHT_EZDAC: f64 = 45.0;

/// Options for .maxpat generation.
pub struct GenerateOptions {
    /// Patcher classnamespace: "box" (standard Max) or "rnbo" (RNBO subset).
    pub classnamespace: String,
}

impl Default for GenerateOptions {
    fn default() -> Self {
        Self {
            classnamespace: "box".to_string(),
        }
    }
}

/// Generate a .maxpat JSON string from a PatchGraph.
pub fn generate(graph: &PatchGraph) -> Result<String, CodegenError> {
    generate_with_options(graph, &GenerateOptions::default())
}

/// Generate a .maxpat JSON string from a PatchGraph (with options).
pub fn generate_with_options(
    graph: &PatchGraph,
    opts: &GenerateOptions,
) -> Result<String, CodegenError> {
    generate_with_ui(graph, opts, None)
}

/// Generate a .maxpat JSON string from a PatchGraph (with UiData).
///
/// When `ui_data` is provided, position and decoration attributes loaded from .uiflutmax
/// are reflected in the generated .maxpat. When None, automatic layout is used.
pub fn generate_with_ui(
    graph: &PatchGraph,
    opts: &GenerateOptions,
    ui_data: Option<&UiData>,
) -> Result<String, CodegenError> {
    let patcher = build_patcher(graph, opts, ui_data)?;
    let root = json!({ "patcher": patcher });
    serde_json::to_string_pretty(&root).map_err(|e| CodegenError::Serialization(e.to_string()))
}

/// Build the patcher object.
fn build_patcher(
    graph: &PatchGraph,
    opts: &GenerateOptions,
    ui_data: Option<&UiData>,
) -> Result<Value, CodegenError> {
    let is_rnbo = opts.classnamespace == "rnbo";
    let is_gen = opts.classnamespace == "dsp.gen";
    let needs_port_indices = is_rnbo || is_gen;
    let ordered_nodes = topological_order(graph);

    // RNBO/gen~ mode: pre-calculate inlet/outlet port indices
    let inlet_indices: HashMap<String, usize> = if needs_port_indices {
        let mut control_idx = 0usize;
        let mut signal_idx = 0usize;
        let mut map = HashMap::new();
        for node in &ordered_nodes {
            match node.object_name.as_str() {
                "inlet" => {
                    map.insert(node.id.clone(), control_idx);
                    control_idx += 1;
                }
                "inlet~" => {
                    map.insert(node.id.clone(), signal_idx);
                    signal_idx += 1;
                }
                _ => {}
            }
        }
        map
    } else {
        HashMap::new()
    };

    let outlet_indices: HashMap<String, usize> = if needs_port_indices {
        let mut control_idx = 0usize;
        let mut signal_idx = 0usize;
        let mut map = HashMap::new();
        for node in &ordered_nodes {
            match node.object_name.as_str() {
                "outlet" => {
                    map.insert(node.id.clone(), control_idx);
                    control_idx += 1;
                }
                "outlet~" => {
                    map.insert(node.id.clone(), signal_idx);
                    signal_idx += 1;
                }
                _ => {}
            }
        }
        map
    } else {
        HashMap::new()
    };

    // Node ID -> sequential ID mapping ("obj-1", "obj-2", ...)
    let mut id_map: HashMap<String, String> = HashMap::new();
    for (i, node) in ordered_nodes.iter().enumerate() {
        id_map.insert(node.id.clone(), format!("obj-{}", i + 1));
    }

    // Sugiyama auto-layout
    let layout = sugiyama_layout(graph);

    // Box generation
    let classnamespace = opts.classnamespace.as_str();
    let mut boxes: Vec<Value> = ordered_nodes
        .iter()
        .enumerate()
        .map(|(i, node)| {
            let mapped_id = format!("obj-{}", i + 1);
            let (x, y) = layout
                .positions
                .get(&node.id)
                .copied()
                .unwrap_or((LAYOUT_X, LAYOUT_Y_START + (i as f64) * LAYOUT_Y_STEP));
            let serial = i + 1; // rnbo_serial: 1-based monotonically increasing
            let port_index = inlet_indices
                .get(&node.id)
                .or_else(|| outlet_indices.get(&node.id))
                .copied();
            build_box(
                node,
                &BoxContext {
                    id: &mapped_id,
                    x,
                    y,
                    classnamespace,
                    serial,
                    port_index,
                    ui_data,
                },
            )
        })
        .collect();

    // Append visual-only boxes from UI data (comments, panels, images)
    if let Some(ui) = ui_data {
        let mut visual_counter = ordered_nodes.len() + 1;

        // Restore comment boxes
        for comment in &ui.comments {
            let rect = comment
                .get("rect")
                .cloned()
                .unwrap_or(json!([50, 50, 200, 20]));
            let text = comment.get("text").and_then(|t| t.as_str()).unwrap_or("");
            let id = format!("obj-{}", visual_counter);
            visual_counter += 1;
            boxes.push(json!({
                "box": {
                    "id": id,
                    "maxclass": "comment",
                    "text": text,
                    "numinlets": 1,
                    "numoutlets": 0,
                    "outlettype": [],
                    "patching_rect": rect,
                }
            }));
        }

        // Restore panel boxes
        for panel in &ui.panels {
            let rect = panel
                .get("rect")
                .cloned()
                .unwrap_or(json!([50, 50, 200, 200]));
            let id = format!("obj-{}", visual_counter);
            visual_counter += 1;
            let mut box_obj = serde_json::Map::new();
            box_obj.insert("id".into(), json!(id));
            box_obj.insert("maxclass".into(), json!("panel"));
            box_obj.insert("numinlets".into(), json!(1));
            box_obj.insert("numoutlets".into(), json!(0));
            box_obj.insert("outlettype".into(), json!([]));
            box_obj.insert("patching_rect".into(), rect);
            // Restore panel attributes
            if let Some(obj) = panel.as_object() {
                for (k, v) in obj {
                    if k != "rect" {
                        box_obj.insert(k.clone(), v.clone());
                    }
                }
            }
            boxes.push(json!({ "box": Value::Object(box_obj) }));
        }

        // Restore image boxes (fpic)
        for image in &ui.images {
            let rect = image
                .get("rect")
                .cloned()
                .unwrap_or(json!([50, 50, 200, 200]));
            let pic = image.get("pic").and_then(|p| p.as_str()).unwrap_or("");
            let id = format!("obj-{}", visual_counter);
            visual_counter += 1;
            let mut box_obj = serde_json::Map::new();
            box_obj.insert("id".into(), json!(id));
            box_obj.insert("maxclass".into(), json!("fpic"));
            box_obj.insert("numinlets".into(), json!(1));
            box_obj.insert("numoutlets".into(), json!(1));
            box_obj.insert("outlettype".into(), json!(["jit_matrix"]));
            box_obj.insert("patching_rect".into(), rect);
            if !pic.is_empty() {
                box_obj.insert("pic".into(), json!(pic));
            }
            boxes.push(json!({ "box": Value::Object(box_obj) }));
        }

        // Suppress unused variable warning
        let _ = visual_counter;
    }

    // Line generation
    let lines: Vec<Value> = graph
        .edges
        .iter()
        .map(|edge| {
            let source_id = id_map
                .get(&edge.source_id)
                .cloned()
                .unwrap_or_else(|| edge.source_id.clone());
            let dest_id = id_map
                .get(&edge.dest_id)
                .cloned()
                .unwrap_or_else(|| edge.dest_id.clone());
            let mut patchline = serde_json::Map::new();
            patchline.insert("source".into(), json!([source_id, edge.source_outlet]));
            patchline.insert("destination".into(), json!([dest_id, edge.dest_inlet]));
            if let Some(order) = edge.order {
                patchline.insert("order".into(), json!(order));
            }
            json!({ "patchline": Value::Object(patchline) })
        })
        .collect();

    // Combine fixed template + dynamic fields
    let mut patcher = Map::new();
    patcher.insert("fileversion".into(), json!(1));
    patcher.insert(
        "appversion".into(),
        json!({
            "major": 8,
            "minor": 6,
            "revision": 0,
            "architecture": "x64",
            "modernui": 1
        }),
    );
    patcher.insert("classnamespace".into(), json!(&opts.classnamespace));
    // Use patcher rect from UI data if available, otherwise derive from Sugiyama layout
    let patcher_rect = ui_data
        .and_then(|ui| ui.patcher.get("rect"))
        .cloned()
        .unwrap_or_else(|| {
            json!([
                100.0,
                100.0,
                layout.patcher_size.0.max(640.0),
                layout.patcher_size.1.max(480.0)
            ])
        });
    patcher.insert("rect".into(), patcher_rect);
    patcher.insert("bglocked".into(), json!(0));
    patcher.insert("openinpresentation".into(), json!(0));
    patcher.insert("default_fontsize".into(), json!(12.0));
    patcher.insert("default_fontface".into(), json!(0));
    patcher.insert("default_fontname".into(), json!("Arial"));
    patcher.insert("gridonopen".into(), json!(1));
    patcher.insert("gridsize".into(), json!([15.0, 15.0]));
    patcher.insert("gridsnaponopen".into(), json!(1));
    patcher.insert("objectsnaponopen".into(), json!(1));
    patcher.insert("statusbarvisible".into(), json!(2));
    patcher.insert("toolbarvisible".into(), json!(1));
    patcher.insert("lefttoolbarpinned".into(), json!(0));
    patcher.insert("toptoolbarpinned".into(), json!(0));
    patcher.insert("righttoolbarpinned".into(), json!(0));
    patcher.insert("bottomtoolbarpinned".into(), json!(0));
    patcher.insert("toolbars_unpinned_last_save".into(), json!(0));
    patcher.insert("tallnewobj".into(), json!(0));
    patcher.insert("boxanimatetime".into(), json!(200));
    patcher.insert("enablehscroll".into(), json!(1));
    patcher.insert("enablevscroll".into(), json!(1));
    patcher.insert("devicewidth".into(), json!(0.0));
    patcher.insert("description".into(), json!(""));
    patcher.insert("digest".into(), json!(""));
    patcher.insert("tags".into(), json!(""));
    patcher.insert("style".into(), json!(""));
    patcher.insert("subpatcher_template".into(), json!(""));
    patcher.insert("assistshowspatchername".into(), json!(0));
    patcher.insert("boxes".into(), Value::Array(boxes));
    patcher.insert("lines".into(), Value::Array(lines));
    patcher.insert("dependency_cache".into(), json!([]));
    patcher.insert("autosave".into(), json!(0));

    Ok(Value::Object(patcher))
}

/// Layout and rendering context for generating a box.
struct BoxContext<'a> {
    id: &'a str,
    x: f64,
    y: f64,
    classnamespace: &'a str,
    serial: usize,
    port_index: Option<usize>,
    ui_data: Option<&'a UiData>,
}

/// Generate box JSON from a PatchNode.
fn build_box(node: &PatchNode, ctx: &BoxContext) -> Value {
    let is_rnbo = ctx.classnamespace == "rnbo";
    let is_gen = ctx.classnamespace == "dsp.gen";
    let (maxclass, width, height) = classify_maxclass(node, ctx.classnamespace);
    let outlettype = compute_outlettype(node, is_rnbo, is_gen);

    // RNBO mode: outlet/outport has numoutlets=0 (sink)
    // gen~ mode: `out N` has numoutlets=0 (sink)
    let effective_num_outlets =
        if (is_rnbo || is_gen) && matches!(node.object_name.as_str(), "outlet" | "outlet~") {
            0
        } else {
            node.num_outlets
        };

    let mut box_obj = Map::new();
    box_obj.insert("id".into(), json!(ctx.id));
    box_obj.insert("maxclass".into(), json!(maxclass));
    box_obj.insert("numinlets".into(), json!(node.num_inlets));
    box_obj.insert("numoutlets".into(), json!(effective_num_outlets));

    if !outlettype.is_empty() {
        box_obj.insert("outlettype".into(), json!(outlettype));
    }

    box_obj.insert("patching_rect".into(), json!([ctx.x, ctx.y, width, height]));

    // text field: for newobj and message
    if maxclass == "newobj" {
        let text = if is_rnbo {
            // RNBO mode: inlet/outlet → inport/outport or in~/out~ text
            match node.object_name.as_str() {
                "inlet" => {
                    let name = node
                        .varname
                        .clone()
                        .unwrap_or_else(|| format!("port_{}", ctx.port_index.unwrap_or(0)));
                    format!("inport {}", name)
                }
                "inlet~" => {
                    let idx = ctx.port_index.unwrap_or(0) + 1; // RNBO uses 1-based
                    format!("in~ {}", idx)
                }
                "outlet" => {
                    let name = node
                        .varname
                        .clone()
                        .unwrap_or_else(|| format!("port_{}", ctx.port_index.unwrap_or(0)));
                    format!("outport {}", name)
                }
                "outlet~" => {
                    let idx = ctx.port_index.unwrap_or(0) + 1; // RNBO uses 1-based
                    format!("out~ {}", idx)
                }
                _ => {
                    let mut t = build_object_text(node);
                    if !node.attrs.is_empty() {
                        let attr_str: String = node
                            .attrs
                            .iter()
                            .map(|(k, v)| format!("@{} {}", k, v))
                            .collect::<Vec<_>>()
                            .join(" ");
                        t = format!("{} {}", t, attr_str);
                    }
                    t
                }
            }
        } else if is_gen {
            // gen~ mode: inlet/outlet → `in N` / `out N` (1-based)
            match node.object_name.as_str() {
                "inlet" | "inlet~" => {
                    let idx = ctx.port_index.unwrap_or(0) + 1; // gen~ uses 1-based
                    format!("in {}", idx)
                }
                "outlet" | "outlet~" => {
                    let idx = ctx.port_index.unwrap_or(0) + 1; // gen~ uses 1-based
                    format!("out {}", idx)
                }
                _ => {
                    let mut t = build_object_text(node);
                    if !node.attrs.is_empty() {
                        let attr_str: String = node
                            .attrs
                            .iter()
                            .map(|(k, v)| format!("@{} {}", k, v))
                            .collect::<Vec<_>>()
                            .join(" ");
                        t = format!("{} {}", t, attr_str);
                    }
                    t
                }
            }
        } else {
            let mut t = build_object_text(node);
            // newobj: append .attr() attributes as @key value to text
            if !node.attrs.is_empty() {
                let attr_str: String = node
                    .attrs
                    .iter()
                    .map(|(k, v)| format!("@{} {}", k, v))
                    .collect::<Vec<_>>()
                    .join(" ");
                t = format!("{} {}", t, attr_str);
            }
            t
        };
        box_obj.insert("text".into(), json!(text));
    } else if maxclass == "message" {
        // message box: text uses the content (args[0]) as-is
        let text = if node.args.is_empty() {
            String::new()
        } else {
            node.args.join(" ")
        };
        box_obj.insert("text".into(), json!(text));
    }

    // varname: output flutmax wire name as Max varname attribute
    if let Some(ref vn) = node.varname {
        box_obj.insert("varname".into(), json!(vn));
    }

    // UI objects (non-newobj): output .attr() attributes as top-level fields in box JSON
    if maxclass != "newobj" && !node.attrs.is_empty() {
        for (key, value) in &node.attrs {
            // Output as number if parseable, otherwise as string
            if let Ok(f) = value.parse::<f64>() {
                box_obj.insert(key.clone(), json!(f));
            } else {
                box_obj.insert(key.clone(), json!(value));
            }
        }
    }

    // Codebox: emit code field and special attributes
    if matches!(maxclass, "v8.codebox" | "codebox") {
        if let Some(ref code) = node.code {
            box_obj.insert("code".into(), json!(code));
        }
        if maxclass == "v8.codebox" {
            box_obj.insert("filename".into(), json!("none"));
            // v8.codebox uses empty text (code is in the code field)
            if !box_obj.contains_key("text") {
                box_obj.insert("text".into(), json!(""));
            }
        }
    }

    // .uiflutmax UI data: override position and add decorative attributes
    if let Some(ui_entry) = ctx
        .ui_data
        .and_then(|ui| node.varname.as_ref().and_then(|vn| ui.entries.get(vn)))
    {
        // Override position from UI data
        if let Some(rect) = ui_entry.get("rect") {
            box_obj.insert("patching_rect".into(), rect.clone());
        }
        // Add decorative attributes (everything except "rect")
        if let Some(obj) = ui_entry.as_object() {
            for (k, v) in obj {
                if k != "rect" {
                    box_obj.insert(k.clone(), v.clone());
                }
            }
        }
    }

    // RNBO mode: add rnbo_serial and rnbo_uniqueid
    if is_rnbo {
        box_obj.insert("rnbo_serial".into(), json!(ctx.serial));
        box_obj.insert(
            "rnbo_uniqueid".into(),
            json!(format!(
                "{}_{}",
                node.object_name.replace('~', "_tilde"),
                ctx.id
            )),
        );
    }

    json!({ "box": Value::Object(box_obj) })
}

/// Determine maxclass from PatchNode object_name.
/// Returns: (maxclass, width, height)
fn classify_maxclass(node: &PatchNode, classnamespace: &str) -> (&'static str, f64, f64) {
    let is_rnbo = classnamespace == "rnbo";
    let is_gen = classnamespace == "dsp.gen";
    match node.object_name.as_str() {
        "inlet" | "inlet~" if is_rnbo || is_gen => ("newobj", BOX_WIDTH_NEWOBJ, BOX_HEIGHT_NEWOBJ),
        "outlet" | "outlet~" if is_rnbo || is_gen => {
            ("newobj", BOX_WIDTH_NEWOBJ, BOX_HEIGHT_NEWOBJ)
        }
        "inlet" => ("inlet", BOX_WIDTH_INLET_OUTLET, BOX_HEIGHT_INLET_OUTLET),
        "inlet~" => ("inlet", BOX_WIDTH_INLET_OUTLET, BOX_HEIGHT_INLET_OUTLET),
        "outlet" => ("outlet", BOX_WIDTH_INLET_OUTLET, BOX_HEIGHT_INLET_OUTLET),
        "outlet~" => ("outlet", BOX_WIDTH_INLET_OUTLET, BOX_HEIGHT_INLET_OUTLET),
        "ezdac~" => ("ezdac~", BOX_WIDTH_EZDAC, BOX_HEIGHT_EZDAC),
        "message" => ("message", 50.0, 22.0),
        "button" => ("button", 50.0, 50.0),
        "flonum" => ("flonum", 80.0, 22.0),
        "number" => ("number", 50.0, 22.0),
        "toggle" => ("toggle", 20.0, 20.0),
        "umenu" => ("umenu", 100.0, 22.0),
        "panel" => ("panel", 100.0, 50.0),
        "jsui" => ("jsui", 64.0, 64.0),
        // Additional UI objects
        "textbutton" => ("textbutton", 100.0, 20.0),
        "live.text" => ("live.text", 44.0, 15.0),
        "live.dial" => ("live.dial", 47.0, 48.0),
        "live.toggle" => ("live.toggle", 15.0, 15.0),
        "live.menu" => ("live.menu", 100.0, 15.0),
        "live.numbox" => ("live.numbox", 44.0, 15.0),
        "live.tab" => ("live.tab", 100.0, 20.0),
        "live.comment" => ("live.comment", 100.0, 18.0),
        "slider" => ("slider", 20.0, 140.0),
        "dial" => ("dial", 40.0, 40.0),
        "multislider" => ("multislider", 120.0, 100.0),
        "kslider" => ("kslider", 168.0, 53.0),
        "tab" => ("tab", 200.0, 24.0),
        "rslider" => ("rslider", 100.0, 22.0),
        "filtergraph~" => ("filtergraph~", 256.0, 128.0),
        "spectroscope~" => ("spectroscope~", 300.0, 100.0),
        "scope~" => ("scope~", 130.0, 130.0),
        "meter~" => ("meter~", 13.0, 80.0),
        "gain~" => ("gain~", 22.0, 140.0),
        "ezadc~" => ("ezadc~", BOX_WIDTH_EZDAC, BOX_HEIGHT_EZDAC),
        "number~" => ("number~", 56.0, 22.0),
        "bpatcher" => ("bpatcher", 128.0, 128.0),
        "fpic" => ("fpic", 100.0, 100.0),
        "textedit" => ("textedit", 100.0, 22.0),
        "attrui" => ("attrui", 150.0, 22.0),
        "nslider" => ("nslider", 50.0, 120.0),
        "preset" => ("preset", 100.0, 40.0),
        // Codebox objects
        "v8.codebox" => ("v8.codebox", 200.0, 100.0),
        "codebox" => ("codebox", 200.0, 100.0),
        _ => ("newobj", BOX_WIDTH_NEWOBJ, BOX_HEIGHT_NEWOBJ),
    }
}

/// Compute the outlettype array for an object.
fn compute_outlettype(node: &PatchNode, is_rnbo: bool, is_gen: bool) -> Vec<&'static str> {
    // RNBO mode: outlet/outport is a sink, so no outlettype
    // gen~ mode: `out N` is a sink, so no outlettype
    if (is_rnbo || is_gen) && matches!(node.object_name.as_str(), "outlet" | "outlet~") {
        return vec![];
    }

    if node.num_outlets == 0 {
        return vec![];
    }

    match node.object_name.as_str() {
        // RNBO mode: inport (control inlet) → outlettype = [""]
        "inlet" if is_rnbo => vec![""],
        // RNBO mode: in~ (signal inlet) → outlettype = ["signal"]
        "inlet~" if is_rnbo => vec!["signal"],

        // gen~ mode: `in N` (all I/O is signal) → outlettype = [""]
        "inlet" | "inlet~" if is_gen => vec![""],

        // inlet/inlet~ has one outlettype
        "inlet" => vec![""],
        "inlet~" => vec!["signal"],

        // message box
        "message" => vec![""],

        // UI objects
        "button" => vec!["bang"],
        "toggle" => vec!["int"],
        "umenu" => vec!["int", "", ""],
        "flonum" => vec!["", "bang"],
        "number" => vec!["", "bang"],
        "textbutton" => vec!["", "", "int"],
        "live.text" => vec!["", ""],
        "live.dial" => vec!["", ""],
        "live.toggle" => vec![""],
        "live.menu" => vec!["", "", ""],
        "live.numbox" => vec!["", ""],
        "live.tab" => vec!["", "", ""],
        "live.comment" => vec![],
        "slider" => vec![""],
        "dial" => vec![""],
        "multislider" => vec!["", ""],
        "kslider" => vec!["", ""],
        "tab" => vec!["", "", ""],
        "rslider" => vec!["", ""],
        "bpatcher" => {
            // bpatcher outlet count depends on the patch. Use node.num_outlets
            vec![""; node.num_outlets as usize]
        }

        // Signal objects: all outlets are "signal"
        name if name.ends_with('~') => {
            let mut types = vec!["signal"];
            // For objects like line~ with 2+ outlets: the last may be "bang"
            if name == "line~" && node.num_outlets >= 2 {
                types = vec!["signal", "bang"];
            }
            // Keep as-is if already sufficient, otherwise pad with signal
            while types.len() < node.num_outlets as usize {
                types.push("signal");
            }
            types.truncate(node.num_outlets as usize);
            types
        }

        // Codebox objects
        "v8.codebox" | "codebox" => {
            vec![""; node.num_outlets as usize]
        }

        // Control objects
        "trigger" | "t" => {
            // trigger outlet types depend on arg types; simplified to "" padding
            vec![""; node.num_outlets as usize]
        }

        _ => {
            // Use "signal" when is_signal is set (e.g., for Abstractions)
            if node.is_signal {
                vec!["signal"; node.num_outlets as usize]
            } else {
                // Default: set all outlets to "" (generic message)
                vec![""; node.num_outlets as usize]
            }
        }
    }
}

/// Generate object text from a PatchNode.
/// e.g., object_name="cycle~", args=["440"] -> "cycle~ 440"
fn build_object_text(node: &PatchNode) -> String {
    if node.args.is_empty() {
        node.object_name.clone()
    } else {
        format!("{} {}", node.object_name, node.args.join(" "))
    }
}

/// Sort nodes in topological order.
/// inlet -> processing objects -> outlet order.
/// Not a full topological sort; a simplified classification-based reordering.
fn topological_order(graph: &PatchGraph) -> Vec<&PatchNode> {
    let mut inlets: Vec<&PatchNode> = Vec::new();
    let mut outlets: Vec<&PatchNode> = Vec::new();
    let mut others: Vec<&PatchNode> = Vec::new();

    for node in &graph.nodes {
        match node.object_name.as_str() {
            "inlet" | "inlet~" => inlets.push(node),
            "outlet" | "outlet~" => outlets.push(node),
            _ => others.push(node),
        }
    }

    // Maintain original order within each category
    let mut result = Vec::with_capacity(graph.nodes.len());
    result.extend(inlets);
    result.extend(others);
    result.extend(outlets);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use flutmax_sema::graph::{NodePurity, PatchEdge, PatchNode};

    /// Minimal graph: cycle~ 440 -> ezdac~
    fn make_minimal_graph() -> PatchGraph {
        let mut g = PatchGraph::new();
        g.add_node(PatchNode {
            id: "osc".into(),
            object_name: "cycle~".into(),
            args: vec!["440".into()],
            num_inlets: 2,
            num_outlets: 1,
            is_signal: true,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        g.add_node(PatchNode {
            id: "dac".into(),
            object_name: "ezdac~".into(),
            args: vec![],
            num_inlets: 2,
            num_outlets: 0,
            is_signal: true,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        g.add_edge(PatchEdge {
            source_id: "osc".into(),
            source_outlet: 0,
            dest_id: "dac".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });
        g.add_edge(PatchEdge {
            source_id: "osc".into(),
            source_outlet: 0,
            dest_id: "dac".into(),
            dest_inlet: 1,
            is_feedback: false,
            order: None,
        });
        g
    }

    /// Graph: inlet -> cycle~ -> *~ -> outlet~
    fn make_l2_graph() -> PatchGraph {
        let mut g = PatchGraph::new();
        g.add_node(PatchNode {
            id: "in_freq".into(),
            object_name: "inlet".into(),
            args: vec![],
            num_inlets: 0,
            num_outlets: 1,
            is_signal: false,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        g.add_node(PatchNode {
            id: "cycle".into(),
            object_name: "cycle~".into(),
            args: vec![],
            num_inlets: 2,
            num_outlets: 1,
            is_signal: true,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        g.add_node(PatchNode {
            id: "mul".into(),
            object_name: "*~".into(),
            args: vec!["0.5".into()],
            num_inlets: 2,
            num_outlets: 1,
            is_signal: true,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        g.add_node(PatchNode {
            id: "out_audio".into(),
            object_name: "outlet~".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 0,
            is_signal: true,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        g.add_edge(PatchEdge {
            source_id: "in_freq".into(),
            source_outlet: 0,
            dest_id: "cycle".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });
        g.add_edge(PatchEdge {
            source_id: "cycle".into(),
            source_outlet: 0,
            dest_id: "mul".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });
        g.add_edge(PatchEdge {
            source_id: "mul".into(),
            source_outlet: 0,
            dest_id: "out_audio".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });
        g
    }

    #[test]
    fn test_generate_valid_json() {
        let graph = make_minimal_graph();
        let json_str = generate(&graph).unwrap();

        // Must be parseable JSON
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        assert!(parsed.is_object());
        assert!(parsed.get("patcher").is_some());
    }

    #[test]
    fn test_patcher_fixed_fields() {
        let graph = make_minimal_graph();
        let json_str = generate(&graph).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        let patcher = parsed.get("patcher").unwrap();

        assert_eq!(patcher["fileversion"], 1);
        assert_eq!(patcher["appversion"]["major"], 8);
        assert_eq!(patcher["appversion"]["minor"], 6);
        assert_eq!(patcher["classnamespace"], "box");
        assert_eq!(patcher["default_fontname"], "Arial");
        assert_eq!(patcher["autosave"], 0);
    }

    #[test]
    fn test_boxes_count() {
        let graph = make_minimal_graph();
        let json_str = generate(&graph).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        let boxes = parsed["patcher"]["boxes"].as_array().unwrap();
        assert_eq!(boxes.len(), 2);
    }

    #[test]
    fn test_box_structure() {
        let graph = make_minimal_graph();
        let json_str = generate(&graph).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        let boxes = parsed["patcher"]["boxes"].as_array().unwrap();

        // cycle~ box
        let cycle_box = &boxes[0]["box"];
        assert_eq!(cycle_box["id"], "obj-1");
        assert_eq!(cycle_box["maxclass"], "newobj");
        assert_eq!(cycle_box["numinlets"], 2);
        assert_eq!(cycle_box["numoutlets"], 1);
        assert_eq!(cycle_box["text"], "cycle~ 440");
        let outlettype = cycle_box["outlettype"].as_array().unwrap();
        assert_eq!(outlettype.len(), 1);
        assert_eq!(outlettype[0], "signal");

        // ezdac~ box
        let dac_box = &boxes[1]["box"];
        assert_eq!(dac_box["id"], "obj-2");
        assert_eq!(dac_box["maxclass"], "ezdac~");
        assert_eq!(dac_box["numinlets"], 2);
        assert_eq!(dac_box["numoutlets"], 0);
        // ezdac~ has no outlettype (0 outlets)
        assert!(dac_box.get("outlettype").is_none());
    }

    #[test]
    fn test_lines_count() {
        let graph = make_minimal_graph();
        let json_str = generate(&graph).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        let lines = parsed["patcher"]["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_line_structure() {
        let graph = make_minimal_graph();
        let json_str = generate(&graph).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        let lines = parsed["patcher"]["lines"].as_array().unwrap();

        // Both have source "obj-1" (cycle~), dest "obj-2" (ezdac~)
        for line in lines {
            let patchline = &line["patchline"];
            let source = patchline["source"].as_array().unwrap();
            let dest = patchline["destination"].as_array().unwrap();

            assert_eq!(source[0], "obj-1");
            assert_eq!(source[1], 0);
            assert_eq!(dest[0], "obj-2");
            // dest_inlet is 0 or 1
            let inlet = dest[1].as_u64().unwrap();
            assert!(inlet == 0 || inlet == 1);
        }
    }

    #[test]
    fn test_patching_rect_layout() {
        let graph = make_minimal_graph();
        let json_str = generate(&graph).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        let boxes = parsed["patcher"]["boxes"].as_array().unwrap();

        let rect0 = boxes[0]["box"]["patching_rect"].as_array().unwrap();
        let rect1 = boxes[1]["box"]["patching_rect"].as_array().unwrap();

        // Sugiyama layout: linear chain → both at same x column
        let x0 = rect0[0].as_f64().unwrap();
        let x1 = rect1[0].as_f64().unwrap();
        assert_eq!(x0, x1, "linear chain nodes should share the same x");

        // Y increases sequentially (osc in layer 0, dac in layer 1)
        let y0 = rect0[1].as_f64().unwrap();
        let y1 = rect1[1].as_f64().unwrap();
        assert!(y1 > y0, "downstream node should have larger y");
    }

    #[test]
    fn test_l2_topological_order() {
        let graph = make_l2_graph();
        let json_str = generate(&graph).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        let boxes = parsed["patcher"]["boxes"].as_array().unwrap();

        // Topological order: inlet -> cycle~ -> *~ -> outlet~
        assert_eq!(boxes[0]["box"]["maxclass"], "inlet");
        assert_eq!(boxes[1]["box"]["text"], "cycle~");
        assert_eq!(boxes[2]["box"]["text"], "*~ 0.5");
        assert_eq!(boxes[3]["box"]["maxclass"], "outlet");
    }

    #[test]
    fn test_inlet_outlettype() {
        let graph = make_l2_graph();
        let json_str = generate(&graph).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        let boxes = parsed["patcher"]["boxes"].as_array().unwrap();

        // inlet outlettype is [""]
        let inlet_box = &boxes[0]["box"];
        assert_eq!(inlet_box["maxclass"], "inlet");
        let outlettype = inlet_box["outlettype"].as_array().unwrap();
        assert_eq!(outlettype.len(), 1);
        assert_eq!(outlettype[0], "");
    }

    #[test]
    fn test_outlet_tilde_maxclass() {
        let graph = make_l2_graph();
        let json_str = generate(&graph).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        let boxes = parsed["patcher"]["boxes"].as_array().unwrap();

        // outlet~ maxclass is "outlet~"
        let outlet_box = &boxes[3]["box"];
        assert_eq!(outlet_box["maxclass"], "outlet");
        assert_eq!(outlet_box["numinlets"], 1);
        assert_eq!(outlet_box["numoutlets"], 0);
    }

    #[test]
    fn test_empty_graph() {
        let graph = PatchGraph::new();
        let json_str = generate(&graph).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();

        let boxes = parsed["patcher"]["boxes"].as_array().unwrap();
        let lines = parsed["patcher"]["lines"].as_array().unwrap();
        assert_eq!(boxes.len(), 0);
        assert_eq!(lines.len(), 0);
    }

    #[test]
    fn test_dependency_cache_empty() {
        let graph = make_minimal_graph();
        let json_str = generate(&graph).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();

        let dep_cache = parsed["patcher"]["dependency_cache"].as_array().unwrap();
        assert_eq!(dep_cache.len(), 0);
    }

    #[test]
    fn test_build_object_text_no_args() {
        let node = PatchNode {
            id: "test".into(),
            object_name: "cycle~".into(),
            args: vec![],
            num_inlets: 2,
            num_outlets: 1,
            is_signal: true,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        };
        assert_eq!(build_object_text(&node), "cycle~");
    }

    #[test]
    fn test_build_object_text_with_args() {
        let node = PatchNode {
            id: "test".into(),
            object_name: "cycle~".into(),
            args: vec!["440".into()],
            num_inlets: 2,
            num_outlets: 1,
            is_signal: true,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        };
        assert_eq!(build_object_text(&node), "cycle~ 440");
    }

    #[test]
    fn test_build_object_text_multiple_args() {
        let node = PatchNode {
            id: "test".into(),
            object_name: "trigger".into(),
            args: vec!["b".into(), "b".into(), "b".into()],
            num_inlets: 1,
            num_outlets: 3,
            is_signal: false,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        };
        assert_eq!(build_object_text(&node), "trigger b b b");
    }

    #[test]
    fn test_classify_maxclass_inlet() {
        let node = PatchNode {
            id: "test".into(),
            object_name: "inlet".into(),
            args: vec![],
            num_inlets: 0,
            num_outlets: 1,
            is_signal: false,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        };
        let (maxclass, _, _) = classify_maxclass(&node, "box");
        assert_eq!(maxclass, "inlet");
    }

    #[test]
    fn test_classify_maxclass_inlet_tilde() {
        // inlet~ uses the same maxclass "inlet" internally in Max,
        // In actual Max patches, signal inlets also use "inlet" maxclass.
        let node = PatchNode {
            id: "test".into(),
            object_name: "inlet~".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 1,
            is_signal: true,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        };
        let (maxclass, _, _) = classify_maxclass(&node, "box");
        assert_eq!(maxclass, "inlet");
    }

    #[test]
    fn test_classify_maxclass_newobj() {
        let node = PatchNode {
            id: "test".into(),
            object_name: "cycle~".into(),
            args: vec!["440".into()],
            num_inlets: 2,
            num_outlets: 1,
            is_signal: true,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        };
        let (maxclass, _, _) = classify_maxclass(&node, "box");
        assert_eq!(maxclass, "newobj");
    }

    #[test]
    fn test_compute_outlettype_signal() {
        let node = PatchNode {
            id: "test".into(),
            object_name: "cycle~".into(),
            args: vec![],
            num_inlets: 2,
            num_outlets: 1,
            is_signal: true,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        };
        let types = compute_outlettype(&node, false, false);
        assert_eq!(types, vec!["signal"]);
    }

    #[test]
    fn test_compute_outlettype_no_outlets() {
        let node = PatchNode {
            id: "test".into(),
            object_name: "ezdac~".into(),
            args: vec![],
            num_inlets: 2,
            num_outlets: 0,
            is_signal: true,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        };
        let types = compute_outlettype(&node, false, false);
        assert!(types.is_empty());
    }

    #[test]
    fn test_roundtrip_l2() {
        // AST -> PatchGraph -> JSON -> parse -> structural verification
        use crate::builder::build_graph;
        use flutmax_ast::*;

        let prog = Program {
            in_decls: vec![InDecl {
                index: 0,
                name: "freq".to_string(),
                port_type: PortType::Float,
            }],
            out_decls: vec![OutDecl {
                index: 0,
                name: "audio".to_string(),
                port_type: PortType::Signal,
                value: None,
            }],
            wires: vec![
                Wire {
                    name: "osc".to_string(),
                    value: Expr::Call {
                        object: "cycle~".to_string(),
                        args: vec![CallArg::positional(Expr::Ref("freq".to_string()))],
                    },
                    span: None,
                    attrs: vec![],
                },
                Wire {
                    name: "amp".to_string(),
                    value: Expr::Call {
                        object: "mul~".to_string(),
                        args: vec![
                            CallArg::positional(Expr::Ref("osc".to_string())),
                            CallArg::positional(Expr::Lit(LitValue::Float(0.5))),
                        ],
                    },
                    span: None,
                    attrs: vec![],
                },
            ],
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![OutAssignment {
                index: 0,
                value: Expr::Ref("amp".to_string()),
                span: None,
            }],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();
        let json_str = generate(&graph).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();

        let patcher = &parsed["patcher"];
        let boxes = patcher["boxes"].as_array().unwrap();
        let lines = patcher["lines"].as_array().unwrap();

        // 4 nodes: inlet, cycle~, *~, outlet~
        assert_eq!(boxes.len(), 4);
        // 3 edges: inlet->cycle~, cycle~->*~, *~->outlet~
        assert_eq!(lines.len(), 3);

        // First box is inlet
        assert_eq!(boxes[0]["box"]["maxclass"], "inlet");

        // Last box is outlet~
        assert_eq!(boxes[3]["box"]["maxclass"], "outlet");
    }

    #[test]
    fn test_unique_ids() {
        let graph = make_l2_graph();
        let json_str = generate(&graph).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        let boxes = parsed["patcher"]["boxes"].as_array().unwrap();

        let ids: Vec<&str> = boxes
            .iter()
            .map(|b| b["box"]["id"].as_str().unwrap())
            .collect();

        // All IDs are unique
        let mut unique_ids = ids.clone();
        unique_ids.sort();
        unique_ids.dedup();
        assert_eq!(ids.len(), unique_ids.len());
    }

    #[test]
    fn test_message_box_output() {
        let mut g = PatchGraph::new();
        g.add_node(PatchNode {
            id: "msg1".into(),
            object_name: "message".into(),
            args: vec!["bang".into()],
            num_inlets: 2,
            num_outlets: 1,
            is_signal: false,
            varname: Some("click".into()),
            hot_inlets: vec![true, false],
            purity: NodePurity::Stateful,
            attrs: vec![],
            code: None,
        });

        let json_str = generate(&g).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        let boxes = parsed["patcher"]["boxes"].as_array().unwrap();

        let msg_box = &boxes[0]["box"];
        assert_eq!(msg_box["maxclass"], "message");
        assert_eq!(msg_box["text"], "bang");
        assert_eq!(msg_box["numinlets"], 2);
        assert_eq!(msg_box["numoutlets"], 1);
        assert_eq!(msg_box["varname"], "click");

        let outlettype = msg_box["outlettype"].as_array().unwrap();
        assert_eq!(outlettype.len(), 1);
        assert_eq!(outlettype[0], "");
    }

    #[test]
    fn test_fanout_patchline_has_order() {
        // cycle~ -> ezdac~ (inlet 0 and inlet 1) fanout
        let mut graph = make_minimal_graph();
        // Set order on fanout edges
        graph.edges[0].order = Some(0);
        graph.edges[1].order = Some(1);

        let json_str = generate(&graph).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        let lines = parsed["patcher"]["lines"].as_array().unwrap();

        // Both lines have an order field
        for (i, line) in lines.iter().enumerate() {
            let patchline = &line["patchline"];
            let order = patchline.get("order");
            assert!(order.is_some(), "patchline {} should have order field", i);
            assert_eq!(order.unwrap().as_u64().unwrap(), i as u64);
        }
    }

    #[test]
    fn test_non_fanout_patchline_no_order() {
        // Single-connection edges have no order
        let graph = make_l2_graph();
        let json_str = generate(&graph).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        let lines = parsed["patcher"]["lines"].as_array().unwrap();

        for (i, line) in lines.iter().enumerate() {
            let patchline = &line["patchline"];
            assert!(
                patchline.get("order").is_none(),
                "patchline {} should not have order field",
                i
            );
        }
    }

    // ================================================
    // .attr() chain codegen tests
    // ================================================

    #[test]
    fn test_newobj_attrs_in_text() {
        // newobj: attrs should be appended as @key value in text field
        let mut g = PatchGraph::new();
        g.add_node(PatchNode {
            id: "osc".into(),
            object_name: "cycle~".into(),
            args: vec!["440".into()],
            num_inlets: 2,
            num_outlets: 1,
            is_signal: true,
            varname: Some("osc".into()),
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![("phase".into(), "0.5".into())],
            code: None,
        });

        let json_str = generate(&g).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        let boxes = parsed["patcher"]["boxes"].as_array().unwrap();

        let text = boxes[0]["box"]["text"].as_str().unwrap();
        assert_eq!(text, "cycle~ 440 @phase 0.5");
    }

    #[test]
    fn test_newobj_multiple_attrs_in_text() {
        let mut g = PatchGraph::new();
        g.add_node(PatchNode {
            id: "osc".into(),
            object_name: "cycle~".into(),
            args: vec![],
            num_inlets: 2,
            num_outlets: 1,
            is_signal: true,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![
                ("frequency".into(), "440.".into()),
                ("phase".into(), "0.5".into()),
            ],
            code: None,
        });

        let json_str = generate(&g).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        let boxes = parsed["patcher"]["boxes"].as_array().unwrap();

        let text = boxes[0]["box"]["text"].as_str().unwrap();
        assert_eq!(text, "cycle~ @frequency 440. @phase 0.5");
    }

    #[test]
    fn test_ui_object_attrs_as_fields() {
        // UI object (flonum): attrs should be top-level box JSON fields
        let mut g = PatchGraph::new();
        g.add_node(PatchNode {
            id: "fnum".into(),
            object_name: "flonum".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 2,
            is_signal: false,
            varname: Some("w".into()),
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![
                ("minimum".into(), "0.".into()),
                ("maximum".into(), "100.".into()),
            ],
            code: None,
        });

        let json_str = generate(&g).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        let boxes = parsed["patcher"]["boxes"].as_array().unwrap();
        let box_obj = &boxes[0]["box"];

        assert_eq!(box_obj["maxclass"], "flonum");
        assert_eq!(box_obj["minimum"], 0.0);
        assert_eq!(box_obj["maximum"], 100.0);
        // UI objects should NOT have attrs in text (no text field for flonum)
        assert!(box_obj.get("text").is_none());
    }

    #[test]
    fn test_ui_object_string_attr() {
        let mut g = PatchGraph::new();
        g.add_node(PatchNode {
            id: "dial".into(),
            object_name: "live.dial".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 2,
            is_signal: false,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![("parameter_longname".into(), "Cutoff".into())],
            code: None,
        });

        let json_str = generate(&g).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        let boxes = parsed["patcher"]["boxes"].as_array().unwrap();
        let box_obj = &boxes[0]["box"];

        assert_eq!(box_obj["maxclass"], "live.dial");
        assert_eq!(box_obj["parameter_longname"], "Cutoff");
    }

    #[test]
    fn test_no_attrs_unchanged() {
        // When no attrs, output should be unchanged
        let mut g = PatchGraph::new();
        g.add_node(PatchNode {
            id: "osc".into(),
            object_name: "cycle~".into(),
            args: vec!["440".into()],
            num_inlets: 2,
            num_outlets: 1,
            is_signal: true,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });

        let json_str = generate(&g).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        let boxes = parsed["patcher"]["boxes"].as_array().unwrap();

        let text = boxes[0]["box"]["text"].as_str().unwrap();
        assert_eq!(text, "cycle~ 440");
    }

    // ================================================
    // RNBO codegen tests
    // ================================================

    /// Graph: inlet -> cycle~ -> outlet~ (for RNBO tests)
    fn make_rnbo_graph() -> PatchGraph {
        let mut g = PatchGraph::new();
        g.add_node(PatchNode {
            id: "in_freq".into(),
            object_name: "inlet".into(),
            args: vec![],
            num_inlets: 0,
            num_outlets: 1,
            is_signal: false,
            varname: Some("freq".into()),
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        g.add_node(PatchNode {
            id: "osc".into(),
            object_name: "cycle~".into(),
            args: vec!["440".into()],
            num_inlets: 2,
            num_outlets: 1,
            is_signal: true,
            varname: Some("osc".into()),
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        g.add_node(PatchNode {
            id: "out_audio".into(),
            object_name: "outlet~".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 0,
            is_signal: true,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        g.add_edge(PatchEdge {
            source_id: "in_freq".into(),
            source_outlet: 0,
            dest_id: "osc".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });
        g.add_edge(PatchEdge {
            source_id: "osc".into(),
            source_outlet: 0,
            dest_id: "out_audio".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });
        g
    }

    fn rnbo_opts() -> GenerateOptions {
        GenerateOptions {
            classnamespace: "rnbo".to_string(),
        }
    }

    #[test]
    fn test_generate_rnbo_classnamespace() {
        let graph = make_rnbo_graph();
        let json_str = generate_with_options(&graph, &rnbo_opts()).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        let patcher = parsed.get("patcher").unwrap();

        assert_eq!(patcher["classnamespace"], "rnbo");
    }

    #[test]
    fn test_rnbo_inport_outport() {
        let graph = make_rnbo_graph();
        let json_str = generate_with_options(&graph, &rnbo_opts()).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        let boxes = parsed["patcher"]["boxes"].as_array().unwrap();

        // inlet (control) → "inport freq" (uses varname)
        let inlet_box = &boxes[0]["box"];
        assert_eq!(inlet_box["maxclass"], "newobj");
        assert_eq!(inlet_box["text"], "inport freq");

        // outlet~ (signal) → "out~ 1" (1-based index)
        let outlet_box = &boxes[2]["box"];
        assert_eq!(outlet_box["maxclass"], "newobj");
        assert_eq!(outlet_box["text"], "out~ 1");
    }

    #[test]
    fn test_rnbo_signal_io() {
        // Graph with signal inlet~ and signal outlet~
        let mut g = PatchGraph::new();
        g.add_node(PatchNode {
            id: "in_sig".into(),
            object_name: "inlet~".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 1,
            is_signal: true,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        g.add_node(PatchNode {
            id: "out_sig".into(),
            object_name: "outlet~".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 0,
            is_signal: true,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        g.add_edge(PatchEdge {
            source_id: "in_sig".into(),
            source_outlet: 0,
            dest_id: "out_sig".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });

        let json_str = generate_with_options(&g, &rnbo_opts()).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        let boxes = parsed["patcher"]["boxes"].as_array().unwrap();

        // inlet~ → "in~ 1"
        let inlet_box = &boxes[0]["box"];
        assert_eq!(inlet_box["maxclass"], "newobj");
        assert_eq!(inlet_box["text"], "in~ 1");
        let outlettype = inlet_box["outlettype"].as_array().unwrap();
        assert_eq!(outlettype, &[json!("signal")]);

        // outlet~ → "out~ 1"
        let outlet_box = &boxes[1]["box"];
        assert_eq!(outlet_box["maxclass"], "newobj");
        assert_eq!(outlet_box["text"], "out~ 1");
        // outlet is sink: numoutlets = 0, no outlettype
        assert_eq!(outlet_box["numoutlets"], 0);
        assert!(outlet_box.get("outlettype").is_none());
    }

    #[test]
    fn test_rnbo_serial() {
        let graph = make_rnbo_graph();
        let json_str = generate_with_options(&graph, &rnbo_opts()).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        let boxes = parsed["patcher"]["boxes"].as_array().unwrap();

        // Each box should have rnbo_serial (1-based) and rnbo_uniqueid
        for (i, boxval) in boxes.iter().enumerate() {
            let b = &boxval["box"];
            let serial = b["rnbo_serial"].as_u64().unwrap();
            assert_eq!(serial, (i + 1) as u64, "rnbo_serial for box {}", i);

            let uniqueid = b["rnbo_uniqueid"].as_str().unwrap();
            assert!(!uniqueid.is_empty(), "rnbo_uniqueid should not be empty");
        }

        // Verify specific uniqueid format: "object_name_obj-N"
        let inlet_uid = boxes[0]["box"]["rnbo_uniqueid"].as_str().unwrap();
        assert_eq!(inlet_uid, "inlet_obj-1");

        let cycle_uid = boxes[1]["box"]["rnbo_uniqueid"].as_str().unwrap();
        assert_eq!(cycle_uid, "cycle_tilde_obj-2");

        let outlet_uid = boxes[2]["box"]["rnbo_uniqueid"].as_str().unwrap();
        assert_eq!(outlet_uid, "outlet_tilde_obj-3");
    }

    #[test]
    fn test_standard_unchanged() {
        // Verify generate() (default options) produces standard Max output
        let graph = make_rnbo_graph();
        let json_str = generate(&graph).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        let patcher = parsed.get("patcher").unwrap();

        // classnamespace should be "box"
        assert_eq!(patcher["classnamespace"], "box");

        let boxes = patcher["boxes"].as_array().unwrap();

        // inlet should use "inlet" maxclass, not "newobj"
        let inlet_box = &boxes[0]["box"];
        assert_eq!(inlet_box["maxclass"], "inlet");
        // No text field for standard inlet
        assert!(inlet_box.get("text").is_none());

        // outlet~ should use "outlet" maxclass
        let outlet_box = &boxes[2]["box"];
        assert_eq!(outlet_box["maxclass"], "outlet");

        // No rnbo_serial or rnbo_uniqueid in standard mode
        for boxval in boxes {
            let b = &boxval["box"];
            assert!(
                b.get("rnbo_serial").is_none(),
                "standard mode should not have rnbo_serial"
            );
            assert!(
                b.get("rnbo_uniqueid").is_none(),
                "standard mode should not have rnbo_uniqueid"
            );
        }
    }

    #[test]
    fn test_rnbo_control_outlet() {
        // Test control outlet → "outport name"
        let mut g = PatchGraph::new();
        g.add_node(PatchNode {
            id: "out_ctrl".into(),
            object_name: "outlet".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 1,
            is_signal: false,
            varname: Some("result".into()),
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });

        let json_str = generate_with_options(&g, &rnbo_opts()).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        let boxes = parsed["patcher"]["boxes"].as_array().unwrap();

        let outlet_box = &boxes[0]["box"];
        assert_eq!(outlet_box["maxclass"], "newobj");
        assert_eq!(outlet_box["text"], "outport result");
        // Control outlet in RNBO is sink: numoutlets = 0
        assert_eq!(outlet_box["numoutlets"], 0);
    }

    #[test]
    fn test_rnbo_inport_fallback_name() {
        // When no varname, use port_N as fallback
        let mut g = PatchGraph::new();
        g.add_node(PatchNode {
            id: "in_unnamed".into(),
            object_name: "inlet".into(),
            args: vec![],
            num_inlets: 0,
            num_outlets: 1,
            is_signal: false,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });

        let json_str = generate_with_options(&g, &rnbo_opts()).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        let boxes = parsed["patcher"]["boxes"].as_array().unwrap();

        let inlet_box = &boxes[0]["box"];
        assert_eq!(inlet_box["text"], "inport port_0");
    }

    // ================================================
    // Codebox tests
    // ================================================

    #[test]
    fn test_classify_maxclass_codebox() {
        // v8.codebox should return "v8.codebox" maxclass
        let node = PatchNode {
            id: "cb1".into(),
            object_name: "v8.codebox".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 1,
            is_signal: false,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        };
        let (maxclass, width, height) = classify_maxclass(&node, "box");
        assert_eq!(maxclass, "v8.codebox");
        assert_eq!(width, 200.0);
        assert_eq!(height, 100.0);

        // codebox (gen~) should return "codebox" maxclass
        let node2 = PatchNode {
            id: "cb2".into(),
            object_name: "codebox".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 1,
            is_signal: false,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        };
        let (maxclass2, _, _) = classify_maxclass(&node2, "box");
        assert_eq!(maxclass2, "codebox");
    }

    #[test]
    fn test_build_box_codebox_with_code() {
        // v8.codebox with code field should emit code, filename, and text in JSON
        let node = PatchNode {
            id: "cb1".into(),
            object_name: "v8.codebox".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 1,
            is_signal: false,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: Some("function bang() { outlet(0, 42); }".into()),
        };

        let box_json = build_box(
            &node,
            &BoxContext {
                id: "obj-1",
                x: 100.0,
                y: 50.0,
                classnamespace: "box",
                serial: 1,
                port_index: None,
                ui_data: None,
            },
        );
        let box_obj = &box_json["box"];

        assert_eq!(box_obj["maxclass"], "v8.codebox");
        assert_eq!(box_obj["code"], "function bang() { outlet(0, 42); }");
        assert_eq!(box_obj["filename"], "none");
        assert_eq!(box_obj["text"], "");
    }

    #[test]
    fn test_build_box_codebox_without_code() {
        // codebox (gen~) without code field should not emit code/filename
        let node = PatchNode {
            id: "cb1".into(),
            object_name: "codebox".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 1,
            is_signal: false,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        };

        let box_json = build_box(
            &node,
            &BoxContext {
                id: "obj-1",
                x: 100.0,
                y: 50.0,
                classnamespace: "box",
                serial: 1,
                port_index: None,
                ui_data: None,
            },
        );
        let box_obj = &box_json["box"];

        assert_eq!(box_obj["maxclass"], "codebox");
        assert!(box_obj.get("code").is_none());
        assert!(box_obj.get("filename").is_none());
    }

    #[test]
    fn test_standard_codegen_unchanged() {
        // Standard generate() still works with existing PatchGraph
        let mut g = PatchGraph::new();
        g.add_node(PatchNode {
            id: "osc".into(),
            object_name: "cycle~".into(),
            args: vec!["440".into()],
            num_inlets: 2,
            num_outlets: 1,
            is_signal: true,
            varname: Some("osc".into()),
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });

        let json_str = generate(&g).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        let boxes = parsed["patcher"]["boxes"].as_array().unwrap();
        assert_eq!(boxes.len(), 1);
        assert_eq!(boxes[0]["box"]["maxclass"], "newobj");
        assert_eq!(boxes[0]["box"]["text"], "cycle~ 440");
        // No code field for regular objects
        assert!(boxes[0]["box"].get("code").is_none());
    }

    #[test]
    fn test_gen_mode_classify_inlet_outlet() {
        // In gen~ mode, inlet/outlet should become "newobj"
        let inlet_node = PatchNode {
            id: "in".into(),
            object_name: "inlet~".into(),
            args: vec![],
            num_inlets: 0,
            num_outlets: 1,
            is_signal: true,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        };
        let (maxclass, _, _) = classify_maxclass(&inlet_node, "dsp.gen");
        assert_eq!(maxclass, "newobj");

        let outlet_node = PatchNode {
            id: "out".into(),
            object_name: "outlet~".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 0,
            is_signal: true,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        };
        let (maxclass, _, _) = classify_maxclass(&outlet_node, "dsp.gen");
        assert_eq!(maxclass, "newobj");
    }

    #[test]
    fn test_gen_mode_build_box_text() {
        // gen~ mode should generate "in N" / "out N" text
        let inlet_node = PatchNode {
            id: "in".into(),
            object_name: "inlet~".into(),
            args: vec![],
            num_inlets: 0,
            num_outlets: 1,
            is_signal: true,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        };
        let box_json = build_box(
            &inlet_node,
            &BoxContext {
                id: "obj-1",
                x: 100.0,
                y: 50.0,
                classnamespace: "dsp.gen",
                serial: 1,
                port_index: Some(0),
                ui_data: None,
            },
        );
        let box_obj = &box_json["box"];
        assert_eq!(box_obj["maxclass"], "newobj");
        assert_eq!(box_obj["text"], "in 1");
        // gen~ should NOT have rnbo_serial/rnbo_uniqueid
        assert!(box_obj.get("rnbo_serial").is_none());

        let outlet_node = PatchNode {
            id: "out".into(),
            object_name: "outlet~".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 0,
            is_signal: true,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        };
        let box_json = build_box(
            &outlet_node,
            &BoxContext {
                id: "obj-2",
                x: 100.0,
                y: 120.0,
                classnamespace: "dsp.gen",
                serial: 2,
                port_index: Some(0),
                ui_data: None,
            },
        );
        let box_obj = &box_json["box"];
        assert_eq!(box_obj["maxclass"], "newobj");
        assert_eq!(box_obj["text"], "out 1");
        assert_eq!(box_obj["numoutlets"], 0); // sink
    }

    #[test]
    fn test_gen_mode_codegen() {
        // Full gen~ codegen roundtrip
        let mut g = PatchGraph::new();
        g.add_node(PatchNode {
            id: "in1".into(),
            object_name: "inlet~".into(),
            args: vec![],
            num_inlets: 0,
            num_outlets: 1,
            is_signal: true,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        g.add_node(PatchNode {
            id: "mul".into(),
            object_name: "*".into(),
            args: vec!["0.5".into()],
            num_inlets: 2,
            num_outlets: 1,
            is_signal: false,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        g.add_node(PatchNode {
            id: "out1".into(),
            object_name: "outlet~".into(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 0,
            is_signal: true,
            varname: None,
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });
        g.add_edge(PatchEdge {
            source_id: "in1".into(),
            source_outlet: 0,
            dest_id: "mul".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });
        g.add_edge(PatchEdge {
            source_id: "mul".into(),
            source_outlet: 0,
            dest_id: "out1".into(),
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });

        let opts = GenerateOptions {
            classnamespace: "dsp.gen".to_string(),
        };
        let json_str = generate_with_options(&g, &opts).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed["patcher"]["classnamespace"], "dsp.gen");

        let boxes = parsed["patcher"]["boxes"].as_array().unwrap();
        assert_eq!(boxes.len(), 3);

        // First box: inlet~ → "in 1"
        assert_eq!(boxes[0]["box"]["maxclass"], "newobj");
        assert_eq!(boxes[0]["box"]["text"], "in 1");

        // Second box: * 0.5
        assert_eq!(boxes[1]["box"]["maxclass"], "newobj");
        assert_eq!(boxes[1]["box"]["text"], "* 0.5");

        // Third box: outlet~ → "out 1"
        assert_eq!(boxes[2]["box"]["maxclass"], "newobj");
        assert_eq!(boxes[2]["box"]["text"], "out 1");
        assert_eq!(boxes[2]["box"]["numoutlets"], 0);
    }

    // ─── UiData tests ───

    #[test]
    fn test_ui_data_from_json_basic() {
        let json_str = r#"{
            "_patcher": { "rect": [50, 50, 800, 600] },
            "osc": { "rect": [100, 200, 80, 22] },
            "dac": { "rect": [100, 400, 45, 45], "background": 0 }
        }"#;
        let ui = UiData::from_json(json_str).unwrap();

        // Patcher-level settings
        assert_eq!(ui.patcher["rect"], json!([50, 50, 800, 600]));

        // Per-wire entries
        assert!(ui.entries.contains_key("osc"));
        assert!(ui.entries.contains_key("dac"));
        assert!(!ui.entries.contains_key("_patcher"));
        assert_eq!(ui.entries["osc"]["rect"], json!([100, 200, 80, 22]));
        assert_eq!(ui.entries["dac"]["background"], json!(0));
    }

    #[test]
    fn test_ui_data_from_json_empty() {
        let ui = UiData::from_json("{}").unwrap();
        assert!(ui.patcher.is_empty());
        assert!(ui.entries.is_empty());
    }

    #[test]
    fn test_ui_data_from_json_invalid() {
        assert!(UiData::from_json("not json").is_none());
        assert!(UiData::from_json("42").is_none());
        assert!(UiData::from_json("[]").is_none());
    }

    #[test]
    fn test_ui_data_from_json_no_patcher() {
        let json_str = r#"{ "osc": { "rect": [10, 20, 80, 22] } }"#;
        let ui = UiData::from_json(json_str).unwrap();
        assert!(ui.patcher.is_empty());
        assert_eq!(ui.entries.len(), 1);
    }

    #[test]
    fn test_build_box_with_ui_data_rect_override() {
        let node = PatchNode {
            id: "osc".into(),
            object_name: "cycle~".into(),
            args: vec!["440".into()],
            num_inlets: 2,
            num_outlets: 1,
            is_signal: true,
            varname: Some("osc".into()),
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        };
        let ui = UiData::from_json(r#"{ "osc": { "rect": [250, 350, 90, 24] } }"#).unwrap();

        let box_json = build_box(&node, "obj-1", 100.0, 50.0, "box", 1, None, Some(&ui));
        let rect = box_json["box"]["patching_rect"].as_array().unwrap();

        // Should use UI data rect, not auto-layout position
        assert_eq!(rect[0], json!(250));
        assert_eq!(rect[1], json!(350));
        assert_eq!(rect[2], json!(90));
        assert_eq!(rect[3], json!(24));
    }

    #[test]
    fn test_build_box_with_ui_data_decorative_attrs() {
        let node = PatchNode {
            id: "osc".into(),
            object_name: "cycle~".into(),
            args: vec!["440".into()],
            num_inlets: 2,
            num_outlets: 1,
            is_signal: true,
            varname: Some("osc".into()),
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        };
        let ui = UiData::from_json(
            r#"{
            "osc": {
                "rect": [250, 350, 90, 24],
                "background": 0,
                "fontsize": 14
            }
        }"#,
        )
        .unwrap();

        let box_json = build_box(&node, "obj-1", 100.0, 50.0, "box", 1, None, Some(&ui));
        let box_obj = &box_json["box"];

        // Decorative attributes should be present
        assert_eq!(box_obj["background"], json!(0));
        assert_eq!(box_obj["fontsize"], json!(14));
    }

    #[test]
    fn test_build_box_without_varname_ignores_ui_data() {
        let node = PatchNode {
            id: "osc".into(),
            object_name: "cycle~".into(),
            args: vec!["440".into()],
            num_inlets: 2,
            num_outlets: 1,
            is_signal: true,
            varname: None, // no varname
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        };
        let ui = UiData::from_json(r#"{ "osc": { "rect": [250, 350, 90, 24] } }"#).unwrap();

        let box_json = build_box(&node, "obj-1", 100.0, 50.0, "box", 1, None, Some(&ui));
        let rect = box_json["box"]["patching_rect"].as_array().unwrap();

        // Should use auto-layout position since there's no varname to match
        assert_eq!(rect[0], json!(100.0));
        assert_eq!(rect[1], json!(50.0));
    }

    #[test]
    fn test_build_patcher_with_ui_data_patcher_rect() {
        let mut g = PatchGraph::new();
        g.add_node(PatchNode {
            id: "osc".into(),
            object_name: "cycle~".into(),
            args: vec!["440".into()],
            num_inlets: 2,
            num_outlets: 1,
            is_signal: true,
            varname: Some("osc".into()),
            hot_inlets: vec![],
            purity: NodePurity::Unknown,
            attrs: vec![],
            code: None,
        });

        let ui = UiData::from_json(
            r#"{
            "_patcher": { "rect": [50, 50, 800, 600] },
            "osc": { "rect": [200, 300, 80, 22] }
        }"#,
        )
        .unwrap();

        let json_str = generate_with_ui(&g, &GenerateOptions::default(), Some(&ui)).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();

        // Patcher rect should come from UI data
        assert_eq!(parsed["patcher"]["rect"], json!([50, 50, 800, 600]));

        // Box rect should come from UI data
        let boxes = parsed["patcher"]["boxes"].as_array().unwrap();
        assert_eq!(boxes[0]["box"]["patching_rect"], json!([200, 300, 80, 22]));
    }

    #[test]
    fn test_generate_with_ui_none_is_same_as_generate() {
        let graph = make_minimal_graph();

        let json_without = generate(&graph).unwrap();
        let json_with_none = generate_with_ui(&graph, &GenerateOptions::default(), None).unwrap();

        // Both should produce identical output
        assert_eq!(json_without, json_with_none);
    }
}
