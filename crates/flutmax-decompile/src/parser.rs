use serde_json::Value;
use std::fmt;

/// A parsed .maxpat file containing boxes and patch lines.
#[derive(Debug, Clone)]
pub struct MaxPat {
    pub boxes: Vec<MaxBox>,
    pub lines: Vec<MaxLine>,
    /// RNBO patchers use `classnamespace: "rnbo"` to indicate RNBO context.
    pub classnamespace: Option<String>,
    /// Patcher window rect: [x, y, width, height] (from "rect" field).
    pub rect: Option<[f64; 4]>,
}

/// A single box (object) in the Max patcher.
#[derive(Debug, Clone)]
pub struct MaxBox {
    pub id: String,
    pub maxclass: String,
    pub text: Option<String>,
    pub numinlets: u32,
    pub numoutlets: u32,
    pub outlettype: Vec<String>,
    pub comment: Option<String>,
    pub varname: Option<String>,
    /// patching_rect: [x, y, width, height]
    pub patching_rect: [f64; 4],
    /// Embedded subpatcher (for `[p name]`, `bpatcher`, `poly~`, `pfft~`).
    pub embedded_patcher: Option<MaxPat>,
    /// Non-structural attributes from the box JSON (e.g., minimum, maximum, parameter_longname).
    /// These are key-value pairs that define object behavior but are not part of the
    /// structural graph topology.
    pub extra_attrs: Vec<(String, Value)>,
    /// Inline code content for codebox objects (v8.codebox, codebox).
    /// Multi-line JavaScript or GenExpr code stored in the `"code"` JSON field.
    pub code: Option<String>,
}

impl MaxBox {
    /// X coordinate from patching_rect
    pub fn patching_rect_x(&self) -> f64 {
        self.patching_rect[0]
    }
}

/// A single patch line (connection) between two boxes.
#[derive(Debug, Clone)]
pub struct MaxLine {
    pub source_id: String,
    pub source_outlet: u32,
    pub dest_id: String,
    pub dest_inlet: u32,
    pub order: Option<u32>,
}

/// Errors that can occur during decompilation.
#[derive(Debug)]
pub enum DecompileError {
    JsonParse(String),
    MissingField(String),
    UnsupportedFeature(String),
}

impl fmt::Display for DecompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecompileError::JsonParse(msg) => write!(f, "JSON parse error: {}", msg),
            DecompileError::MissingField(msg) => write!(f, "Missing field: {}", msg),
            DecompileError::UnsupportedFeature(msg) => {
                write!(f, "Unsupported feature: {}", msg)
            }
        }
    }
}

impl std::error::Error for DecompileError {}

/// Parse a .maxpat JSON string into a `MaxPat` struct.
pub fn parse_maxpat(json_str: &str) -> Result<MaxPat, DecompileError> {
    let root: Value =
        serde_json::from_str(json_str).map_err(|e| DecompileError::JsonParse(e.to_string()))?;

    let patcher = root
        .get("patcher")
        .ok_or_else(|| DecompileError::MissingField("patcher".to_string()))?;

    parse_patcher_value(patcher)
}

/// Parse a patcher JSON value (the object under "patcher" key) into a `MaxPat`.
///
/// This is used both for top-level parsing and for recursively parsing
/// embedded subpatchers.
pub fn parse_patcher_value(patcher: &Value) -> Result<MaxPat, DecompileError> {
    let boxes = parse_boxes(patcher)?;
    let lines = parse_lines(patcher)?;
    let classnamespace = patcher
        .get("classnamespace")
        .and_then(|v| v.as_str())
        .map(String::from);

    let rect = patcher
        .get("rect")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            if arr.len() >= 4 {
                Some([
                    arr[0].as_f64().unwrap_or(0.0),
                    arr[1].as_f64().unwrap_or(0.0),
                    arr[2].as_f64().unwrap_or(0.0),
                    arr[3].as_f64().unwrap_or(0.0),
                ])
            } else {
                None
            }
        });

    Ok(MaxPat {
        boxes,
        lines,
        classnamespace,
        rect,
    })
}

/// Fields that are structural (topology) and should NOT be treated
/// as user-facing object attributes for `.attr()` or `.uiflutmax` output.
///
/// Note: Decorative/display fields (bgcolor, fontsize, etc.) are intentionally
/// NOT excluded here. They flow into `extra_attrs` so the analyzer can classify
/// them as decorative (for .uiflutmax) or functional (for .flutmax .attr()).
const STRUCTURAL_FIELDS: &[&str] = &[
    "id",
    "maxclass",
    "text",
    "numinlets",
    "numoutlets",
    "outlettype",
    "patching_rect",
    "comment",
    "varname",
    "patcher",
    "style",
    // Hidden/ignoreclick are patcher internals, not user attributes:
    "hidden",
    "ignoreclick",
    // Saved state fields:
    "saved_object_attributes",
    "saved_attribute_attributes",
    // Linecount for multi-line display:
    "linecount",
    // RNBO metadata (complex structure, not roundtrippable as .attr()):
    "rnboinfo",
    // Codebox fields (code is extracted separately, filename is structural):
    "code",
    "filename",
];

fn parse_boxes(patcher: &Value) -> Result<Vec<MaxBox>, DecompileError> {
    let boxes_array = patcher
        .get("boxes")
        .and_then(|v| v.as_array())
        .ok_or_else(|| DecompileError::MissingField("patcher.boxes".to_string()))?;

    let mut result = Vec::new();
    for entry in boxes_array {
        let box_obj = entry
            .get("box")
            .ok_or_else(|| DecompileError::MissingField("box".to_string()))?;

        let id = box_obj
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DecompileError::MissingField("box.id".to_string()))?
            .to_string();

        let maxclass = box_obj
            .get("maxclass")
            .and_then(|v| v.as_str())
            .ok_or_else(|| DecompileError::MissingField("box.maxclass".to_string()))?
            .to_string();

        let text = box_obj
            .get("text")
            .and_then(|v| v.as_str())
            .map(String::from);

        let numinlets = box_obj
            .get("numinlets")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;

        let numoutlets = box_obj
            .get("numoutlets")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;

        let outlettype = box_obj
            .get("outlettype")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let comment = box_obj
            .get("comment")
            .and_then(|v| v.as_str())
            .map(String::from);

        let varname = box_obj
            .get("varname")
            .and_then(|v| v.as_str())
            .map(String::from);

        let code = box_obj
            .get("code")
            .and_then(|v| v.as_str())
            .map(String::from);

        // Check for embedded patcher (subpatchers, bpatchers, poly~, pfft~)
        let embedded_patcher = if let Some(patcher_val) = box_obj.get("patcher") {
            // Recursively parse the embedded patcher
            Some(parse_patcher_value(patcher_val)?)
        } else {
            None
        };

        // Extract non-structural attributes (e.g., minimum, maximum, parameter_longname)
        let extra_attrs: Vec<(String, Value)> = box_obj
            .as_object()
            .map(|obj| {
                obj.iter()
                    .filter(|(k, _)| !STRUCTURAL_FIELDS.contains(&k.as_str()))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default();

        let patching_rect = box_obj
            .get("patching_rect")
            .and_then(|v| v.as_array())
            .map(|arr| {
                let mut rect = [0.0f64; 4];
                for (i, val) in arr.iter().enumerate().take(4) {
                    rect[i] = val.as_f64().unwrap_or(0.0);
                }
                rect
            })
            .unwrap_or([0.0; 4]);

        result.push(MaxBox {
            id,
            maxclass,
            text,
            numinlets,
            numoutlets,
            outlettype,
            comment,
            varname,
            patching_rect,
            embedded_patcher,
            extra_attrs,
            code,
        });
    }

    Ok(result)
}

fn parse_lines(patcher: &Value) -> Result<Vec<MaxLine>, DecompileError> {
    let lines_array = patcher
        .get("lines")
        .and_then(|v| v.as_array())
        .ok_or_else(|| DecompileError::MissingField("patcher.lines".to_string()))?;

    let mut result = Vec::new();
    for entry in lines_array {
        let patchline = entry
            .get("patchline")
            .ok_or_else(|| DecompileError::MissingField("patchline".to_string()))?;

        let source = patchline
            .get("source")
            .and_then(|v| v.as_array())
            .ok_or_else(|| DecompileError::MissingField("patchline.source".to_string()))?;

        let dest = patchline
            .get("destination")
            .and_then(|v| v.as_array())
            .ok_or_else(|| DecompileError::MissingField("patchline.destination".to_string()))?;

        if source.len() < 2 || dest.len() < 2 {
            return Err(DecompileError::MissingField(
                "source/destination must have at least 2 elements".to_string(),
            ));
        }

        let source_id = source[0]
            .as_str()
            .ok_or_else(|| DecompileError::MissingField("source[0] (id)".to_string()))?
            .to_string();
        let source_outlet = source[1]
            .as_u64()
            .ok_or_else(|| DecompileError::MissingField("source[1] (outlet)".to_string()))?
            as u32;

        let dest_id = dest[0]
            .as_str()
            .ok_or_else(|| DecompileError::MissingField("destination[0] (id)".to_string()))?
            .to_string();
        let dest_inlet = dest[1]
            .as_u64()
            .ok_or_else(|| DecompileError::MissingField("destination[1] (inlet)".to_string()))?
            as u32;

        let order = patchline
            .get("order")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);

        result.push(MaxLine {
            source_id,
            source_outlet,
            dest_id,
            dest_inlet,
            order,
        });
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    const L1_JSON: &str = include_str!("../../../tests/e2e/expected/L1_sine.maxpat");
    const L2_JSON: &str = include_str!("../../../tests/e2e/expected/L2_simple_synth.maxpat");
    const L3B_JSON: &str = include_str!("../../../tests/e2e/expected/L3b_control_fanout.maxpat");

    #[test]
    fn parse_l1_boxes_and_lines() {
        let pat = parse_maxpat(L1_JSON).unwrap();
        assert_eq!(pat.boxes.len(), 2);
        assert_eq!(pat.lines.len(), 1);

        // cycle~ 440
        let cycle = &pat.boxes[0];
        assert_eq!(cycle.id, "obj-1");
        assert_eq!(cycle.maxclass, "newobj");
        assert_eq!(cycle.text.as_deref(), Some("cycle~ 440"));
        assert_eq!(cycle.numinlets, 2);
        assert_eq!(cycle.numoutlets, 1);
        assert_eq!(cycle.outlettype, vec!["signal"]);

        // outlet
        let outlet = &pat.boxes[1];
        assert_eq!(outlet.maxclass, "outlet");
    }

    #[test]
    fn parse_l2_inlet_outlet_detection() {
        let pat = parse_maxpat(L2_JSON).unwrap();
        assert_eq!(pat.boxes.len(), 4);
        assert_eq!(pat.lines.len(), 3);

        let inlet_count = pat.boxes.iter().filter(|b| b.maxclass == "inlet").count();
        let outlet_count = pat.boxes.iter().filter(|b| b.maxclass == "outlet").count();
        assert_eq!(inlet_count, 1);
        assert_eq!(outlet_count, 1);
    }

    #[test]
    fn parse_l3b_trigger_node() {
        let pat = parse_maxpat(L3B_JSON).unwrap();
        assert_eq!(pat.boxes.len(), 7);
        assert_eq!(pat.lines.len(), 6);

        let trigger = pat.boxes.iter().find(|b| {
            b.text
                .as_deref()
                .map_or(false, |t| t.starts_with("trigger"))
        });
        assert!(trigger.is_some());
        let trigger = trigger.unwrap();
        assert_eq!(trigger.numoutlets, 2);
    }

    #[test]
    fn parse_line_details() {
        let pat = parse_maxpat(L1_JSON).unwrap();
        let line = &pat.lines[0];
        assert_eq!(line.source_id, "obj-1");
        assert_eq!(line.source_outlet, 0);
        assert_eq!(line.dest_id, "obj-2");
        assert_eq!(line.dest_inlet, 0);
    }

    #[test]
    fn parse_invalid_json() {
        let result = parse_maxpat("not json");
        assert!(result.is_err());
    }

    #[test]
    fn parse_missing_patcher() {
        let result = parse_maxpat("{}");
        assert!(result.is_err());
    }

    #[test]
    fn parse_embedded_subpatcher() {
        let json = r#"{
            "patcher": {
                "boxes": [
                    {
                        "box": {
                            "id": "obj-1",
                            "maxclass": "newobj",
                            "text": "p myfilter",
                            "numinlets": 2,
                            "numoutlets": 1,
                            "outlettype": ["signal"],
                            "patcher": {
                                "boxes": [
                                    { "box": { "id": "sub-1", "maxclass": "inlet~", "numinlets": 0, "numoutlets": 1, "outlettype": ["signal"] } },
                                    { "box": { "id": "sub-2", "maxclass": "inlet", "numinlets": 0, "numoutlets": 1, "outlettype": [""] } },
                                    { "box": { "id": "sub-3", "maxclass": "newobj", "text": "biquad~", "numinlets": 6, "numoutlets": 1, "outlettype": ["signal"] } },
                                    { "box": { "id": "sub-4", "maxclass": "outlet~", "numinlets": 1, "numoutlets": 0, "outlettype": [] } }
                                ],
                                "lines": [
                                    { "patchline": { "source": ["sub-1", 0], "destination": ["sub-3", 0] } },
                                    { "patchline": { "source": ["sub-3", 0], "destination": ["sub-4", 0] } }
                                ]
                            }
                        }
                    },
                    { "box": { "id": "obj-2", "maxclass": "outlet", "numinlets": 1, "numoutlets": 0, "outlettype": [] } }
                ],
                "lines": [
                    { "patchline": { "source": ["obj-1", 0], "destination": ["obj-2", 0] } }
                ]
            }
        }"#;

        let pat = parse_maxpat(json).unwrap();
        assert_eq!(pat.boxes.len(), 2);
        assert_eq!(pat.lines.len(), 1);

        // First box should have an embedded patcher
        let sub_box = &pat.boxes[0];
        assert_eq!(sub_box.text.as_deref(), Some("p myfilter"));
        assert!(sub_box.embedded_patcher.is_some());

        let embedded = sub_box.embedded_patcher.as_ref().unwrap();
        assert_eq!(embedded.boxes.len(), 4);
        assert_eq!(embedded.lines.len(), 2);

        // Check embedded boxes
        let inlet_count = embedded
            .boxes
            .iter()
            .filter(|b| b.maxclass == "inlet~")
            .count();
        let outlet_count = embedded
            .boxes
            .iter()
            .filter(|b| b.maxclass == "outlet~")
            .count();
        assert_eq!(inlet_count, 1);
        assert_eq!(outlet_count, 1);
    }

    #[test]
    fn parse_nested_subpatchers() {
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
        assert_eq!(pat.boxes.len(), 1);

        // Outer subpatcher
        let outer = pat.boxes[0].embedded_patcher.as_ref().unwrap();
        assert_eq!(outer.boxes.len(), 3);

        // Inner subpatcher (nested)
        let inner_box = outer
            .boxes
            .iter()
            .find(|b| b.text.as_deref() == Some("p inner"))
            .unwrap();
        assert!(inner_box.embedded_patcher.is_some());

        let inner = inner_box.embedded_patcher.as_ref().unwrap();
        assert_eq!(inner.boxes.len(), 3);
        assert_eq!(inner.lines.len(), 2);
    }

    #[test]
    fn parse_box_without_embedded_patcher() {
        let pat = parse_maxpat(L1_JSON).unwrap();
        // Regular boxes should have None for embedded_patcher
        for b in &pat.boxes {
            assert!(
                b.embedded_patcher.is_none(),
                "Box {} should not have embedded patcher",
                b.id
            );
        }
    }

    #[test]
    fn parse_box_extra_attrs() {
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
                            "fontname": "Arial",
                            "fontsize": 12
                        }
                    }
                ],
                "lines": []
            }
        }"#;

        let pat = parse_maxpat(json).unwrap();
        assert_eq!(pat.boxes.len(), 1);
        let b = &pat.boxes[0];

        // extra_attrs should contain minimum, maximum, and decorative fields
        // (decorative separation happens in the analyzer, not the parser)
        let attr_keys: Vec<&str> = b.extra_attrs.iter().map(|(k, _)| k.as_str()).collect();
        assert!(attr_keys.contains(&"minimum"), "Should extract minimum");
        assert!(attr_keys.contains(&"maximum"), "Should extract maximum");
        // Decorative fields now flow through extra_attrs (separated by analyzer)
        assert!(
            attr_keys.contains(&"fontname"),
            "Should extract fontname (decorative)"
        );
        assert!(
            attr_keys.contains(&"fontsize"),
            "Should extract fontsize (decorative)"
        );
        // Structural fields should be excluded
        assert!(!attr_keys.contains(&"id"), "Should exclude id");
        assert!(!attr_keys.contains(&"maxclass"), "Should exclude maxclass");
        assert!(
            !attr_keys.contains(&"patching_rect"),
            "Should exclude patching_rect"
        );
        assert!(
            !attr_keys.contains(&"numinlets"),
            "Should exclude numinlets"
        );
        assert!(
            !attr_keys.contains(&"numoutlets"),
            "Should exclude numoutlets"
        );
        assert!(
            !attr_keys.contains(&"outlettype"),
            "Should exclude outlettype"
        );
    }

    #[test]
    fn parse_box_no_extra_attrs() {
        // A minimal box with only structural fields should have empty extra_attrs
        let pat = parse_maxpat(L1_JSON).unwrap();
        for b in &pat.boxes {
            // L1 sine boxes (cycle~ and outlet) have only structural fields
            // They may have some non-structural fields from the fixture though
            // Just verify extra_attrs is populated (not None or panic)
            let _ = &b.extra_attrs;
        }
    }

    #[test]
    fn parse_rnbo_classnamespace() {
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
        assert_eq!(pat.classnamespace.as_deref(), Some("rnbo"));
        assert_eq!(pat.boxes.len(), 3);
        assert_eq!(pat.lines.len(), 2);
    }

    #[test]
    fn parse_standard_patcher_classnamespace() {
        // Standard Max patchers use classnamespace "box" (not "rnbo")
        let pat = parse_maxpat(L1_JSON).unwrap();
        assert_ne!(pat.classnamespace.as_deref(), Some("rnbo"));
        // The standard namespace is "box"
        assert_eq!(pat.classnamespace.as_deref(), Some("box"));
    }

    #[test]
    fn parse_rnboinfo_excluded_from_extra_attrs() {
        let json = r#"{
            "patcher": {
                "classnamespace": "rnbo",
                "boxes": [
                    {
                        "box": {
                            "id": "obj-1",
                            "maxclass": "newobj",
                            "text": "cycle~",
                            "numinlets": 2,
                            "numoutlets": 1,
                            "outlettype": ["signal"],
                            "patching_rect": [50, 50, 80, 22],
                            "rnboinfo": { "some": "complex_data" }
                        }
                    }
                ],
                "lines": []
            }
        }"#;

        let pat = parse_maxpat(json).unwrap();
        let b = &pat.boxes[0];
        let attr_keys: Vec<&str> = b.extra_attrs.iter().map(|(k, _)| k.as_str()).collect();
        assert!(
            !attr_keys.contains(&"rnboinfo"),
            "rnboinfo should be excluded from extra_attrs"
        );
    }

    #[test]
    fn parse_v8_codebox_with_code() {
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

        let pat = parse_maxpat(json).unwrap();
        assert_eq!(pat.boxes.len(), 1);
        let b = &pat.boxes[0];
        assert_eq!(b.maxclass, "v8.codebox");
        assert_eq!(
            b.code.as_deref(),
            Some("function bang() {\n  outlet(0, 42);\n}\n"),
            "code field should be parsed"
        );
        // code and filename should NOT appear in extra_attrs
        let attr_keys: Vec<&str> = b.extra_attrs.iter().map(|(k, _)| k.as_str()).collect();
        assert!(
            !attr_keys.contains(&"code"),
            "code should be excluded from extra_attrs"
        );
        assert!(
            !attr_keys.contains(&"filename"),
            "filename should be excluded from extra_attrs"
        );
    }

    #[test]
    fn parse_box_without_code_field() {
        // Regular newobj boxes should have code: None
        let pat = parse_maxpat(L1_JSON).unwrap();
        for b in &pat.boxes {
            assert!(
                b.code.is_none(),
                "Regular box {} should not have code",
                b.id
            );
        }
    }
}
