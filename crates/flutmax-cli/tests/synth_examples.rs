//! Integration tests for complex synthesizer examples.
//!
//! Each test compiles a .flutmax source from `examples/synths/` through the
//! full pipeline and validates the resulting .maxpat JSON.

use std::path::PathBuf;

/// Return the workspace root directory.
fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .expect("no parent of crates/flutmax-cli")
        .parent()
        .expect("no grandparent")
        .to_path_buf()
}

/// Read a synth example .flutmax file.
fn read_synth_example(name: &str) -> String {
    let path = workspace_root().join("examples/synths").join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read synth example {}: {}", path.display(), e))
}

/// Read a multi-file synth example .flutmax file.
fn read_multi_file_example(name: &str) -> String {
    let path = workspace_root()
        .join("examples/synths/multi_file_synth")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "failed to read multi-file example {}: {}",
            path.display(),
            e
        )
    })
}

/// Compile a .flutmax source string, validate the output, and return the JSON.
///
/// Filters out known false positives (signal->outlet connections).
fn compile_and_validate(source: &str, label: &str) -> String {
    // Step 1: Compile
    let json = flutmax_cli::compile(source)
        .unwrap_or_else(|e| panic!("compilation failed for {}: {}", label, e));

    // Step 2: Validate JSON structure
    let report = flutmax_validate::validate_str(&json, &format!("{}.maxpat", label));

    // Filter out known false positives: signal->outlet connections
    let real_errors: Vec<_> = report
        .errors
        .iter()
        .filter(|e| e.severity == flutmax_validate::Severity::Error)
        .filter(|e| {
            if e.layer == "static" && e.message.contains("Signal outlet") {
                if let Some(dest_start) = e.message.rfind("of '") {
                    let dest_name = &e.message[dest_start + 4..e.message.len() - 1];
                    if dest_name.starts_with("obj-") {
                        return false;
                    }
                }
            }
            true
        })
        .collect();

    assert!(
        real_errors.is_empty(),
        "Validation errors for {}:\n{}",
        label,
        report
    );

    // Step 3: Basic structural checks
    let parsed: serde_json::Value =
        serde_json::from_str(&json).expect("output should be valid JSON");
    assert!(
        parsed.get("patcher").is_some(),
        "missing 'patcher' key for {}",
        label
    );
    let boxes = parsed["patcher"]["boxes"]
        .as_array()
        .expect("missing boxes array");
    assert!(!boxes.is_empty(), "boxes should not be empty for {}", label);

    json
}

// ─── Single-file synthesizer examples ───

#[test]
fn compile_fm_synth() {
    let source = read_synth_example("fm_synth.flutmax");
    let json = compile_and_validate(&source, "fm_synth");

    // FM synth should have: 4 inlets + cycle~ x2 + mul x2 + mul~ x2 + outlet~ = 11 boxes minimum
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let boxes = parsed["patcher"]["boxes"].as_array().unwrap();
    assert!(
        boxes.len() >= 10,
        "FM synth should have at least 10 boxes, got {}",
        boxes.len()
    );
}

#[test]
fn compile_subtractive_synth() {
    let source = read_synth_example("subtractive_synth.flutmax");
    let json = compile_and_validate(&source, "subtractive_synth");

    // Should have phasor~ objects
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let boxes = parsed["patcher"]["boxes"].as_array().unwrap();
    let has_phasor = boxes.iter().any(|b| {
        b["box"]["text"]
            .as_str()
            .map(|t| t.starts_with("phasor~"))
            .unwrap_or(false)
    });
    assert!(
        has_phasor,
        "subtractive synth should contain phasor~ objects"
    );
}

#[test]
fn compile_delay_effect() {
    let source = read_synth_example("delay_effect.flutmax");
    let json = compile_and_validate(&source, "delay_effect");

    // Should have tapin~ and tapout~ (feedback loop)
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let boxes = parsed["patcher"]["boxes"].as_array().unwrap();
    let has_tapin = boxes.iter().any(|b| {
        b["box"]["text"]
            .as_str()
            .map(|t| t.starts_with("tapin~"))
            .unwrap_or(false)
    });
    let has_tapout = boxes.iter().any(|b| {
        b["box"]["text"]
            .as_str()
            .map(|t| t.starts_with("tapout~"))
            .unwrap_or(false)
    });
    assert!(has_tapin, "delay effect should contain tapin~");
    assert!(has_tapout, "delay effect should contain tapout~");

    // Should have 2 outlets (stereo)
    let outlet_count = boxes
        .iter()
        .filter(|b| {
            let mc = b["box"]["maxclass"].as_str().unwrap_or("");
            mc == "outlet" || mc == "outlet~"
        })
        .count();
    assert_eq!(
        outlet_count, 2,
        "delay effect should have 2 outlets for stereo output"
    );
}

#[test]
fn compile_granular_simple() {
    let source = read_synth_example("granular_simple.flutmax");
    let json = compile_and_validate(&source, "granular_simple");

    // Should have 4 cycle~ objects
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let boxes = parsed["patcher"]["boxes"].as_array().unwrap();
    let cycle_count = boxes
        .iter()
        .filter(|b| {
            b["box"]["text"]
                .as_str()
                .map(|t| t.starts_with("cycle~"))
                .unwrap_or(false)
        })
        .count();
    assert_eq!(
        cycle_count, 4,
        "granular simple should have 4 cycle~ objects, got {}",
        cycle_count
    );
}

// ─── Multi-file abstraction example ───

#[test]
fn compile_multi_file_oscillator() {
    let source = read_multi_file_example("oscillator.flutmax");
    compile_and_validate(&source, "multi_oscillator");
}

#[test]
fn compile_multi_file_mixer_2ch() {
    let source = read_multi_file_example("mixer_2ch.flutmax");
    compile_and_validate(&source, "multi_mixer_2ch");
}

#[test]
fn compile_multi_file_main_synth_with_registry() {
    use flutmax_sema::registry::AbstractionRegistry;

    // Read all files
    let osc_source = read_multi_file_example("oscillator.flutmax");
    let mixer_source = read_multi_file_example("mixer_2ch.flutmax");
    let main_source = read_multi_file_example("main_synth.flutmax");

    // Parse all
    let osc_ast = flutmax_parser::parse(&osc_source).expect("oscillator should parse");
    let mixer_ast = flutmax_parser::parse(&mixer_source).expect("mixer_2ch should parse");
    let main_ast = flutmax_parser::parse(&main_source).expect("main_synth should parse");

    // Build registry
    let mut registry = AbstractionRegistry::new();
    registry.register("oscillator", &osc_ast);
    registry.register("mixer_2ch", &mixer_ast);
    registry.register("main_synth", &main_ast);

    // Compile main_synth with registry
    let json = flutmax_cli::compile_with_registry(&main_source, Some(&registry))
        .expect("main_synth should compile with registry");

    // Validate
    let report = flutmax_validate::validate_str(&json, "main_synth.maxpat");
    let real_errors: Vec<_> = report
        .errors
        .iter()
        .filter(|e| e.severity == flutmax_validate::Severity::Error)
        .filter(|e| {
            if e.layer == "static" && e.message.contains("Signal outlet") {
                if let Some(dest_start) = e.message.rfind("of '") {
                    let dest_name = &e.message[dest_start + 4..e.message.len() - 1];
                    if dest_name.starts_with("obj-") {
                        return false;
                    }
                }
            }
            true
        })
        .collect();
    assert!(
        real_errors.is_empty(),
        "Validation errors for main_synth:\n{}",
        report
    );

    // Check structure
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let boxes = parsed["patcher"]["boxes"].as_array().unwrap();

    // main_synth should contain oscillator and mixer_2ch abstraction references
    let has_oscillator = boxes.iter().any(|b| {
        b["box"]["text"]
            .as_str()
            .map(|t| t.starts_with("oscillator"))
            .unwrap_or(false)
    });
    let has_mixer = boxes.iter().any(|b| {
        b["box"]["text"]
            .as_str()
            .map(|t| t.starts_with("mixer_2ch"))
            .unwrap_or(false)
    });

    assert!(
        has_oscillator,
        "main_synth should reference oscillator abstraction"
    );
    assert!(
        has_mixer,
        "main_synth should reference mixer_2ch abstraction"
    );

    // Should have 2 oscillator instances
    let osc_count = boxes
        .iter()
        .filter(|b| {
            b["box"]["text"]
                .as_str()
                .map(|t| t == "oscillator")
                .unwrap_or(false)
        })
        .count();
    assert_eq!(
        osc_count, 2,
        "main_synth should have 2 oscillator instances, got {}",
        osc_count
    );
}

// ─── Maintainability metrics ───

#[test]
fn verify_code_reduction_ratio() {
    // All synth examples should produce significantly more .maxpat JSON
    // than their .flutmax source (at least 10x)
    let examples = [
        "fm_synth.flutmax",
        "subtractive_synth.flutmax",
        "delay_effect.flutmax",
        "granular_simple.flutmax",
    ];

    for name in &examples {
        let source = read_synth_example(name);
        let json = flutmax_cli::compile(&source)
            .unwrap_or_else(|e| panic!("compilation failed for {}: {}", name, e));

        let source_lines = source.lines().count();
        let json_lines = json.lines().count();
        let ratio = json_lines / source_lines;

        assert!(
            ratio >= 10,
            "{}: expected at least 10x code reduction, got {}x ({} -> {} lines)",
            name,
            ratio,
            source_lines,
            json_lines
        );
    }
}
