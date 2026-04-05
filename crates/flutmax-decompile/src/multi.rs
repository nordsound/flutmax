//! Multi-file decompilation for .maxpat files containing subpatchers.
//!
//! When a .maxpat contains `[p name]` subpatchers, `decompile_multi()` extracts
//! each embedded patcher into a separate .flutmax file, replacing the `[p name]`
//! object with a reference to the extracted abstraction.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::parser::DecompileError;

/// Result of a multi-file decompilation.
///
/// Contains a map of filename -> source content, plus the name of the main file.
#[derive(Debug)]
pub struct DecompileResult {
    /// Map of filename (e.g., "main.flutmax", "sub_delay.flutmax") to source content.
    pub files: HashMap<String, String>,
    /// Code files extracted from codebox objects (filename -> code content).
    /// v8.codebox produces `.js` files, gen~ codebox produces `.genexpr` files.
    pub code_files: HashMap<String, String>,
    /// Filenames of RNBO subpatchers (classnamespace: "rnbo").
    pub rnbo_patchers: HashSet<String>,
    /// Filenames of gen~ subpatchers (classnamespace: "dsp.gen").
    pub gen_patchers: HashSet<String>,
    /// UI sidecar files: "name.uiflutmax" -> JSON content with positions + decorative attrs.
    pub ui_files: HashMap<String, String>,
    /// The filename of the main/top-level file within `files`.
    pub main_file: String,
}

/// Decompile a .maxpat JSON string into potentially multiple .flutmax files.
///
/// `main_name` is the base name (without extension) for the top-level patcher.
///
/// If the patch contains subpatchers (`[p name]`), each is extracted and
/// decompiled as a separate file. The main patcher replaces `[p name]` with
/// a reference to the extracted abstraction.
///
/// If the patch has no subpatchers, this returns a single-file result equivalent
/// to calling `decompile()`.
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
    let root: Value =
        serde_json::from_str(json_str).map_err(|e| DecompileError::JsonParse(e.to_string()))?;

    let patcher = root
        .get("patcher")
        .ok_or_else(|| DecompileError::MissingField("patcher".into()))?;

    let boxes = patcher
        .get("boxes")
        .and_then(|b| b.as_array())
        .ok_or_else(|| DecompileError::MissingField("patcher.boxes".into()))?;

    // Collect subpatchers: find boxes with embedded "patcher" field
    let mut subpatchers: Vec<(String, Value)> = Vec::new(); // (name, patcher_json)
    let mut sub_name_counts: HashMap<String, usize> = HashMap::new();
    let mut rnbo_patchers: HashSet<String> = HashSet::new();
    let mut gen_patchers: HashSet<String> = HashSet::new();

    for box_wrapper in boxes {
        let b = &box_wrapper["box"];

        // Check for embedded subpatcher
        if let Some(embedded_patcher) = b.get("patcher") {
            // Check classnamespace for special patcher types
            let classnamespace = embedded_patcher
                .get("classnamespace")
                .and_then(|v| v.as_str())
                .unwrap_or("box");

            // Extract subpatcher name from the text field (e.g., "p delay" -> "delay")
            let sub_name = if let Some(text) = b.get("text").and_then(|t| t.as_str()) {
                let parts: Vec<&str> = text.split_whitespace().collect();
                if parts.len() >= 2 && (parts[0] == "p" || parts[0] == "patcher") {
                    parts[1].to_string()
                } else if parts.len() == 1 && parts[0] == "p" {
                    // Anonymous subpatcher
                    "sub".to_string()
                } else {
                    // Use the first word as a prefix
                    format!("sub_{}", parts[0])
                }
            } else {
                "sub".to_string()
            };

            // Disambiguate duplicate names
            let count = sub_name_counts.entry(sub_name.clone()).or_insert(0);
            let unique_name = if *count == 0 {
                sub_name.clone()
            } else {
                format!("{}_{}", sub_name, count)
            };
            *count += 1;

            // Track RNBO patchers
            if classnamespace == "rnbo" {
                rnbo_patchers.insert(format!("{}.flutmax", unique_name));
            }

            // Track gen~ patchers
            if classnamespace == "dsp.gen" {
                gen_patchers.insert(format!("{}.flutmax", unique_name));
            }

            // Build a minimal top-level .maxpat wrapper for the embedded patcher
            let sub_maxpat = serde_json::json!({
                "patcher": embedded_patcher,
            });

            subpatchers.push((unique_name, sub_maxpat));
        }
    }

    let mut files = HashMap::new();
    let mut ui_files = HashMap::new();

    // Collect code files and UI data from codebox objects via analyze()
    let mut code_files = HashMap::new();
    if let Ok(maxpat) = crate::parser::parse_maxpat(json_str) {
        if let Ok(patch) = crate::analyzer::analyze(&maxpat, objdb) {
            // Generate .uiflutmax for the main patch (before consuming code_files)
            if let Some(ui_content) = crate::emitter::emit_ui_file(&patch) {
                ui_files.insert(format!("{}.uiflutmax", main_name), ui_content);
            }
            for (filename, content) in patch.code_files {
                code_files.insert(filename, content);
            }
        }
    }

    if subpatchers.is_empty() {
        // No subpatchers — single file decompile
        let source = crate::emitter::decompile_with_objdb(json_str, objdb)?;
        let main_file = format!("{}.flutmax", main_name);
        files.insert(main_file.clone(), source);
        return Ok(DecompileResult {
            files,
            code_files,
            rnbo_patchers,
            gen_patchers,
            ui_files,
            main_file,
        });
    }

    // Decompile each subpatcher
    for (sub_name, sub_json) in &subpatchers {
        let sub_json_str = serde_json::to_string(sub_json)
            .map_err(|e| DecompileError::JsonParse(format!("serialize sub: {}", e)))?;

        // Collect code files and UI data from subpatchers
        if let Ok(sub_maxpat) = crate::parser::parse_maxpat(&sub_json_str) {
            if let Ok(sub_patch) = crate::analyzer::analyze(&sub_maxpat, objdb) {
                // Generate .uiflutmax for the subpatcher (before consuming code_files)
                if let Some(ui_content) = crate::emitter::emit_ui_file(&sub_patch) {
                    ui_files.insert(format!("{}.uiflutmax", sub_name), ui_content);
                }
                for (filename, content) in sub_patch.code_files {
                    code_files.insert(filename, content);
                }
            }
        }

        match crate::emitter::decompile_with_objdb(&sub_json_str, objdb) {
            Ok(source) => {
                let filename = format!("{}.flutmax", sub_name);
                files.insert(filename, source);
            }
            Err(_) => {
                // If a subpatcher fails to decompile, skip it (the main patcher
                // will still reference it but compilation may fail later).
            }
        }
    }

    // Decompile the main patcher (the top-level, which still contains
    // the [p name] objects — they will appear as regular objects in the
    // decompiled output).
    let main_source = crate::emitter::decompile_with_objdb(json_str, objdb)?;
    let main_file = format!("{}.flutmax", main_name);
    files.insert(main_file.clone(), main_source);

    Ok(DecompileResult {
        files,
        code_files,
        rnbo_patchers,
        gen_patchers,
        ui_files,
        main_file,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decompile_multi_flat_patch() {
        // A flat patch with no subpatchers should produce a single file
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
                            "maxclass": "outlet~",
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

        let result = decompile_multi(json, "test").unwrap();
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.main_file, "test.flutmax");
        assert!(result.files.contains_key("test.flutmax"));
    }

    #[test]
    fn decompile_multi_with_subpatcher() {
        // A patch with an embedded subpatcher [p delay]
        let json = r#"{
            "patcher": {
                "boxes": [
                    {
                        "box": {
                            "id": "obj-1",
                            "maxclass": "newobj",
                            "text": "p delay",
                            "numinlets": 1,
                            "numoutlets": 1,
                            "outlettype": [""],
                            "patcher": {
                                "boxes": [
                                    {
                                        "box": {
                                            "id": "sub-1",
                                            "maxclass": "inlet",
                                            "numinlets": 0,
                                            "numoutlets": 1,
                                            "outlettype": [""]
                                        }
                                    },
                                    {
                                        "box": {
                                            "id": "sub-2",
                                            "maxclass": "newobj",
                                            "text": "delay 500",
                                            "numinlets": 2,
                                            "numoutlets": 1,
                                            "outlettype": ["bang"]
                                        }
                                    },
                                    {
                                        "box": {
                                            "id": "sub-3",
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
                                            "source": ["sub-1", 0],
                                            "destination": ["sub-2", 0]
                                        }
                                    },
                                    {
                                        "patchline": {
                                            "source": ["sub-2", 0],
                                            "destination": ["sub-3", 0]
                                        }
                                    }
                                ]
                            }
                        }
                    },
                    {
                        "box": {
                            "id": "obj-2",
                            "maxclass": "inlet",
                            "numinlets": 0,
                            "numoutlets": 1,
                            "outlettype": [""]
                        }
                    },
                    {
                        "box": {
                            "id": "obj-3",
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
                            "source": ["obj-2", 0],
                            "destination": ["obj-1", 0]
                        }
                    },
                    {
                        "patchline": {
                            "source": ["obj-1", 0],
                            "destination": ["obj-3", 0]
                        }
                    }
                ]
            }
        }"#;

        let result = decompile_multi(json, "main").unwrap();
        // Should produce at least 2 files: main.flutmax + delay.flutmax
        assert!(
            result.files.len() >= 2,
            "expected >= 2 files, got {}",
            result.files.len()
        );
        assert_eq!(result.main_file, "main.flutmax");
        assert!(result.files.contains_key("main.flutmax"));
        assert!(result.files.contains_key("delay.flutmax"));
    }

    #[test]
    fn decompile_result_main_file_present() {
        let json = r#"{
            "patcher": {
                "boxes": [],
                "lines": []
            }
        }"#;

        let result = decompile_multi(json, "empty").unwrap();
        assert!(result.files.contains_key(&result.main_file));
    }

    #[test]
    fn decompile_multi_tracks_gen_patchers() {
        // A patch with an embedded gen~ subpatcher
        let json = r#"{
            "patcher": {
                "boxes": [
                    {
                        "box": {
                            "id": "obj-1",
                            "maxclass": "newobj",
                            "text": "gen~ mygen",
                            "numinlets": 1,
                            "numoutlets": 1,
                            "outlettype": ["signal"],
                            "patcher": {
                                "classnamespace": "dsp.gen",
                                "boxes": [
                                    {
                                        "box": {
                                            "id": "gen-1",
                                            "maxclass": "newobj",
                                            "text": "in 1",
                                            "numinlets": 0,
                                            "numoutlets": 1,
                                            "outlettype": [""]
                                        }
                                    },
                                    {
                                        "box": {
                                            "id": "gen-2",
                                            "maxclass": "newobj",
                                            "text": "out 1",
                                            "numinlets": 1,
                                            "numoutlets": 0,
                                            "outlettype": []
                                        }
                                    }
                                ],
                                "lines": [
                                    {
                                        "patchline": {
                                            "source": ["gen-1", 0],
                                            "destination": ["gen-2", 0]
                                        }
                                    }
                                ]
                            }
                        }
                    },
                    {
                        "box": {
                            "id": "obj-2",
                            "maxclass": "inlet~",
                            "numinlets": 0,
                            "numoutlets": 1,
                            "outlettype": ["signal"]
                        }
                    },
                    {
                        "box": {
                            "id": "obj-3",
                            "maxclass": "outlet~",
                            "numinlets": 1,
                            "numoutlets": 0,
                            "outlettype": []
                        }
                    }
                ],
                "lines": [
                    {
                        "patchline": {
                            "source": ["obj-2", 0],
                            "destination": ["obj-1", 0]
                        }
                    },
                    {
                        "patchline": {
                            "source": ["obj-1", 0],
                            "destination": ["obj-3", 0]
                        }
                    }
                ]
            }
        }"#;

        let result = decompile_multi(json, "main").unwrap();
        // Should track the gen~ subpatcher
        assert!(
            !result.gen_patchers.is_empty(),
            "gen_patchers should not be empty"
        );
        // Should have a file for the gen~ subpatcher
        let gen_file = result.gen_patchers.iter().next().unwrap();
        assert!(
            result.files.contains_key(gen_file),
            "gen~ file should exist in files map"
        );
    }

    #[test]
    fn decompile_multi_produces_ui_files() {
        let json = r#"{
            "patcher": {
                "rect": [100, 100, 800, 600],
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

        let result = decompile_multi(json, "synth").unwrap();
        // Should have a .uiflutmax file
        assert!(
            result.ui_files.contains_key("synth.uiflutmax"),
            "Should produce synth.uiflutmax, got keys: {:?}",
            result.ui_files.keys().collect::<Vec<_>>()
        );

        // Parse and verify the UI file content
        let ui_content = &result.ui_files["synth.uiflutmax"];
        let parsed: serde_json::Value = serde_json::from_str(ui_content).unwrap();
        assert!(
            parsed["_patcher"]["rect"].is_array(),
            "Should have _patcher.rect"
        );
        assert_eq!(parsed["_patcher"]["rect"][0], 100.0);
    }
}
