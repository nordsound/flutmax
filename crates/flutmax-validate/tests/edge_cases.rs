//! Validation edge case tests.
//!
//! These tests verify that the validator handles various edge cases
//! in .maxpat JSON, including empty patchers, large patchers,
//! unusual object configurations, and malformed data.

use flutmax_validate::{validate_str, Severity};

// ─── Empty and minimal structures ───

#[test]
fn validate_empty_patcher() {
    let json = r#"{"patcher":{"fileversion":1,"boxes":[],"lines":[]}}"#;
    let report = validate_str(json, "empty.maxpat");
    assert!(!report.has_errors());
    assert_eq!(report.boxes_checked, 0);
    assert_eq!(report.lines_checked, 0);
}

#[test]
fn validate_single_box() {
    let json = r#"{
        "patcher": {
            "fileversion": 1,
            "boxes": [
                {
                    "box": {
                        "id": "obj-1",
                        "maxclass": "newobj",
                        "text": "cycle~ 440",
                        "numinlets": 2,
                        "numoutlets": 1,
                        "patching_rect": [100.0, 200.0, 80.0, 22.0]
                    }
                }
            ],
            "lines": []
        }
    }"#;
    let report = validate_str(json, "single.maxpat");
    assert!(!report.has_errors());
    assert_eq!(report.boxes_checked, 1);
}

#[test]
fn validate_single_connection() {
    let json = r#"{
        "patcher": {
            "fileversion": 1,
            "boxes": [
                {
                    "box": {
                        "id": "obj-1",
                        "maxclass": "newobj",
                        "text": "cycle~ 440",
                        "numinlets": 2,
                        "numoutlets": 1,
                        "patching_rect": [100.0, 200.0, 80.0, 22.0]
                    }
                },
                {
                    "box": {
                        "id": "obj-2",
                        "maxclass": "newobj",
                        "text": "dac~",
                        "numinlets": 2,
                        "numoutlets": 0,
                        "patching_rect": [100.0, 300.0, 40.0, 22.0]
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
    let report = validate_str(json, "connection.maxpat");
    assert!(!report.has_errors());
    assert_eq!(report.boxes_checked, 2);
    assert_eq!(report.lines_checked, 1);
}

// ─── Large patchers ───

#[test]
fn validate_50_boxes() {
    let mut boxes_json = Vec::new();
    for i in 0..50 {
        boxes_json.push(format!(
            r#"{{"box":{{"id":"obj-{}","maxclass":"newobj","text":"cycle~ {}","numinlets":2,"numoutlets":1,"patching_rect":[{}.0,{}.0,80.0,22.0]}}}}"#,
            i,
            440 + i,
            100 + (i % 10) * 100,
            100 + (i / 10) * 50
        ));
    }

    let json = format!(
        r#"{{"patcher":{{"fileversion":1,"boxes":[{}],"lines":[]}}}}"#,
        boxes_json.join(",")
    );

    let report = validate_str(&json, "fifty_boxes.maxpat");
    assert!(!report.has_errors());
    assert_eq!(report.boxes_checked, 50);
}

#[test]
fn validate_100_boxes() {
    let mut boxes_json = Vec::new();
    for i in 0..100 {
        boxes_json.push(format!(
            r#"{{"box":{{"id":"obj-{}","maxclass":"newobj","text":"cycle~ {}","numinlets":2,"numoutlets":1,"patching_rect":[{}.0,{}.0,80.0,22.0]}}}}"#,
            i,
            440 + i,
            100 + (i % 10) * 100,
            100 + (i / 10) * 50
        ));
    }

    let json = format!(
        r#"{{"patcher":{{"fileversion":1,"boxes":[{}],"lines":[]}}}}"#,
        boxes_json.join(",")
    );

    let report = validate_str(&json, "hundred_boxes.maxpat");
    assert!(!report.has_errors());
    assert_eq!(report.boxes_checked, 100);
}

#[test]
fn validate_chain_with_many_connections() {
    let count = 20;
    let mut boxes_json = Vec::new();
    let mut lines_json = Vec::new();

    // First box
    boxes_json.push(format!(
        r#"{{"box":{{"id":"obj-0","maxclass":"newobj","text":"cycle~ 440","numinlets":2,"numoutlets":1,"patching_rect":[100.0,100.0,80.0,22.0]}}}}"#
    ));

    // Chain of *~ boxes
    for i in 1..count {
        boxes_json.push(format!(
            r#"{{"box":{{"id":"obj-{}","maxclass":"newobj","text":"*~ 0.99","numinlets":2,"numoutlets":1,"patching_rect":[100.0,{}.0,80.0,22.0]}}}}"#,
            i,
            100 + i * 30
        ));
        lines_json.push(format!(
            r#"{{"patchline":{{"source":["obj-{}",0],"destination":["obj-{}",0]}}}}"#,
            i - 1,
            i
        ));
    }

    let json = format!(
        r#"{{"patcher":{{"fileversion":1,"boxes":[{}],"lines":[{}]}}}}"#,
        boxes_json.join(","),
        lines_json.join(",")
    );

    let report = validate_str(&json, "chain.maxpat");
    assert!(!report.has_errors());
    assert_eq!(report.boxes_checked, count);
    assert_eq!(report.lines_checked, count - 1);
}

// ─── Object types ───

#[test]
fn validate_inlet_outlet_boxes() {
    let json = r#"{
        "patcher": {
            "fileversion": 1,
            "boxes": [
                {
                    "box": {
                        "id": "obj-1",
                        "maxclass": "inlet",
                        "numinlets": 0,
                        "numoutlets": 1,
                        "patching_rect": [100.0, 50.0, 30.0, 30.0]
                    }
                },
                {
                    "box": {
                        "id": "obj-2",
                        "maxclass": "outlet",
                        "numinlets": 1,
                        "numoutlets": 0,
                        "patching_rect": [100.0, 250.0, 30.0, 30.0]
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
    let report = validate_str(json, "inlet_outlet.maxpat");
    assert!(!report.has_errors());
}

#[test]
fn validate_button_box() {
    let json = r#"{
        "patcher": {
            "fileversion": 1,
            "boxes": [
                {
                    "box": {
                        "id": "obj-1",
                        "maxclass": "button",
                        "numinlets": 1,
                        "numoutlets": 1,
                        "patching_rect": [100.0, 100.0, 20.0, 20.0]
                    }
                }
            ],
            "lines": []
        }
    }"#;
    let report = validate_str(json, "button.maxpat");
    assert!(!report.has_errors());
}

#[test]
fn validate_toggle_box() {
    let json = r#"{
        "patcher": {
            "fileversion": 1,
            "boxes": [
                {
                    "box": {
                        "id": "obj-1",
                        "maxclass": "toggle",
                        "numinlets": 1,
                        "numoutlets": 1,
                        "patching_rect": [100.0, 100.0, 20.0, 20.0]
                    }
                }
            ],
            "lines": []
        }
    }"#;
    let report = validate_str(json, "toggle.maxpat");
    assert!(!report.has_errors());
}

// ─── Invalid JSON input ───

#[test]
fn validate_invalid_json() {
    let report = validate_str("{not valid json", "bad.maxpat");
    assert!(report.has_errors());
    assert_eq!(report.errors[0].layer, "json");
}

#[test]
fn validate_empty_string() {
    let report = validate_str("", "empty.maxpat");
    assert!(report.has_errors());
    assert_eq!(report.errors[0].layer, "json");
}

#[test]
fn validate_null_json() {
    let report = validate_str("null", "null.maxpat");
    assert!(report.has_errors());
}

#[test]
fn validate_array_json() {
    let report = validate_str("[]", "array.maxpat");
    assert!(report.has_errors());
}

#[test]
fn validate_string_json() {
    let report = validate_str(r#""hello""#, "string.maxpat");
    assert!(report.has_errors());
}

#[test]
fn validate_number_json() {
    let report = validate_str("42", "number.maxpat");
    assert!(report.has_errors());
}

// ─── Missing required fields ───

#[test]
fn validate_missing_patcher() {
    let json = r#"{"other": "data"}"#;
    let report = validate_str(json, "no_patcher.maxpat");
    assert!(report.has_errors());
}

#[test]
fn validate_missing_fileversion() {
    let json = r#"{"patcher":{"boxes":[],"lines":[]}}"#;
    let report = validate_str(json, "no_fileversion.maxpat");
    assert!(report.has_errors());
}

#[test]
fn validate_missing_box_id() {
    let json = r#"{
        "patcher": {
            "fileversion": 1,
            "boxes": [
                {
                    "box": {
                        "maxclass": "newobj",
                        "text": "cycle~ 440",
                        "numinlets": 2,
                        "numoutlets": 1,
                        "patching_rect": [100.0, 200.0, 80.0, 22.0]
                    }
                }
            ],
            "lines": []
        }
    }"#;
    let report = validate_str(json, "no_box_id.maxpat");
    assert!(report.has_errors());
}

#[test]
fn validate_missing_maxclass() {
    let json = r#"{
        "patcher": {
            "fileversion": 1,
            "boxes": [
                {
                    "box": {
                        "id": "obj-1",
                        "text": "cycle~ 440",
                        "numinlets": 2,
                        "numoutlets": 1,
                        "patching_rect": [100.0, 200.0, 80.0, 22.0]
                    }
                }
            ],
            "lines": []
        }
    }"#;
    let report = validate_str(json, "no_maxclass.maxpat");
    assert!(report.has_errors());
}

// ─── Static analysis ───

#[test]
fn validate_known_object_correct_ports() {
    let json = r#"{
        "patcher": {
            "fileversion": 1,
            "boxes": [
                {
                    "box": {
                        "id": "obj-1",
                        "maxclass": "newobj",
                        "text": "cycle~ 440",
                        "numinlets": 2,
                        "numoutlets": 1,
                        "patching_rect": [100.0, 200.0, 80.0, 22.0]
                    }
                }
            ],
            "lines": []
        }
    }"#;
    let report = validate_str(json, "known_obj.maxpat");
    assert!(!report.has_errors());
    assert_eq!(report.warning_count(), 0);
}

#[test]
fn validate_unknown_object_is_warning_not_error() {
    let json = r#"{
        "patcher": {
            "fileversion": 1,
            "boxes": [
                {
                    "box": {
                        "id": "obj-1",
                        "maxclass": "newobj",
                        "text": "my_custom_abstraction",
                        "numinlets": 1,
                        "numoutlets": 1,
                        "patching_rect": [100.0, 200.0, 80.0, 22.0]
                    }
                }
            ],
            "lines": []
        }
    }"#;
    let report = validate_str(json, "unknown.maxpat");
    // Unknown objects should be warnings, not errors
    assert!(!report.has_errors());
    assert!(report.warning_count() > 0);
}

#[test]
fn validate_wrong_inlet_count() {
    let json = r#"{
        "patcher": {
            "fileversion": 1,
            "boxes": [
                {
                    "box": {
                        "id": "obj-1",
                        "maxclass": "newobj",
                        "text": "cycle~ 440",
                        "numinlets": 5,
                        "numoutlets": 1,
                        "patching_rect": [100.0, 200.0, 80.0, 22.0]
                    }
                }
            ],
            "lines": []
        }
    }"#;
    let report = validate_str(json, "wrong_inlets.maxpat");
    // Should detect inlet count mismatch for cycle~ (expected 2, got 5)
    let has_inlet_error = report
        .errors
        .iter()
        .any(|e| e.severity == Severity::Error && e.message.contains("inlet"));
    assert!(
        has_inlet_error,
        "should detect inlet count mismatch, got: {:?}",
        report.errors
    );
}

#[test]
fn validate_wrong_outlet_count() {
    let json = r#"{
        "patcher": {
            "fileversion": 1,
            "boxes": [
                {
                    "box": {
                        "id": "obj-1",
                        "maxclass": "newobj",
                        "text": "cycle~ 440",
                        "numinlets": 2,
                        "numoutlets": 3,
                        "patching_rect": [100.0, 200.0, 80.0, 22.0]
                    }
                }
            ],
            "lines": []
        }
    }"#;
    let report = validate_str(json, "wrong_outlets.maxpat");
    // Should detect outlet count mismatch for cycle~ (expected 1, got 3)
    let has_outlet_error = report
        .errors
        .iter()
        .any(|e| e.severity == Severity::Error && e.message.contains("outlet"));
    assert!(
        has_outlet_error,
        "should detect outlet count mismatch, got: {:?}",
        report.errors
    );
}

// ─── Report metadata ───

#[test]
fn report_file_name_preserved() {
    let json = r#"{"patcher":{"fileversion":1,"boxes":[],"lines":[]}}"#;
    let report = validate_str(json, "my_patch.maxpat");
    assert_eq!(report.file, "my_patch.maxpat");
}

#[test]
fn report_display_contains_filename() {
    let json = r#"{"patcher":{"fileversion":1,"boxes":[],"lines":[]}}"#;
    let report = validate_str(json, "test_display.maxpat");
    let display = format!("{}", report);
    assert!(display.contains("test_display.maxpat"));
}

#[test]
fn report_error_count_accuracy() {
    let json = r#"{
        "patcher": {
            "boxes": [
                {"box": {"maxclass": "newobj", "numinlets": 1, "numoutlets": 1, "patching_rect": [0,0,20,20]}},
                {"box": {"id": "obj-2", "numinlets": 1, "numoutlets": 1, "patching_rect": [0,0,20,20]}}
            ],
            "lines": []
        }
    }"#;
    let report = validate_str(json, "multi_error.maxpat");
    assert!(report.has_errors());
    assert!(
        report.error_count() >= 2,
        "expected at least 2 errors, got {}",
        report.error_count()
    );
}

// ─── All builtin objects pass validation ───

#[test]
fn validate_all_signal_objects() {
    let signal_objects = vec![
        ("cycle~", 2, 1),
        ("*~", 2, 1),
        ("+~", 2, 1),
        ("-~", 2, 1),
        ("/~", 2, 1),
        ("biquad~", 6, 1),
        ("line~", 2, 2),
        ("noise~", 1, 1),
        ("phasor~", 2, 1),
        ("dac~", 2, 0),
        ("ezdac~", 2, 0),
        ("adc~", 0, 2),
    ];

    for (name, inlets, outlets) in signal_objects {
        let json = format!(
            r#"{{"patcher":{{"fileversion":1,"boxes":[{{"box":{{"id":"obj-1","maxclass":"newobj","text":"{}","numinlets":{},"numoutlets":{},"patching_rect":[100.0,200.0,80.0,22.0]}}}}],"lines":[]}}}}"#,
            name, inlets, outlets
        );
        let report = validate_str(&json, &format!("{}.maxpat", name));
        assert!(
            !report.has_errors(),
            "signal object '{}' should validate without errors. Errors: {:?}",
            name,
            report.errors
        );
    }
}

#[test]
fn validate_all_control_objects() {
    let control_objects = vec![
        ("*", 2, 1),
        ("+", 2, 1),
        ("-", 2, 1),
        ("/", 2, 1),
        ("%", 2, 1),
        ("loadbang", 1, 1),
        ("button", 1, 1),
        ("print", 1, 0),
    ];

    for (name, inlets, outlets) in control_objects {
        let json = format!(
            r#"{{"patcher":{{"fileversion":1,"boxes":[{{"box":{{"id":"obj-1","maxclass":"newobj","text":"{}","numinlets":{},"numoutlets":{},"patching_rect":[100.0,200.0,80.0,22.0]}}}}],"lines":[]}}}}"#,
            name, inlets, outlets
        );
        let report = validate_str(&json, &format!("{}.maxpat", name));
        assert!(
            !report.has_errors(),
            "control object '{}' should validate without errors. Errors: {:?}",
            name,
            report.errors
        );
    }
}

// ─── Mixed valid/invalid boxes ───

#[test]
fn validate_mix_valid_and_invalid_boxes() {
    let json = r#"{
        "patcher": {
            "fileversion": 1,
            "boxes": [
                {
                    "box": {
                        "id": "obj-1",
                        "maxclass": "newobj",
                        "text": "cycle~ 440",
                        "numinlets": 2,
                        "numoutlets": 1,
                        "patching_rect": [100.0, 200.0, 80.0, 22.0]
                    }
                },
                {
                    "box": {
                        "id": "obj-2",
                        "maxclass": "newobj",
                        "text": "cycle~ 440",
                        "numinlets": 99,
                        "numoutlets": 1,
                        "patching_rect": [200.0, 200.0, 80.0, 22.0]
                    }
                }
            ],
            "lines": []
        }
    }"#;
    let report = validate_str(json, "mixed.maxpat");
    assert!(report.has_errors());
    assert_eq!(report.boxes_checked, 2);
}

// Note: Tests that compile .flutmax and then validate the output
// are in crates/flutmax-cli/tests/compile_validate.rs to avoid
// circular dev-dependencies (flutmax-validate -> flutmax-cli -> flutmax-validate).
