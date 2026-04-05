use serde_json::Value;
use std::collections::HashMap;

use flutmax_objdb::{InletSpec, ObjectDb, ObjectDef, OutletSpec, PortType};

/// Static analysis error type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaticErrorType {
    /// Object name not found in the object database.
    UnknownObject,
    /// numinlets in .maxpat doesn't match expected count.
    InletCountMismatch,
    /// numoutlets in .maxpat doesn't match expected count.
    OutletCountMismatch,
    /// Signal outlet connected to a control-only inlet.
    SignalControlMismatch,
    /// maxclass is not a known Max box type.
    InvalidMaxclass,
}

impl std::fmt::Display for StaticErrorType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StaticErrorType::UnknownObject => write!(f, "unknown_object"),
            StaticErrorType::InletCountMismatch => write!(f, "inlet_count_mismatch"),
            StaticErrorType::OutletCountMismatch => write!(f, "outlet_count_mismatch"),
            StaticErrorType::SignalControlMismatch => write!(f, "signal_control_mismatch"),
            StaticErrorType::InvalidMaxclass => write!(f, "invalid_maxclass"),
        }
    }
}

/// A single static check finding.
#[derive(Debug, Clone)]
pub struct StaticCheckError {
    pub error_type: StaticErrorType,
    pub box_id: String,
    pub message: String,
}

impl std::fmt::Display for StaticCheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({}): {}", self.box_id, self.error_type, self.message)
    }
}

/// Entry in the built-in object database.
/// (name, default_inlets, default_outlets, is_signal)
struct BuiltinEntry {
    name: &'static str,
    default_inlets: u32,
    default_outlets: u32,
    is_signal: bool,
}

/// Get a minimal built-in object database for validation.
/// Covers the objects flutmax can generate.
fn builtin_object_db() -> Vec<BuiltinEntry> {
    vec![
        BuiltinEntry {
            name: "cycle~",
            default_inlets: 2,
            default_outlets: 1,
            is_signal: true,
        },
        BuiltinEntry {
            name: "*~",
            default_inlets: 2,
            default_outlets: 1,
            is_signal: true,
        },
        BuiltinEntry {
            name: "+~",
            default_inlets: 2,
            default_outlets: 1,
            is_signal: true,
        },
        BuiltinEntry {
            name: "-~",
            default_inlets: 2,
            default_outlets: 1,
            is_signal: true,
        },
        BuiltinEntry {
            name: "/~",
            default_inlets: 2,
            default_outlets: 1,
            is_signal: true,
        },
        BuiltinEntry {
            name: "%~",
            default_inlets: 2,
            default_outlets: 1,
            is_signal: true,
        },
        BuiltinEntry {
            name: "*",
            default_inlets: 2,
            default_outlets: 1,
            is_signal: false,
        },
        BuiltinEntry {
            name: "+",
            default_inlets: 2,
            default_outlets: 1,
            is_signal: false,
        },
        BuiltinEntry {
            name: "-",
            default_inlets: 2,
            default_outlets: 1,
            is_signal: false,
        },
        BuiltinEntry {
            name: "/",
            default_inlets: 2,
            default_outlets: 1,
            is_signal: false,
        },
        BuiltinEntry {
            name: "%",
            default_inlets: 2,
            default_outlets: 1,
            is_signal: false,
        },
        BuiltinEntry {
            name: "trigger",
            default_inlets: 1,
            default_outlets: 0,
            is_signal: false,
        },
        BuiltinEntry {
            name: "t",
            default_inlets: 1,
            default_outlets: 0,
            is_signal: false,
        },
        BuiltinEntry {
            name: "pack",
            default_inlets: 0,
            default_outlets: 1,
            is_signal: false,
        },
        BuiltinEntry {
            name: "unpack",
            default_inlets: 1,
            default_outlets: 0,
            is_signal: false,
        },
        BuiltinEntry {
            name: "loadbang",
            default_inlets: 1,
            default_outlets: 1,
            is_signal: false,
        },
        BuiltinEntry {
            name: "button",
            default_inlets: 1,
            default_outlets: 1,
            is_signal: false,
        },
        BuiltinEntry {
            name: "print",
            default_inlets: 1,
            default_outlets: 0,
            is_signal: false,
        },
        BuiltinEntry {
            name: "biquad~",
            default_inlets: 6,
            default_outlets: 1,
            is_signal: true,
        },
        BuiltinEntry {
            name: "line~",
            default_inlets: 2,
            default_outlets: 2,
            is_signal: true,
        },
        BuiltinEntry {
            name: "noise~",
            default_inlets: 1,
            default_outlets: 1,
            is_signal: true,
        },
        BuiltinEntry {
            name: "phasor~",
            default_inlets: 2,
            default_outlets: 1,
            is_signal: true,
        },
        BuiltinEntry {
            name: "dac~",
            default_inlets: 2,
            default_outlets: 0,
            is_signal: true,
        },
        BuiltinEntry {
            name: "ezdac~",
            default_inlets: 2,
            default_outlets: 0,
            is_signal: true,
        },
        BuiltinEntry {
            name: "adc~",
            default_inlets: 0,
            default_outlets: 2,
            is_signal: true,
        },
    ]
}

/// Known flutmax alias → Max object name mappings.
/// These aliases resolve to real Max objects and should not trigger UnknownObject.
fn known_aliases() -> HashMap<&'static str, &'static str> {
    let mut m = HashMap::new();
    m.insert("add", "+");
    m.insert("sub", "-");
    m.insert("mul", "*");
    m.insert("dvd", "/");
    m.insert("mod", "%");
    m.insert("add~", "+~");
    m.insert("sub~", "-~");
    m.insert("mul~", "*~");
    m.insert("dvd~", "/~");
    m.insert("mod~", "%~");
    m
}

/// Valid maxclass values in Max.
fn is_valid_maxclass(maxclass: &str) -> bool {
    matches!(
        maxclass,
        "newobj"
            | "inlet"
            | "outlet"
            | "comment"
            | "message"
            | "button"
            | "toggle"
            | "flonum"
            | "number"
            | "slider"
            | "dial"
            | "gain~"
            | "ezdac~"
            | "ezadc~"
            | "scope~"
            | "spectroscope~"
            | "meter~"
            | "number~"
            | "bpatcher"
            | "panel"
            | "live.gain~"
            | "live.dial"
            | "live.slider"
            | "live.toggle"
            | "live.button"
            | "live.numbox"
            | "live.menu"
            | "live.text"
            | "live.tab"
            | "multislider"
            | "matrixctrl"
            | "kslider"
            | "nslider"
            | "umenu"
            | "textedit"
            | "fpic"
            | "pictctrl"
            | "swatch"
            | "attrui"
            | "preset"
            | "dropfile"
    )
}

/// Objects that have variable inlet/outlet counts depending on arguments.
fn is_variable_inlet_object(name: &str) -> bool {
    matches!(name, "pack" | "pak")
}

fn is_variable_outlet_object(name: &str) -> bool {
    matches!(name, "trigger" | "t" | "unpack")
}

/// Information about a box extracted from the .maxpat JSON.
#[derive(Debug)]
struct BoxInfo {
    id: String,
    maxclass: String,
    /// The resolved object name from the `text` field (first token), if applicable.
    object_name: Option<String>,
    numinlets: u32,
    numoutlets: u32,
}

/// Extract the object name (first token) from the `text` field.
fn parse_object_name(text: &str) -> &str {
    text.split_whitespace().next().unwrap_or("")
}

/// Extract all boxes from the .maxpat JSON.
fn extract_boxes(json: &Value) -> Vec<BoxInfo> {
    let mut boxes = Vec::new();

    let patcher = match json.get("patcher") {
        Some(p) => p,
        None => return boxes,
    };

    let boxes_arr = match patcher.get("boxes").and_then(|b| b.as_array()) {
        Some(arr) => arr,
        None => return boxes,
    };

    for box_entry in boxes_arr {
        let inner = match box_entry.get("box") {
            Some(b) => b,
            None => continue,
        };

        let id = inner
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let maxclass = inner
            .get("maxclass")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let numinlets = inner.get("numinlets").and_then(|v| v.as_u64()).unwrap_or(0) as u32;

        let numoutlets = inner
            .get("numoutlets")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;

        let object_name = if maxclass == "newobj" {
            inner
                .get("text")
                .and_then(|v| v.as_str())
                .map(|text| parse_object_name(text).to_string())
        } else {
            None
        };

        boxes.push(BoxInfo {
            id,
            maxclass,
            object_name,
            numinlets,
            numoutlets,
        });
    }

    boxes
}

/// Patchline info for signal/control mismatch checking.
#[derive(Debug)]
struct PatchlineInfo {
    source_id: String,
    dest_id: String,
}

/// Extract all patchlines from the .maxpat JSON.
fn extract_patchlines(json: &Value) -> Vec<PatchlineInfo> {
    let mut lines = Vec::new();

    let patcher = match json.get("patcher") {
        Some(p) => p,
        None => return lines,
    };

    let lines_arr = match patcher.get("lines").and_then(|l| l.as_array()) {
        Some(arr) => arr,
        None => return lines,
    };

    for line_entry in lines_arr {
        let patchline = match line_entry.get("patchline") {
            Some(pl) => pl,
            None => continue,
        };

        let source = patchline.get("source").and_then(|v| v.as_array());
        let dest = patchline.get("destination").and_then(|v| v.as_array());

        if let (Some(src), Some(dst)) = (source, dest) {
            if src.len() == 2 && dst.len() == 2 {
                let source_id = src[0].as_str().unwrap_or("").to_string();
                let dest_id = dst[0].as_str().unwrap_or("").to_string();

                lines.push(PatchlineInfo { source_id, dest_id });
            }
        }
    }

    lines
}

/// Run static analysis on a parsed .maxpat JSON value.
///
/// Validates object names, inlet/outlet counts, and signal/control mismatches
/// against the built-in object database.
pub fn validate_static(json: &Value) -> Vec<StaticCheckError> {
    validate_static_with_objdb(json, None)
}

/// Run static analysis with an optional full ObjectDb.
///
/// When `objdb` is `Some`, objects are first looked up in the full database.
/// If not found there (or if `objdb` is `None`), falls back to the built-in database.
pub fn validate_static_with_objdb(json: &Value, objdb: Option<&ObjectDb>) -> Vec<StaticCheckError> {
    let mut errors = Vec::new();

    // Build lookup from builtin DB
    let builtins = builtin_object_db();
    let mut builtin_db: HashMap<&str, &BuiltinEntry> = HashMap::new();
    for entry in &builtins {
        builtin_db.insert(entry.name, entry);
    }

    let aliases = known_aliases();

    let boxes = extract_boxes(json);
    let patchlines = extract_patchlines(json);

    // Check 0: maxclass validity
    for bx in &boxes {
        if !is_valid_maxclass(&bx.maxclass) {
            errors.push(StaticCheckError {
                error_type: StaticErrorType::InvalidMaxclass,
                box_id: bx.id.clone(),
                message: format!("Invalid maxclass '{}'", bx.maxclass),
            });
        }
    }

    // Build box_id → BoxInfo lookup and box_id → is_signal lookup
    let mut box_map: HashMap<&str, &BoxInfo> = HashMap::new();
    let mut box_is_signal: HashMap<&str, bool> = HashMap::new();

    for bx in &boxes {
        box_map.insert(&bx.id, bx);

        // Determine if this box is a signal object
        if let Some(ref obj_name) = bx.object_name {
            let resolved = aliases
                .get(obj_name.as_str())
                .copied()
                .unwrap_or(obj_name.as_str());

            // Try objdb first, then builtin
            if let Some(def) = objdb.and_then(|db| db.lookup(resolved)) {
                box_is_signal.insert(&bx.id, is_objdef_signal(def));
            } else if let Some(entry) = builtin_db.get(resolved) {
                box_is_signal.insert(&bx.id, entry.is_signal);
            } else {
                // Heuristic: names ending with ~ are signal objects
                box_is_signal.insert(&bx.id, obj_name.ends_with('~'));
            }
        } else {
            box_is_signal.insert(&bx.id, false);
        }
    }

    for bx in &boxes {
        // Only check newobj boxes
        if bx.maxclass != "newobj" {
            continue;
        }

        let obj_name = match &bx.object_name {
            Some(name) if !name.is_empty() => name.as_str(),
            _ => continue,
        };

        // Resolve alias if needed
        let resolved_name = aliases.get(obj_name).copied().unwrap_or(obj_name);

        // Try full objdb first
        if let Some(def) = objdb.and_then(|db| db.lookup(resolved_name)) {
            check_box_with_objdef(bx, obj_name, def, &mut errors);
            continue;
        }

        // Fall back to builtin DB
        let entry = builtin_db.get(resolved_name);
        if entry.is_none() {
            errors.push(StaticCheckError {
                error_type: StaticErrorType::UnknownObject,
                box_id: bx.id.clone(),
                message: format!("Unknown object '{}'", obj_name),
            });
            continue;
        }

        let entry = entry.unwrap();

        // Check 2: Inlet count mismatch
        if !is_variable_inlet_object(resolved_name) && bx.numinlets != entry.default_inlets {
            errors.push(StaticCheckError {
                error_type: StaticErrorType::InletCountMismatch,
                box_id: bx.id.clone(),
                message: format!(
                    "'{}' has numinlets={}, expected {}",
                    obj_name, bx.numinlets, entry.default_inlets
                ),
            });
        }
        // For variable-inlet objects, inlet count depends on arguments — skip check.

        // Check 3: Outlet count mismatch
        if !is_variable_outlet_object(resolved_name) && bx.numoutlets != entry.default_outlets {
            errors.push(StaticCheckError {
                error_type: StaticErrorType::OutletCountMismatch,
                box_id: bx.id.clone(),
                message: format!(
                    "'{}' has numoutlets={}, expected {}",
                    obj_name, bx.numoutlets, entry.default_outlets
                ),
            });
        }
        // For variable-outlet objects, outlet count depends on arguments — skip check.
    }

    // Check 4: Signal→Control mismatch on patchlines
    for pl in &patchlines {
        let source_is_signal = box_is_signal
            .get(pl.source_id.as_str())
            .copied()
            .unwrap_or(false);
        let dest_is_signal = box_is_signal
            .get(pl.dest_id.as_str())
            .copied()
            .unwrap_or(false);

        // If the source is a signal object and destination is not, that's a mismatch
        if source_is_signal && !dest_is_signal {
            // Look up the destination box to get its name
            let dest_name = box_map
                .get(pl.dest_id.as_str())
                .and_then(|b| b.object_name.as_deref())
                .unwrap_or(&pl.dest_id);
            let src_name = box_map
                .get(pl.source_id.as_str())
                .and_then(|b| b.object_name.as_deref())
                .unwrap_or(&pl.source_id);

            errors.push(StaticCheckError {
                error_type: StaticErrorType::SignalControlMismatch,
                box_id: pl.dest_id.clone(),
                message: format!(
                    "Signal outlet from '{}' connected to control inlet of '{}'",
                    src_name, dest_name
                ),
            });
        }
    }

    errors
}

/// Check a box against an ObjectDef from the full objdb.
fn check_box_with_objdef(
    bx: &BoxInfo,
    obj_name: &str,
    def: &ObjectDef,
    errors: &mut Vec<StaticCheckError>,
) {
    // Check inlet count (skip for variable-inlet objects)
    if !def.has_variable_inlets() {
        let expected = def.default_inlet_count() as u32;
        if bx.numinlets != expected {
            errors.push(StaticCheckError {
                error_type: StaticErrorType::InletCountMismatch,
                box_id: bx.id.clone(),
                message: format!(
                    "'{}' has numinlets={}, expected {}",
                    obj_name, bx.numinlets, expected
                ),
            });
        }
    }

    // Check outlet count (skip for variable-outlet objects)
    if !def.has_variable_outlets() {
        let expected = def.default_outlet_count() as u32;
        if bx.numoutlets != expected {
            errors.push(StaticCheckError {
                error_type: StaticErrorType::OutletCountMismatch,
                box_id: bx.id.clone(),
                message: format!(
                    "'{}' has numoutlets={}, expected {}",
                    obj_name, bx.numoutlets, expected
                ),
            });
        }
    }
}

/// Determine if an ObjectDef represents a signal-rate object.
///
/// An object is considered "signal" if any of its outlets produce signal data.
fn is_objdef_signal(def: &ObjectDef) -> bool {
    let outlets = match &def.outlets {
        OutletSpec::Fixed(ports) => ports,
        OutletSpec::Variable { defaults, .. } => defaults,
    };
    // If any outlet produces signal, it's a signal object
    if outlets.iter().any(|p| p.port_type.accepts_signal()) {
        return true;
    }

    // Also check inlets: if the first inlet accepts signal, it's likely a signal object
    let inlets = match &def.inlets {
        InletSpec::Fixed(ports) => ports,
        InletSpec::Variable { defaults, .. } => defaults,
    };
    if let Some(first) = inlets.first() {
        if first.port_type == PortType::Signal {
            return true;
        }
    }

    // Fallback: name ends with ~
    def.name.ends_with('~')
}

/// Refpage subdirectories relative to the C74 directory.
/// Insert an ObjectDef only if the existing entry has fewer inlet definitions.
/// This prevents package-specific entries (e.g., RNBO cycle~ with no inlet list)
/// from overriding richer standard entries (e.g., msp cycle~ with full inlet descriptions).
fn insert_if_richer(db: &mut flutmax_objdb::ObjectDb, def: flutmax_objdb::ObjectDef) {
    if let Some(existing) = db.lookup(&def.name) {
        if existing.default_inlet_count() >= def.default_inlet_count() {
            return; // Existing entry is richer or equal — keep it
        }
    }
    db.insert(def);
}

const REFPAGE_SUBDIRS: &[&str] = &[
    "docs/refpages/max-ref",
    "docs/refpages/msp-ref",
    "docs/refpages/jit-ref",
    "docs/refpages/m4l-ref",
];

/// Find the Max C74 resource directory.
///
/// Search order:
/// 1. `MAX_INSTALL_PATH` environment variable → derive C74 path
/// 2. OS-specific default locations
///
/// Returns `None` if Max is not installed.
pub fn find_max_c74_dir() -> Option<std::path::PathBuf> {
    // 1. Environment variable
    if let Ok(install_path) = std::env::var("MAX_INSTALL_PATH") {
        let base = std::path::PathBuf::from(&install_path);
        // macOS: Max.app/Contents/Resources/C74
        let macos_c74 = base.join("Contents").join("Resources").join("C74");
        if macos_c74.is_dir() {
            return Some(macos_c74);
        }
        // Windows: Max 8/resources/C74
        let win_c74 = base.join("resources").join("C74");
        if win_c74.is_dir() {
            return Some(win_c74);
        }
        // Direct C74 path
        if base.join("docs").join("refpages").is_dir() {
            return Some(base);
        }
    }

    // 2. OS-specific defaults
    #[cfg(target_os = "macos")]
    {
        let default = std::path::PathBuf::from("/Applications/Max.app/Contents/Resources/C74");
        if default.is_dir() {
            return Some(default);
        }
    }
    #[cfg(target_os = "windows")]
    {
        for version in &["Max 9", "Max 8"] {
            let default = std::path::PathBuf::from(format!(
                "C:\\Program Files\\Cycling '74\\{}\\resources\\C74",
                version
            ));
            if default.is_dir() {
                return Some(default);
            }
        }
    }

    None
}

/// Try to load the ObjectDb from standard Max refpage locations.
///
/// Scans max-ref, msp-ref, jit-ref, m4l-ref, and all Package refpage directories.
/// Returns `None` if Max is not installed or refpages are unavailable.
pub fn try_load_max_objdb() -> Option<ObjectDb> {
    let c74_dir = find_max_c74_dir()?;
    let mut db = ObjectDb::new();

    // Load standard refpage directories
    for subdir in REFPAGE_SUBDIRS {
        let dir = c74_dir.join(subdir);
        if !dir.is_dir() {
            continue;
        }
        if let Ok((loaded, _errors)) = flutmax_objdb::parser::load_directory(&dir) {
            for name in loaded.names() {
                if let Some(def) = loaded.lookup(name) {
                    db.insert(def.clone());
                }
            }
        }
    }

    // Scan Package directories recursively.
    // Only insert package entries that don't override standard entries with
    // LESS information (e.g., RNBO cycle~ has no inlet list vs msp cycle~ which does).
    let packages_dir = c74_dir.join("packages");
    if packages_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(packages_dir) {
            for entry in entries.flatten() {
                let pkg_path = entry.path();
                if !pkg_path.is_dir() {
                    continue;
                }

                // Try "docs/refpages" (most packages)
                let pkg_refpages = pkg_path.join("docs").join("refpages");
                if pkg_refpages.is_dir() {
                    if let Ok((loaded, _)) =
                        flutmax_objdb::parser::load_directory_recursive(&pkg_refpages)
                    {
                        for name in loaded.names() {
                            if let Some(def) = loaded.lookup(name) {
                                insert_if_richer(&mut db, def.clone());
                            }
                        }
                    }
                }

                // Some packages use "refpages1" instead of "refpages" (e.g., Gen)
                let pkg_refpages1 = pkg_path.join("docs").join("refpages1");
                if pkg_refpages1.is_dir() {
                    if let Ok((loaded, _)) =
                        flutmax_objdb::parser::load_directory_recursive(&pkg_refpages1)
                    {
                        for name in loaded.names() {
                            if let Some(def) = loaded.lookup(name) {
                                insert_if_richer(&mut db, def.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    if db.is_empty() {
        None
    } else {
        Some(db)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Helper: build a minimal valid .maxpat with given boxes and lines.
    fn make_maxpat(boxes: Vec<Value>, lines: Vec<Value>) -> Value {
        json!({
            "patcher": {
                "fileversion": 1,
                "boxes": boxes,
                "lines": lines
            }
        })
    }

    /// Helper: build a newobj box.
    fn newobj_box(id: &str, text: &str, numinlets: u32, numoutlets: u32) -> Value {
        json!({
            "box": {
                "id": id,
                "maxclass": "newobj",
                "text": text,
                "numinlets": numinlets,
                "numoutlets": numoutlets,
                "patching_rect": [100.0, 200.0, 80.0, 22.0]
            }
        })
    }

    /// Helper: build a patchline.
    fn patchline(src_id: &str, src_outlet: u32, dst_id: &str, dst_inlet: u32) -> Value {
        json!({
            "patchline": {
                "source": [src_id, src_outlet],
                "destination": [dst_id, dst_inlet]
            }
        })
    }

    #[test]
    fn valid_maxpat_known_objects_no_errors() {
        let maxpat = make_maxpat(
            vec![
                newobj_box("obj-1", "cycle~ 440", 2, 1),
                newobj_box("obj-2", "dac~", 2, 0),
            ],
            vec![patchline("obj-1", 0, "obj-2", 0)],
        );
        let errors = validate_static(&maxpat);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn unknown_object_warning() {
        let maxpat = make_maxpat(
            vec![newobj_box("obj-1", "nonexistent_object~ 440", 2, 1)],
            vec![],
        );
        let errors = validate_static(&maxpat);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].error_type, StaticErrorType::UnknownObject);
        assert_eq!(errors[0].box_id, "obj-1");
        assert!(errors[0].message.contains("nonexistent_object~"));
    }

    #[test]
    fn inlet_count_mismatch() {
        let maxpat = make_maxpat(
            vec![newobj_box("obj-1", "cycle~ 440", 5, 1)], // cycle~ has 2 inlets, not 5
            vec![],
        );
        let errors = validate_static(&maxpat);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].error_type, StaticErrorType::InletCountMismatch);
        assert_eq!(errors[0].box_id, "obj-1");
        assert!(errors[0].message.contains("numinlets=5"));
        assert!(errors[0].message.contains("expected 2"));
    }

    #[test]
    fn outlet_count_mismatch() {
        let maxpat = make_maxpat(
            vec![newobj_box("obj-1", "cycle~ 440", 2, 3)], // cycle~ has 1 outlet, not 3
            vec![],
        );
        let errors = validate_static(&maxpat);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].error_type, StaticErrorType::OutletCountMismatch);
        assert_eq!(errors[0].box_id, "obj-1");
        assert!(errors[0].message.contains("numoutlets=3"));
        assert!(errors[0].message.contains("expected 1"));
    }

    #[test]
    fn trigger_variable_outlets_no_false_positive() {
        // trigger with 3 outlets (from "t b b b") — should not report mismatch
        let maxpat = make_maxpat(vec![newobj_box("obj-1", "trigger b b b", 1, 3)], vec![]);
        let errors = validate_static(&maxpat);
        assert!(
            errors.is_empty(),
            "Expected no errors for trigger with variable outlets, got: {:?}",
            errors
        );
    }

    #[test]
    fn trigger_alias_variable_outlets_no_false_positive() {
        // "t" alias for trigger with 2 outlets — should not report mismatch
        let maxpat = make_maxpat(vec![newobj_box("obj-1", "t b i", 1, 2)], vec![]);
        let errors = validate_static(&maxpat);
        assert!(
            errors.is_empty(),
            "Expected no errors for 't' alias with variable outlets, got: {:?}",
            errors
        );
    }

    #[test]
    fn pack_variable_inlets_no_false_positive() {
        // pack with 4 inlets (from "pack 0 0 0 0") — should not report mismatch
        let maxpat = make_maxpat(vec![newobj_box("obj-1", "pack 0 0 0 0", 4, 1)], vec![]);
        let errors = validate_static(&maxpat);
        assert!(
            errors.is_empty(),
            "Expected no errors for pack with variable inlets, got: {:?}",
            errors
        );
    }

    #[test]
    fn unpack_variable_outlets_no_false_positive() {
        // unpack with 3 outlets — should not report mismatch
        let maxpat = make_maxpat(vec![newobj_box("obj-1", "unpack 0 0 0", 1, 3)], vec![]);
        let errors = validate_static(&maxpat);
        assert!(
            errors.is_empty(),
            "Expected no errors for unpack with variable outlets, got: {:?}",
            errors
        );
    }

    #[test]
    fn signal_to_control_mismatch() {
        // cycle~ (signal) → print (control) — mismatch
        let maxpat = make_maxpat(
            vec![
                newobj_box("obj-1", "cycle~ 440", 2, 1),
                newobj_box("obj-2", "print", 1, 0),
            ],
            vec![patchline("obj-1", 0, "obj-2", 0)],
        );
        let errors = validate_static(&maxpat);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].error_type, StaticErrorType::SignalControlMismatch);
        assert!(errors[0].message.contains("cycle~"));
        assert!(errors[0].message.contains("print"));
    }

    #[test]
    fn signal_to_signal_no_mismatch() {
        // cycle~ (signal) → *~ (signal) — ok
        let maxpat = make_maxpat(
            vec![
                newobj_box("obj-1", "cycle~ 440", 2, 1),
                newobj_box("obj-2", "*~ 0.5", 2, 1),
            ],
            vec![patchline("obj-1", 0, "obj-2", 0)],
        );
        let errors = validate_static(&maxpat);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn control_to_control_no_mismatch() {
        // button → print — ok
        let maxpat = make_maxpat(
            vec![
                newobj_box("obj-1", "button", 1, 1),
                newobj_box("obj-2", "print", 1, 0),
            ],
            vec![patchline("obj-1", 0, "obj-2", 0)],
        );
        let errors = validate_static(&maxpat);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn non_newobj_boxes_skipped() {
        // Non-newobj maxclass boxes should not be checked
        let maxpat = json!({
            "patcher": {
                "fileversion": 1,
                "boxes": [
                    {
                        "box": {
                            "id": "obj-1",
                            "maxclass": "comment",
                            "text": "this is a comment",
                            "numinlets": 1,
                            "numoutlets": 0,
                            "patching_rect": [100.0, 100.0, 150.0, 20.0]
                        }
                    }
                ],
                "lines": []
            }
        });
        let errors = validate_static(&maxpat);
        assert!(
            errors.is_empty(),
            "Expected no errors for comment box, got: {:?}",
            errors
        );
    }

    #[test]
    fn flutmax_alias_resolves_correctly() {
        // The codegen emits "+", "-", etc. after alias resolution.
        // These should be found in the builtin DB.
        let maxpat = make_maxpat(
            vec![
                newobj_box("obj-1", "+ 1", 2, 1),
                newobj_box("obj-2", "- 1", 2, 1),
                newobj_box("obj-3", "* 2", 2, 1),
                newobj_box("obj-4", "/ 2", 2, 1),
                newobj_box("obj-5", "*~ 0.5", 2, 1),
            ],
            vec![],
        );
        let errors = validate_static(&maxpat);
        assert!(
            errors.is_empty(),
            "Expected no errors for alias objects, got: {:?}",
            errors
        );
    }

    #[test]
    fn empty_patcher_no_errors() {
        let maxpat = make_maxpat(vec![], vec![]);
        let errors = validate_static(&maxpat);
        assert!(errors.is_empty());
    }

    #[test]
    fn multiple_errors_reported() {
        let maxpat = make_maxpat(
            vec![
                newobj_box("obj-1", "unknown_obj", 1, 1),
                newobj_box("obj-2", "cycle~ 440", 5, 3), // both inlet and outlet wrong
            ],
            vec![],
        );
        let errors = validate_static(&maxpat);
        // obj-1: UnknownObject, obj-2: InletCountMismatch + OutletCountMismatch
        assert_eq!(errors.len(), 3, "Expected 3 errors, got: {:?}", errors);

        let unknown = errors
            .iter()
            .find(|e| e.error_type == StaticErrorType::UnknownObject);
        assert!(unknown.is_some());

        let inlet = errors
            .iter()
            .find(|e| e.error_type == StaticErrorType::InletCountMismatch);
        assert!(inlet.is_some());

        let outlet = errors
            .iter()
            .find(|e| e.error_type == StaticErrorType::OutletCountMismatch);
        assert!(outlet.is_some());
    }

    #[test]
    fn missing_patcher_key_no_panic() {
        let json = json!({"something": "else"});
        let errors = validate_static(&json);
        assert!(errors.is_empty());
    }

    #[test]
    fn biquad_correct_inlets() {
        let maxpat = make_maxpat(vec![newobj_box("obj-1", "biquad~ 1 0 0 0 0", 6, 1)], vec![]);
        let errors = validate_static(&maxpat);
        assert!(
            errors.is_empty(),
            "Expected no errors for biquad~, got: {:?}",
            errors
        );
    }

    #[test]
    fn invalid_maxclass_detected() {
        let json = make_maxpat(
            vec![
                json!({"box": {"id": "obj-1", "maxclass": "outlet~", "numinlets": 1, "numoutlets": 0, "patching_rect": [0, 0, 30, 30]}}),
            ],
            vec![],
        );
        let errors = validate_static(&json);
        let mc_errors: Vec<_> = errors
            .iter()
            .filter(|e| e.error_type == StaticErrorType::InvalidMaxclass)
            .collect();
        assert_eq!(mc_errors.len(), 1);
        assert!(mc_errors[0].message.contains("outlet~"));
    }

    #[test]
    fn valid_maxclass_no_error() {
        let json = make_maxpat(
            vec![
                json!({"box": {"id": "obj-1", "maxclass": "outlet", "numinlets": 1, "numoutlets": 0, "patching_rect": [0, 0, 30, 30]}}),
                json!({"box": {"id": "obj-2", "maxclass": "inlet", "numinlets": 0, "numoutlets": 1, "outlettype": [""], "patching_rect": [0, 0, 30, 30]}}),
            ],
            vec![],
        );
        let errors = validate_static(&json);
        let mc_errors: Vec<_> = errors
            .iter()
            .filter(|e| e.error_type == StaticErrorType::InvalidMaxclass)
            .collect();
        assert!(mc_errors.is_empty());
    }

    #[test]
    fn static_error_type_display() {
        assert_eq!(
            format!("{}", StaticErrorType::UnknownObject),
            "unknown_object"
        );
        assert_eq!(
            format!("{}", StaticErrorType::InletCountMismatch),
            "inlet_count_mismatch"
        );
        assert_eq!(
            format!("{}", StaticErrorType::OutletCountMismatch),
            "outlet_count_mismatch"
        );
        assert_eq!(
            format!("{}", StaticErrorType::SignalControlMismatch),
            "signal_control_mismatch"
        );
        assert_eq!(
            format!("{}", StaticErrorType::InvalidMaxclass),
            "invalid_maxclass"
        );
    }

    #[test]
    fn static_check_error_display() {
        let err = StaticCheckError {
            error_type: StaticErrorType::UnknownObject,
            box_id: "obj-1".to_string(),
            message: "Unknown object 'foo'".to_string(),
        };
        let display = format!("{}", err);
        assert_eq!(display, "obj-1 (unknown_object): Unknown object 'foo'");
    }

    // ---- objdb integration tests ----

    use flutmax_objdb::{ArgDef, InletSpec, Module, ObjectDef, OutletSpec, PortDef};

    /// Helper: build a minimal ObjectDb with cycle~ for testing.
    fn make_test_objdb() -> ObjectDb {
        let mut db = ObjectDb::new();
        db.insert(ObjectDef {
            name: "cycle~".to_string(),
            module: Module::Msp,
            category: "MSP Synthesis".to_string(),
            digest: "Sinusoidal oscillator".to_string(),
            inlets: InletSpec::Fixed(vec![
                PortDef {
                    id: 0,
                    port_type: PortType::SignalFloat,
                    is_hot: true,
                    description: "Frequency".to_string(),
                },
                PortDef {
                    id: 1,
                    port_type: PortType::SignalFloat,
                    is_hot: false,
                    description: "Phase (0-1)".to_string(),
                },
            ]),
            outlets: OutletSpec::Fixed(vec![PortDef {
                id: 0,
                port_type: PortType::Signal,
                is_hot: false,
                description: "Output".to_string(),
            }]),
            args: vec![],
        });
        db.insert(ObjectDef {
            name: "print".to_string(),
            module: Module::Max,
            category: "Debug".to_string(),
            digest: "Print to console".to_string(),
            inlets: InletSpec::Fixed(vec![PortDef {
                id: 0,
                port_type: PortType::Any,
                is_hot: true,
                description: "Input".to_string(),
            }]),
            outlets: OutletSpec::Fixed(vec![]),
            args: vec![],
        });
        db.insert(ObjectDef {
            name: "trigger".to_string(),
            module: Module::Max,
            category: "Control".to_string(),
            digest: "Send input to many places".to_string(),
            inlets: InletSpec::Variable {
                defaults: vec![PortDef {
                    id: 0,
                    port_type: PortType::Dynamic,
                    is_hot: true,
                    description: "Input".to_string(),
                }],
                min_inlets: 1,
            },
            outlets: OutletSpec::Variable {
                defaults: vec![
                    PortDef {
                        id: 0,
                        port_type: PortType::Dynamic,
                        is_hot: false,
                        description: "Output 1".to_string(),
                    },
                    PortDef {
                        id: 1,
                        port_type: PortType::Dynamic,
                        is_hot: false,
                        description: "Output 2".to_string(),
                    },
                ],
                min_outlets: 1,
            },
            args: vec![ArgDef {
                name: "formats".to_string(),
                arg_type: "symbol".to_string(),
                optional: true,
            }],
        });
        db
    }

    #[test]
    fn objdb_none_behaves_same_as_validate_static() {
        let maxpat = make_maxpat(
            vec![
                newobj_box("obj-1", "cycle~ 440", 2, 1),
                newobj_box("obj-2", "dac~", 2, 0),
            ],
            vec![patchline("obj-1", 0, "obj-2", 0)],
        );
        let errors_old = validate_static(&maxpat);
        let errors_new = validate_static_with_objdb(&maxpat, None);
        assert_eq!(errors_old.len(), errors_new.len());
        for (a, b) in errors_old.iter().zip(errors_new.iter()) {
            assert_eq!(a.error_type, b.error_type);
            assert_eq!(a.box_id, b.box_id);
            assert_eq!(a.message, b.message);
        }
    }

    #[test]
    fn objdb_none_unknown_object_same_as_validate_static() {
        let maxpat = make_maxpat(vec![newobj_box("obj-1", "nonexistent_obj", 1, 1)], vec![]);
        let errors_old = validate_static(&maxpat);
        let errors_new = validate_static_with_objdb(&maxpat, None);
        assert_eq!(errors_old.len(), errors_new.len());
        assert_eq!(errors_old[0].error_type, errors_new[0].error_type);
    }

    #[test]
    fn objdb_recognizes_cycle_from_objdb() {
        let db = make_test_objdb();
        let maxpat = make_maxpat(vec![newobj_box("obj-1", "cycle~ 440", 2, 1)], vec![]);
        let errors = validate_static_with_objdb(&maxpat, Some(&db));
        assert!(
            errors.is_empty(),
            "Expected no errors with objdb, got: {:?}",
            errors
        );
    }

    #[test]
    fn objdb_detects_inlet_mismatch() {
        let db = make_test_objdb();
        let maxpat = make_maxpat(vec![newobj_box("obj-1", "cycle~ 440", 5, 1)], vec![]);
        let errors = validate_static_with_objdb(&maxpat, Some(&db));
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].error_type, StaticErrorType::InletCountMismatch);
        assert!(errors[0].message.contains("numinlets=5"));
        assert!(errors[0].message.contains("expected 2"));
    }

    #[test]
    fn objdb_detects_outlet_mismatch() {
        let db = make_test_objdb();
        let maxpat = make_maxpat(vec![newobj_box("obj-1", "cycle~ 440", 2, 5)], vec![]);
        let errors = validate_static_with_objdb(&maxpat, Some(&db));
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].error_type, StaticErrorType::OutletCountMismatch);
        assert!(errors[0].message.contains("numoutlets=5"));
        assert!(errors[0].message.contains("expected 1"));
    }

    #[test]
    fn objdb_variable_outlets_no_false_positive() {
        let db = make_test_objdb();
        // trigger has variable outlets in the objdb
        let maxpat = make_maxpat(vec![newobj_box("obj-1", "trigger b b b", 1, 3)], vec![]);
        let errors = validate_static_with_objdb(&maxpat, Some(&db));
        assert!(
            errors.is_empty(),
            "Expected no errors for trigger with variable outlets via objdb, got: {:?}",
            errors
        );
    }

    #[test]
    fn objdb_unknown_object_falls_back_to_builtin() {
        let db = make_test_objdb();
        // dac~ is NOT in our test objdb but IS in the builtin db
        let maxpat = make_maxpat(vec![newobj_box("obj-1", "dac~", 2, 0)], vec![]);
        let errors = validate_static_with_objdb(&maxpat, Some(&db));
        assert!(
            errors.is_empty(),
            "Expected dac~ to be found via builtin fallback, got: {:?}",
            errors
        );
    }

    #[test]
    fn objdb_truly_unknown_object_still_detected() {
        let db = make_test_objdb();
        let maxpat = make_maxpat(
            vec![newobj_box("obj-1", "completely_fake_object", 1, 1)],
            vec![],
        );
        let errors = validate_static_with_objdb(&maxpat, Some(&db));
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].error_type, StaticErrorType::UnknownObject);
        assert!(errors[0].message.contains("completely_fake_object"));
    }

    #[test]
    fn objdb_signal_mismatch_with_objdb() {
        let db = make_test_objdb();
        // cycle~ (signal via objdb) → print (control via objdb) — mismatch
        let maxpat = make_maxpat(
            vec![
                newobj_box("obj-1", "cycle~ 440", 2, 1),
                newobj_box("obj-2", "print", 1, 0),
            ],
            vec![patchline("obj-1", 0, "obj-2", 0)],
        );
        let errors = validate_static_with_objdb(&maxpat, Some(&db));
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].error_type, StaticErrorType::SignalControlMismatch);
        assert!(errors[0].message.contains("cycle~"));
        assert!(errors[0].message.contains("print"));
    }

    #[test]
    fn objdb_is_objdef_signal_detection() {
        // Test the is_objdef_signal helper
        let db = make_test_objdb();
        let cycle_def = db.lookup("cycle~").unwrap();
        assert!(is_objdef_signal(cycle_def));

        let print_def = db.lookup("print").unwrap();
        assert!(!is_objdef_signal(print_def));
    }

    #[test]
    fn try_load_max_objdb_returns_some_or_none() {
        // This test verifies the function does not panic regardless of environment.
        // On machines with Max installed, it returns Some with 800+ objects.
        // On CI (no Max), it returns None.
        let result = try_load_max_objdb();
        if let Some(ref db) = result {
            assert!(
                db.len() > 100,
                "Expected > 100 objects when Max is available, got {}",
                db.len()
            );
            // Spot-check well-known objects
            assert!(db.lookup("cycle~").is_some());
            assert!(db.lookup("trigger").is_some());
        }
        // If None, Max is not installed — that's fine
    }

    #[test]
    fn test_objdb_loads_packages() {
        if let Some(db) = try_load_max_objdb() {
            eprintln!("objdb loaded {} objects", db.len());
            assert!(
                db.len() > 1500,
                "Expected 1500+ objects with packages, got {}",
                db.len()
            );
            // Check some package-specific objects exist
            // RNBO objects
            if db.lookup("rnbo~").is_some() || db.lookup("in~").is_some() {
                eprintln!("  RNBO objects present");
            }
            // Jitter objects
            if db.lookup("jit.matrix").is_some() {
                eprintln!("  Jitter objects present");
            }
            // M4L objects
            if db.lookup("live.dial").is_some() {
                eprintln!("  M4L objects present");
            }
        } else {
            eprintln!("SKIP: Max.app not installed");
        }
    }
}
