use serde_json::Value;
use std::collections::HashSet;

/// A structural validation error with a JSON path indicating where the problem was found.
#[derive(Debug, Clone)]
pub struct StructureError {
    pub message: String,
    /// JSON path like "patcher.boxes[2].box.id"
    pub path: String,
}

impl std::fmt::Display for StructureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

/// Validate the structure of a parsed .maxpat JSON value.
///
/// Checks that the JSON conforms to the expected .maxpat schema:
/// required keys, correct types, unique IDs, valid references, etc.
pub fn validate_structure(json: &Value) -> Vec<StructureError> {
    let mut errors = Vec::new();

    // Check root `patcher` key
    let patcher = match json.get("patcher") {
        Some(p) => p,
        None => {
            errors.push(StructureError {
                message: "Missing required key 'patcher'".to_string(),
                path: "(root)".to_string(),
            });
            return errors;
        }
    };

    // Check patcher.fileversion
    match patcher.get("fileversion") {
        Some(v) if v.is_number() => {}
        Some(_) => {
            errors.push(StructureError {
                message: "'fileversion' must be a number".to_string(),
                path: "patcher.fileversion".to_string(),
            });
        }
        None => {
            errors.push(StructureError {
                message: "Missing required key 'fileversion'".to_string(),
                path: "patcher".to_string(),
            });
        }
    }

    // Collect box IDs for cross-referencing with patchlines
    let mut box_ids: HashSet<String> = HashSet::new();

    // Check patcher.boxes
    match patcher.get("boxes") {
        Some(boxes) if boxes.is_array() => {
            let boxes_arr = boxes.as_array().unwrap();
            for (i, box_entry) in boxes_arr.iter().enumerate() {
                validate_box(box_entry, i, &mut box_ids, &mut errors);
            }
        }
        Some(_) => {
            errors.push(StructureError {
                message: "'boxes' must be an array".to_string(),
                path: "patcher.boxes".to_string(),
            });
        }
        None => {
            errors.push(StructureError {
                message: "Missing required key 'boxes'".to_string(),
                path: "patcher".to_string(),
            });
        }
    }

    // Check patcher.lines
    match patcher.get("lines") {
        Some(lines) if lines.is_array() => {
            let lines_arr = lines.as_array().unwrap();
            for (i, line_entry) in lines_arr.iter().enumerate() {
                validate_line(line_entry, i, &box_ids, &mut errors);
            }
        }
        Some(_) => {
            errors.push(StructureError {
                message: "'lines' must be an array".to_string(),
                path: "patcher.lines".to_string(),
            });
        }
        None => {
            errors.push(StructureError {
                message: "Missing required key 'lines'".to_string(),
                path: "patcher".to_string(),
            });
        }
    }

    errors
}

/// Validate a single box entry. Each box entry should be `{"box": {...}}`.
fn validate_box(
    box_entry: &Value,
    index: usize,
    box_ids: &mut HashSet<String>,
    errors: &mut Vec<StructureError>,
) {
    let entry_path = format!("patcher.boxes[{}]", index);

    let inner = match box_entry.get("box") {
        Some(b) => b,
        None => {
            errors.push(StructureError {
                message: "Missing 'box' wrapper".to_string(),
                path: entry_path,
            });
            return;
        }
    };

    let box_path = format!("{}.box", entry_path);

    // Required: id (string)
    match inner.get("id") {
        Some(id) if id.is_string() => {
            let id_str = id.as_str().unwrap().to_string();
            if !box_ids.insert(id_str.clone()) {
                errors.push(StructureError {
                    message: format!("Duplicate box id '{}'", id_str),
                    path: format!("{}.id", box_path),
                });
            }
        }
        Some(_) => {
            errors.push(StructureError {
                message: "'id' must be a string".to_string(),
                path: format!("{}.id", box_path),
            });
        }
        None => {
            errors.push(StructureError {
                message: "Missing required field 'id'".to_string(),
                path: box_path.clone(),
            });
        }
    }

    // Required: maxclass (string)
    let maxclass = match inner.get("maxclass") {
        Some(mc) if mc.is_string() => Some(mc.as_str().unwrap()),
        Some(_) => {
            errors.push(StructureError {
                message: "'maxclass' must be a string".to_string(),
                path: format!("{}.maxclass", box_path),
            });
            None
        }
        None => {
            errors.push(StructureError {
                message: "Missing required field 'maxclass'".to_string(),
                path: box_path.clone(),
            });
            None
        }
    };

    // Required: numinlets (int)
    match inner.get("numinlets") {
        Some(v) if v.is_i64() || v.is_u64() => {}
        Some(_) => {
            errors.push(StructureError {
                message: "'numinlets' must be an integer".to_string(),
                path: format!("{}.numinlets", box_path),
            });
        }
        None => {
            errors.push(StructureError {
                message: "Missing required field 'numinlets'".to_string(),
                path: box_path.clone(),
            });
        }
    }

    // Required: numoutlets (int)
    match inner.get("numoutlets") {
        Some(v) if v.is_i64() || v.is_u64() => {}
        Some(_) => {
            errors.push(StructureError {
                message: "'numoutlets' must be an integer".to_string(),
                path: format!("{}.numoutlets", box_path),
            });
        }
        None => {
            errors.push(StructureError {
                message: "Missing required field 'numoutlets'".to_string(),
                path: box_path.clone(),
            });
        }
    }

    // Required: patching_rect (array of 4 numbers)
    match inner.get("patching_rect") {
        Some(rect) if rect.is_array() => {
            let arr = rect.as_array().unwrap();
            if arr.len() != 4 {
                errors.push(StructureError {
                    message: format!(
                        "'patching_rect' must have exactly 4 elements, found {}",
                        arr.len()
                    ),
                    path: format!("{}.patching_rect", box_path),
                });
            } else {
                for (j, elem) in arr.iter().enumerate() {
                    if !elem.is_number() {
                        errors.push(StructureError {
                            message: format!("'patching_rect[{}]' must be a number", j),
                            path: format!("{}.patching_rect[{}]", box_path, j),
                        });
                    }
                }
            }
        }
        Some(_) => {
            errors.push(StructureError {
                message: "'patching_rect' must be an array".to_string(),
                path: format!("{}.patching_rect", box_path),
            });
        }
        None => {
            errors.push(StructureError {
                message: "Missing required field 'patching_rect'".to_string(),
                path: box_path.clone(),
            });
        }
    }

    // Conditional: newobj boxes must have 'text' field
    if maxclass == Some("newobj") {
        match inner.get("text") {
            Some(t) if t.is_string() => {}
            Some(_) => {
                errors.push(StructureError {
                    message: "'text' must be a string for newobj boxes".to_string(),
                    path: format!("{}.text", box_path),
                });
            }
            None => {
                errors.push(StructureError {
                    message: "newobj box missing required field 'text'".to_string(),
                    path: box_path,
                });
            }
        }
    }
}

/// Validate a single line (patchline) entry.
fn validate_line(
    line_entry: &Value,
    index: usize,
    box_ids: &HashSet<String>,
    errors: &mut Vec<StructureError>,
) {
    let entry_path = format!("patcher.lines[{}]", index);

    let patchline = match line_entry.get("patchline") {
        Some(pl) => pl,
        None => {
            errors.push(StructureError {
                message: "Missing 'patchline' wrapper".to_string(),
                path: entry_path,
            });
            return;
        }
    };

    let pl_path = format!("{}.patchline", entry_path);

    // Validate source: [string, int]
    validate_endpoint(patchline, "source", &pl_path, box_ids, errors);

    // Validate destination: [string, int]
    validate_endpoint(patchline, "destination", &pl_path, box_ids, errors);
}

/// Validate a patchline endpoint (source or destination).
/// Expected format: [string_id, integer_outlet_or_inlet_index]
fn validate_endpoint(
    patchline: &Value,
    field_name: &str,
    pl_path: &str,
    box_ids: &HashSet<String>,
    errors: &mut Vec<StructureError>,
) {
    let field_path = format!("{}.{}", pl_path, field_name);

    match patchline.get(field_name) {
        Some(endpoint) if endpoint.is_array() => {
            let arr = endpoint.as_array().unwrap();
            if arr.len() != 2 {
                errors.push(StructureError {
                    message: format!(
                        "'{}' must be an array of [string, int], found {} elements",
                        field_name,
                        arr.len()
                    ),
                    path: field_path,
                });
                return;
            }

            // First element: box ID (string)
            match arr[0].as_str() {
                Some(id) => {
                    if !box_ids.contains(id) {
                        errors.push(StructureError {
                            message: format!(
                                "{} references non-existent box id '{}'",
                                field_name, id
                            ),
                            path: format!("{}[0]", field_path),
                        });
                    }
                }
                None => {
                    errors.push(StructureError {
                        message: format!("'{}[0]' must be a string (box id)", field_name),
                        path: format!("{}[0]", field_path),
                    });
                }
            }

            // Second element: port index (int)
            if !arr[1].is_i64() && !arr[1].is_u64() {
                errors.push(StructureError {
                    message: format!("'{}[1]' must be an integer (port index)", field_name),
                    path: format!("{}[1]", field_path),
                });
            }
        }
        Some(_) => {
            errors.push(StructureError {
                message: format!("'{}' must be an array", field_name),
                path: field_path,
            });
        }
        None => {
            errors.push(StructureError {
                message: format!("Missing required field '{}'", field_name),
                path: pl_path.to_string(),
            });
        }
    }
}

/// Return the number of boxes in the JSON (for reporting).
pub fn count_boxes(json: &Value) -> usize {
    json.get("patcher")
        .and_then(|p| p.get("boxes"))
        .and_then(|b| b.as_array())
        .map(|a| a.len())
        .unwrap_or(0)
}

/// Return the number of lines in the JSON (for reporting).
pub fn count_lines(json: &Value) -> usize {
    json.get("patcher")
        .and_then(|p| p.get("lines"))
        .and_then(|l| l.as_array())
        .map(|a| a.len())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Helper: build a minimal valid .maxpat JSON structure.
    fn minimal_valid_maxpat() -> Value {
        json!({
            "patcher": {
                "fileversion": 1,
                "appversion": {"major": 8, "minor": 6},
                "boxes": [],
                "lines": []
            }
        })
    }

    /// Helper: build a valid box.
    fn valid_box(id: &str, maxclass: &str) -> Value {
        let mut box_obj = json!({
            "id": id,
            "maxclass": maxclass,
            "numinlets": 1,
            "numoutlets": 1,
            "patching_rect": [100.0, 200.0, 60.0, 22.0]
        });
        if maxclass == "newobj" {
            box_obj
                .as_object_mut()
                .unwrap()
                .insert("text".to_string(), json!("cycle~ 440"));
        }
        json!({"box": box_obj})
    }

    /// Helper: build a valid patchline.
    fn valid_patchline(src_id: &str, src_outlet: i64, dst_id: &str, dst_inlet: i64) -> Value {
        json!({
            "patchline": {
                "source": [src_id, src_outlet],
                "destination": [dst_id, dst_inlet]
            }
        })
    }

    #[test]
    fn valid_maxpat_no_errors() {
        let json = minimal_valid_maxpat();
        let errors = validate_structure(&json);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn missing_patcher_root() {
        let json = json!({"something": "else"});
        let errors = validate_structure(&json);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("patcher"));
        assert_eq!(errors[0].path, "(root)");
    }

    #[test]
    fn missing_boxes() {
        let json = json!({
            "patcher": {
                "fileversion": 1,
                "lines": []
            }
        });
        let errors = validate_structure(&json);
        let boxes_err = errors.iter().find(|e| e.message.contains("boxes"));
        assert!(boxes_err.is_some(), "Expected error about missing 'boxes'");
    }

    #[test]
    fn missing_lines() {
        let json = json!({
            "patcher": {
                "fileversion": 1,
                "boxes": []
            }
        });
        let errors = validate_structure(&json);
        let lines_err = errors.iter().find(|e| e.message.contains("lines"));
        assert!(lines_err.is_some(), "Expected error about missing 'lines'");
    }

    #[test]
    fn missing_fileversion() {
        let json = json!({
            "patcher": {
                "boxes": [],
                "lines": []
            }
        });
        let errors = validate_structure(&json);
        let fv_err = errors.iter().find(|e| e.message.contains("fileversion"));
        assert!(
            fv_err.is_some(),
            "Expected error about missing 'fileversion'"
        );
    }

    #[test]
    fn box_missing_id() {
        let json = json!({
            "patcher": {
                "fileversion": 1,
                "boxes": [
                    {"box": {
                        "maxclass": "button",
                        "numinlets": 1,
                        "numoutlets": 1,
                        "patching_rect": [0.0, 0.0, 20.0, 20.0]
                    }}
                ],
                "lines": []
            }
        });
        let errors = validate_structure(&json);
        let id_err = errors.iter().find(|e| e.message.contains("'id'"));
        assert!(id_err.is_some(), "Expected error about missing 'id'");
    }

    #[test]
    fn box_missing_maxclass() {
        let json = json!({
            "patcher": {
                "fileversion": 1,
                "boxes": [
                    {"box": {
                        "id": "obj-1",
                        "numinlets": 1,
                        "numoutlets": 1,
                        "patching_rect": [0.0, 0.0, 20.0, 20.0]
                    }}
                ],
                "lines": []
            }
        });
        let errors = validate_structure(&json);
        let mc_err = errors.iter().find(|e| e.message.contains("'maxclass'"));
        assert!(mc_err.is_some(), "Expected error about missing 'maxclass'");
    }

    #[test]
    fn duplicate_box_ids() {
        let json = json!({
            "patcher": {
                "fileversion": 1,
                "boxes": [
                    valid_box("obj-1", "button"),
                    valid_box("obj-1", "toggle")
                ],
                "lines": []
            }
        });
        let errors = validate_structure(&json);
        let dup_err = errors.iter().find(|e| e.message.contains("Duplicate"));
        assert!(dup_err.is_some(), "Expected error about duplicate id");
    }

    #[test]
    fn newobj_without_text() {
        let json = json!({
            "patcher": {
                "fileversion": 1,
                "boxes": [
                    {"box": {
                        "id": "obj-1",
                        "maxclass": "newobj",
                        "numinlets": 1,
                        "numoutlets": 1,
                        "patching_rect": [0.0, 0.0, 60.0, 22.0]
                    }}
                ],
                "lines": []
            }
        });
        let errors = validate_structure(&json);
        let text_err = errors
            .iter()
            .find(|e| e.message.contains("text") || e.message.contains("newobj"));
        assert!(
            text_err.is_some(),
            "Expected error about newobj missing 'text', got: {:?}",
            errors
        );
    }

    #[test]
    fn patchline_referencing_nonexistent_box() {
        let json = json!({
            "patcher": {
                "fileversion": 1,
                "boxes": [
                    valid_box("obj-1", "button"),
                    valid_box("obj-2", "toggle")
                ],
                "lines": [
                    valid_patchline("obj-1", 0, "obj-99", 0)
                ]
            }
        });
        let errors = validate_structure(&json);
        let ref_err = errors.iter().find(|e| e.message.contains("non-existent"));
        assert!(
            ref_err.is_some(),
            "Expected error about non-existent box id"
        );
    }

    #[test]
    fn valid_patcher_with_boxes_and_lines() {
        let json = json!({
            "patcher": {
                "fileversion": 1,
                "boxes": [
                    valid_box("obj-1", "newobj"),
                    valid_box("obj-2", "newobj")
                ],
                "lines": [
                    valid_patchline("obj-1", 0, "obj-2", 0)
                ]
            }
        });
        let errors = validate_structure(&json);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn valid_empty_patcher() {
        let json = json!({
            "patcher": {
                "fileversion": 1,
                "boxes": [],
                "lines": []
            }
        });
        let errors = validate_structure(&json);
        assert!(errors.is_empty(), "Expected no errors for empty patcher");
    }

    #[test]
    fn box_missing_wrapper() {
        let json = json!({
            "patcher": {
                "fileversion": 1,
                "boxes": [
                    {"id": "obj-1", "maxclass": "button"}
                ],
                "lines": []
            }
        });
        let errors = validate_structure(&json);
        let wrap_err = errors.iter().find(|e| e.message.contains("wrapper"));
        assert!(
            wrap_err.is_some(),
            "Expected error about missing 'box' wrapper"
        );
    }

    #[test]
    fn patchline_missing_wrapper() {
        let json = json!({
            "patcher": {
                "fileversion": 1,
                "boxes": [
                    valid_box("obj-1", "button"),
                    valid_box("obj-2", "toggle")
                ],
                "lines": [
                    {"source": ["obj-1", 0], "destination": ["obj-2", 0]}
                ]
            }
        });
        let errors = validate_structure(&json);
        let wrap_err = errors
            .iter()
            .find(|e| e.message.contains("patchline") && e.message.contains("wrapper"));
        assert!(
            wrap_err.is_some(),
            "Expected error about missing 'patchline' wrapper"
        );
    }

    #[test]
    fn patching_rect_wrong_element_count() {
        let json = json!({
            "patcher": {
                "fileversion": 1,
                "boxes": [
                    {"box": {
                        "id": "obj-1",
                        "maxclass": "button",
                        "numinlets": 1,
                        "numoutlets": 1,
                        "patching_rect": [0.0, 0.0]
                    }}
                ],
                "lines": []
            }
        });
        let errors = validate_structure(&json);
        let rect_err = errors
            .iter()
            .find(|e| e.message.contains("patching_rect") && e.message.contains("4 elements"));
        assert!(
            rect_err.is_some(),
            "Expected error about patching_rect length"
        );
    }

    #[test]
    fn patching_rect_non_numeric_element() {
        let json = json!({
            "patcher": {
                "fileversion": 1,
                "boxes": [
                    {"box": {
                        "id": "obj-1",
                        "maxclass": "button",
                        "numinlets": 1,
                        "numoutlets": 1,
                        "patching_rect": [0.0, "bad", 60.0, 22.0]
                    }}
                ],
                "lines": []
            }
        });
        let errors = validate_structure(&json);
        let rect_err = errors
            .iter()
            .find(|e| e.message.contains("patching_rect") && e.message.contains("number"));
        assert!(
            rect_err.is_some(),
            "Expected error about non-numeric patching_rect element"
        );
    }

    #[test]
    fn patchline_source_references_nonexistent_box() {
        let json = json!({
            "patcher": {
                "fileversion": 1,
                "boxes": [
                    valid_box("obj-1", "button")
                ],
                "lines": [
                    valid_patchline("obj-missing", 0, "obj-1", 0)
                ]
            }
        });
        let errors = validate_structure(&json);
        let src_err = errors
            .iter()
            .find(|e| e.message.contains("non-existent") && e.message.contains("obj-missing"));
        assert!(
            src_err.is_some(),
            "Expected error about source referencing non-existent box"
        );
    }

    #[test]
    fn count_boxes_and_lines() {
        let json = json!({
            "patcher": {
                "fileversion": 1,
                "boxes": [
                    valid_box("obj-1", "button"),
                    valid_box("obj-2", "toggle")
                ],
                "lines": [
                    valid_patchline("obj-1", 0, "obj-2", 0)
                ]
            }
        });
        assert_eq!(count_boxes(&json), 2);
        assert_eq!(count_lines(&json), 1);
    }

    #[test]
    fn count_boxes_missing_patcher() {
        let json = json!({"foo": "bar"});
        assert_eq!(count_boxes(&json), 0);
        assert_eq!(count_lines(&json), 0);
    }
}
