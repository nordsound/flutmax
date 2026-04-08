//! RNBO integration tests (E62).
//!
//! Compile RNBO multi-file fixtures through the CLI binary and validate
//! the generated .maxpat JSON for structural correctness.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

/// Compile an RNBO fixture directory using the CLI binary.
/// Returns the output directory path containing the generated .maxpat files.
fn compile_rnbo_fixture(
    fixture_name: &str,
) -> (tempfile::TempDir, HashMap<String, serde_json::Value>) {
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rnbo")
        .join(fixture_name);
    let output_dir = tempfile::tempdir().expect("failed to create temp dir");

    let status = Command::new(env!("CARGO_BIN_EXE_flutmax"))
        .args([
            "compile",
            fixture_dir.to_str().unwrap(),
            "-o",
            output_dir.path().to_str().unwrap(),
        ])
        .status()
        .expect("failed to run flutmax");
    assert!(
        status.success(),
        "flutmax compile failed for {}",
        fixture_name
    );

    // Read all generated .maxpat files
    let mut results = HashMap::new();
    for entry in std::fs::read_dir(output_dir.path()).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("maxpat") {
            let stem = path.file_stem().unwrap().to_string_lossy().to_string();
            let content = std::fs::read_to_string(&path).unwrap();
            let json: serde_json::Value = serde_json::from_str(&content)
                .unwrap_or_else(|e| panic!("invalid JSON in {}: {}", path.display(), e));
            results.insert(stem, json);
        }
    }

    (output_dir, results)
}

// ─── Validation helpers ─────────────────────────────────────────────

/// Find all boxes in a patcher matching a predicate on the box text.
fn find_boxes<'a>(
    patcher: &'a serde_json::Value,
    predicate: impl Fn(&str) -> bool,
) -> Vec<&'a serde_json::Value> {
    match patcher["boxes"].as_array() {
        Some(boxes) => boxes
            .iter()
            .filter_map(|b| {
                let box_obj = &b["box"];
                let text = box_obj["text"].as_str().unwrap_or("");
                if predicate(text) {
                    Some(box_obj)
                } else {
                    None
                }
            })
            .collect(),
        None => vec![],
    }
}

/// Find the first rnbo~ box in a top-level patcher.
/// Matches "rnbo~" or "rnbo~ @attr ..." (with preserved attributes).
fn find_rnbo_box(json: &serde_json::Value) -> Option<&serde_json::Value> {
    find_boxes(&json["patcher"], |t| {
        t == "rnbo~" || t.starts_with("rnbo~ @")
    })
    .into_iter()
    .next()
}

/// Validate that all patchcords are within range (source outlet < numoutlets,
/// destination inlet < numinlets). Recurse into subpatchers.
fn validate_patchcords(patcher: &serde_json::Value, path: &str) -> Vec<String> {
    let mut errors = Vec::new();

    let empty_arr = vec![];

    // Build id -> box mapping
    let box_arr = patcher["boxes"].as_array().unwrap_or(&empty_arr);
    let boxes: HashMap<String, &serde_json::Value> = box_arr
        .iter()
        .filter_map(|b| {
            let id = b["box"]["id"].as_str()?;
            Some((id.to_string(), &b["box"]))
        })
        .collect();

    // Check each patchline
    let lines_arr = patcher["lines"].as_array().unwrap_or(&empty_arr);
    for line in lines_arr {
        let src_id = line["patchline"]["source"][0].as_str().unwrap_or("");
        let src_out = line["patchline"]["source"][1].as_u64().unwrap_or(0);
        let dst_id = line["patchline"]["destination"][0].as_str().unwrap_or("");
        let dst_in = line["patchline"]["destination"][1].as_u64().unwrap_or(0);

        if let Some(src) = boxes.get(src_id) {
            let numout = src["numoutlets"].as_u64().unwrap_or(0);
            if src_out >= numout {
                let text = src["text"].as_str().unwrap_or("?");
                errors.push(format!(
                    "{}: outlet out of range: [{}] out[{}] >= numoutlets {}",
                    path, text, src_out, numout
                ));
            }
        }
        if let Some(dst) = boxes.get(dst_id) {
            let numin = dst["numinlets"].as_u64().unwrap_or(0);
            if dst_in >= numin {
                let text = dst["text"].as_str().unwrap_or("?");
                errors.push(format!(
                    "{}: inlet out of range: [{}] in[{}] >= numinlets {}",
                    path, text, dst_in, numin
                ));
            }
        }
    }

    // Recurse into subpatchers
    for b in box_arr {
        let box_obj = &b["box"];
        let text = box_obj["text"].as_str().unwrap_or("?");
        if let Some(sub_patcher) = box_obj.get("patcher") {
            if sub_patcher.is_object() {
                errors.extend(validate_patchcords(
                    sub_patcher,
                    &format!("{} > {}", path, text),
                ));
            }
        }
    }

    errors
}

/// Validate the RNBO-specific structure of a top-level patch.
/// Returns a list of error descriptions (empty = pass).
fn validate_rnbo_structure(json: &serde_json::Value, path: &str) -> Vec<String> {
    let mut errors = Vec::new();
    let patcher = &json["patcher"];
    let empty = vec![];

    // Find rnbo~ boxes and validate
    let top_boxes = patcher["boxes"].as_array().unwrap_or(&empty);
    for box_entry in top_boxes {
        let box_obj = &box_entry["box"];
        let text = box_obj["text"].as_str().unwrap_or("");

        if text == "rnbo~" || text.starts_with("rnbo~ @") {
            // classnamespace of embedded patcher must be "rnbo"
            if let Some(sub_patcher) = box_obj.get("patcher") {
                if sub_patcher["classnamespace"].as_str() != Some("rnbo") {
                    errors.push(format!(
                        "{}: rnbo~ embedded patcher classnamespace != 'rnbo'",
                        path
                    ));
                }
            } else {
                errors.push(format!("{}: rnbo~ missing embedded patcher", path));
            }

            // saved_object_attributes
            if let Some(attrs) = box_obj.get("saved_object_attributes") {
                if attrs.get("parameter_enable").is_none() {
                    errors.push(format!("{}: rnbo~ missing parameter_enable", path));
                }
            } else {
                errors.push(format!("{}: rnbo~ missing saved_object_attributes", path));
            }

            // autosave
            if box_obj.get("autosave").is_none() {
                errors.push(format!("{}: rnbo~ missing autosave", path));
            }

            // inletInfo / outletInfo
            if box_obj.get("inletInfo").is_none() {
                errors.push(format!("{}: rnbo~ missing inletInfo", path));
            }
            if box_obj.get("outletInfo").is_none() {
                errors.push(format!("{}: rnbo~ missing outletInfo", path));
            }

            // numinlets >= 2 (inlet 0: param/msg, inlet 1: MIDI)
            if box_obj["numinlets"].as_u64().unwrap_or(0) < 2 {
                errors.push(format!(
                    "{}: rnbo~ numinlets < 2 (MIDI needs inlet 1)",
                    path
                ));
            }

            // Check out~ numbering (1-based) inside RNBO patcher
            if let Some(sub_patcher) = box_obj.get("patcher") {
                let out_boxes = find_boxes(sub_patcher, |t| t.starts_with("out~ "));
                for ob in &out_boxes {
                    let t = ob["text"].as_str().unwrap_or("");
                    if let Some(num_str) = t.strip_prefix("out~ ") {
                        if let Ok(num) = num_str.parse::<u64>() {
                            if num == 0 {
                                errors.push(format!(
                                    "{}: out~ uses 0-based index '{}' (should be 1-based)",
                                    path, t
                                ));
                            }
                        }
                    }
                }

                // Recursive patchcord validation
                errors.extend(validate_patchcords(
                    sub_patcher,
                    &format!("{} > rnbo~", path),
                ));

                // gen~ embedding checks
                let sub_boxes = sub_patcher["boxes"].as_array().unwrap_or(&empty);
                for sub_box in sub_boxes {
                    let sub = &sub_box["box"];
                    let sub_text = sub["text"].as_str().unwrap_or("");
                    if sub_text.starts_with("gen~ @title") {
                        // rnbo_classname
                        if sub.get("rnbo_classname").is_none() {
                            errors.push(format!("{} > {}: missing rnbo_classname", path, sub_text));
                        }
                        // rnbo_serial
                        if sub.get("rnbo_serial").is_none() {
                            errors.push(format!("{} > {}: missing rnbo_serial", path, sub_text));
                        }
                        // rnbo_uniqueid
                        if sub.get("rnbo_uniqueid").is_none() {
                            errors.push(format!("{} > {}: missing rnbo_uniqueid", path, sub_text));
                        }
                        // Embedded gen~ patcher
                        if let Some(gen_p) = sub.get("patcher") {
                            if gen_p["classnamespace"].as_str() != Some("dsp.gen") {
                                errors.push(format!(
                                    "{} > {}: classnamespace != 'dsp.gen'",
                                    path, sub_text
                                ));
                            }
                            // trigger must not be present in gen~
                            let gen_boxes = gen_p["boxes"].as_array().unwrap_or(&empty);
                            for gen_box in gen_boxes {
                                let gt = gen_box["box"]["text"].as_str().unwrap_or("");
                                if gt.starts_with("trigger ") || gt == "t" {
                                    errors.push(format!(
                                        "{} > {} > {}: trigger found in gen~",
                                        path, sub_text, gt
                                    ));
                                }
                            }
                            // Recursive patchcord check
                            errors.extend(validate_patchcords(
                                gen_p,
                                &format!("{} > {}", path, sub_text),
                            ));
                        } else {
                            errors.push(format!(
                                "{} > {}: gen~ not embedded (missing patcher)",
                                path, sub_text
                            ));
                        }
                    }
                }
            }
        }
    }

    // Top-level patchcord check
    errors.extend(validate_patchcords(patcher, path));

    errors
}

// ─── Tests ──────────────────────────────────────────────────────────

#[test]
fn test_rnbo_sine_compile() {
    let (_dir, results) = compile_rnbo_fixture("sine_test");

    // sine_main.maxpat must exist
    let main_json = results
        .get("sine_main")
        .expect("sine_main.maxpat not generated");

    // Structural validation
    let errors = validate_rnbo_structure(main_json, "sine_main");
    assert!(
        errors.is_empty(),
        "RNBO structure errors:\n{}",
        errors.join("\n")
    );

    // Find rnbo~ box
    let rnbo_box = find_rnbo_box(main_json).expect("rnbo~ box not found in sine_main");

    // classnamespace of embedded patcher
    assert_eq!(
        rnbo_box["patcher"]["classnamespace"].as_str(),
        Some("rnbo"),
        "embedded patcher classnamespace should be 'rnbo'"
    );

    // parameter_enable in saved_object_attributes
    assert_eq!(
        rnbo_box["saved_object_attributes"]["parameter_enable"].as_u64(),
        Some(1),
        "parameter_enable should be 1"
    );

    // autosave
    assert_eq!(
        rnbo_box["autosave"].as_u64(),
        Some(1),
        "autosave should be 1"
    );

    // inletInfo and outletInfo
    assert!(rnbo_box.get("inletInfo").is_some(), "missing inletInfo");
    assert!(rnbo_box.get("outletInfo").is_some(), "missing outletInfo");

    // numinlets >= 2
    assert!(
        rnbo_box["numinlets"].as_u64().unwrap_or(0) >= 2,
        "rnbo~ numinlets should be >= 2"
    );

    // out~ text should be 1-based ("out~ 1", "out~ 2")
    let rnbo_patcher = &rnbo_box["patcher"];
    let out_boxes = find_boxes(rnbo_patcher, |t| t.starts_with("out~ "));
    assert_eq!(out_boxes.len(), 2, "should have 2 out~ boxes");
    let mut out_texts: Vec<&str> = out_boxes
        .iter()
        .map(|b| b["text"].as_str().unwrap())
        .collect();
    out_texts.sort();
    assert_eq!(out_texts, vec!["out~ 1", "out~ 2"]);
}

#[test]
fn test_rnbo_midi_compile() {
    let (_dir, results) = compile_rnbo_fixture("midi_test");

    let main_json = results
        .get("midi_main")
        .expect("midi_main.maxpat not generated");

    // Structural validation
    let errors = validate_rnbo_structure(main_json, "midi_main");
    assert!(
        errors.is_empty(),
        "RNBO structure errors:\n{}",
        errors.join("\n")
    );

    // Find rnbo~ box
    let rnbo_box = find_rnbo_box(main_json).expect("rnbo~ box not found in midi_main");

    // midiin -> rnbo~ inlet 1 connection
    let patcher = &main_json["patcher"];
    let lines = patcher["lines"].as_array().expect("missing lines");

    // Build id -> text mapping
    let id_to_text: HashMap<String, String> = patcher["boxes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|b| {
            let id = b["box"]["id"].as_str()?.to_string();
            let text = b["box"]["text"].as_str().unwrap_or("").to_string();
            Some((id, text))
        })
        .collect();

    // Find the midiin -> rnbo~ connection and verify it targets inlet 1
    let midi_to_rnbo = lines.iter().find(|line| {
        let src_id = line["patchline"]["source"][0].as_str().unwrap_or("");
        let dst_id = line["patchline"]["destination"][0].as_str().unwrap_or("");
        id_to_text.get(src_id).map(|s| s.as_str()) == Some("midiin")
            && id_to_text.get(dst_id).map(|s| s.as_str()) == Some("rnbo~")
    });
    assert!(
        midi_to_rnbo.is_some(),
        "no midiin -> rnbo~ connection found"
    );
    let midi_line = midi_to_rnbo.unwrap();
    assert_eq!(
        midi_line["patchline"]["destination"][1].as_u64(),
        Some(1),
        "midiin should connect to rnbo~ inlet 1 (MIDI)"
    );

    // notein inside RNBO patcher should have numoutlets >= 3
    let rnbo_patcher = &rnbo_box["patcher"];
    let notein_boxes = find_boxes(rnbo_patcher, |t| t == "notein");
    assert!(!notein_boxes.is_empty(), "notein not found in RNBO patcher");
    for notein in &notein_boxes {
        assert!(
            notein["numoutlets"].as_u64().unwrap_or(0) >= 3,
            "notein should have >= 3 outlets"
        );
    }
}

#[test]
fn test_rnbo_gen_embed_compile() {
    let (_dir, results) = compile_rnbo_fixture("gen_test");

    let main_json = results
        .get("gen_main")
        .expect("gen_main.maxpat not generated");

    // Structural validation
    let errors = validate_rnbo_structure(main_json, "gen_main");
    assert!(
        errors.is_empty(),
        "RNBO structure errors:\n{}",
        errors.join("\n")
    );

    // Find rnbo~ box
    let rnbo_box = find_rnbo_box(main_json).expect("rnbo~ box not found in gen_main");
    let rnbo_patcher = &rnbo_box["patcher"];

    // Find gen~ box inside RNBO patcher
    let gen_boxes = find_boxes(rnbo_patcher, |t| t.starts_with("gen~ @title"));
    assert_eq!(gen_boxes.len(), 1, "should have exactly 1 gen~ box in RNBO");
    let gen_box = gen_boxes[0];

    // gen~ should have rnbo_classname
    assert_eq!(
        gen_box["rnbo_classname"].as_str(),
        Some("gen~"),
        "gen~ should have rnbo_classname = 'gen~'"
    );

    // gen~ should have rnbo_serial
    assert!(
        gen_box.get("rnbo_serial").is_some(),
        "gen~ should have rnbo_serial"
    );

    // gen~ should have rnbo_uniqueid
    assert!(
        gen_box.get("rnbo_uniqueid").is_some(),
        "gen~ should have rnbo_uniqueid"
    );

    // gen~ should have an embedded patcher
    let gen_patcher = gen_box
        .get("patcher")
        .expect("gen~ should have an embedded patcher");

    // classnamespace should be "dsp.gen"
    assert_eq!(
        gen_patcher["classnamespace"].as_str(),
        Some("dsp.gen"),
        "gen~ patcher classnamespace should be 'dsp.gen'"
    );

    // No trigger objects in gen~
    let gen_box_list = gen_patcher["boxes"]
        .as_array()
        .expect("gen~ should have boxes");
    for gb in gen_box_list {
        let text = gb["box"]["text"].as_str().unwrap_or("");
        assert!(
            !text.starts_with("trigger ") && text != "t",
            "gen~ should not contain trigger objects, found: '{}'",
            text
        );
    }

    // gen~ patchcords should be valid
    let patchcord_errors = validate_patchcords(gen_patcher, "gen_main > rnbo~ > gen~");
    assert!(
        patchcord_errors.is_empty(),
        "gen~ patchcord errors:\n{}",
        patchcord_errors.join("\n")
    );
}

#[test]
fn test_rnbo_polyphony_attr() {
    let (_dir, results) = compile_rnbo_fixture("poly_test");

    let main_json = results
        .get("poly_main")
        .expect("poly_main.maxpat not generated");

    // Structural validation
    let errors = validate_rnbo_structure(main_json, "poly_main");
    assert!(
        errors.is_empty(),
        "RNBO structure errors:\n{}",
        errors.join("\n")
    );

    // Find rnbo~ box (should match "rnbo~ @polyphony 4")
    let rnbo_box = find_rnbo_box(main_json).expect("rnbo~ box not found in poly_main");

    // text should preserve @polyphony attribute
    let text = rnbo_box["text"].as_str().unwrap_or("");
    assert!(
        text.contains("@polyphony 4"),
        "rnbo~ text should contain '@polyphony 4', got: '{}'",
        text
    );

    // Embedded patcher should still be present
    assert!(
        rnbo_box.get("patcher").is_some(),
        "rnbo~ should have embedded patcher despite @polyphony attr"
    );

    // classnamespace should be "rnbo"
    assert_eq!(
        rnbo_box["patcher"]["classnamespace"].as_str(),
        Some("rnbo"),
        "embedded patcher classnamespace should be 'rnbo'"
    );

    // Standard RNBO attributes should still be present
    assert_eq!(
        rnbo_box["saved_object_attributes"]["parameter_enable"].as_u64(),
        Some(1),
    );
    assert_eq!(rnbo_box["autosave"].as_u64(), Some(1));
    assert!(rnbo_box.get("inletInfo").is_some());
    assert!(rnbo_box.get("outletInfo").is_some());
    assert!(rnbo_box["numinlets"].as_u64().unwrap_or(0) >= 2);
}
