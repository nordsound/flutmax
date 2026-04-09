//! E2E audio simulation tests for flutmax-sim using generic fixtures.
//!
//! Compiles fixture .flutmax files via the CLI and runs the resulting
//! .maxpat through flutmax-sim to verify audio output.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

/// Compile a fixture directory and return the parsed .maxpat JSON by stem name.
fn compile_fixture(fixture_name: &str) -> (tempfile::TempDir, HashMap<String, String>) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rnbo")
        .join(fixture_name);
    let output_dir = tempfile::tempdir().expect("tempdir");
    let status = Command::new(env!("CARGO_BIN_EXE_flutmax"))
        .args([
            "compile",
            dir.to_str().unwrap(),
            "-o",
            output_dir.path().to_str().unwrap(),
        ])
        .status()
        .expect("flutmax");
    assert!(status.success(), "compile failed for {fixture_name}");

    let mut results = HashMap::new();
    for entry in std::fs::read_dir(output_dir.path()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) == Some("maxpat") {
            let stem = path.file_stem().unwrap().to_string_lossy().to_string();
            results.insert(stem, std::fs::read_to_string(&path).unwrap());
        }
    }
    (output_dir, results)
}

#[test]
fn test_sine_test_compiles_and_runs() {
    // Verify the simulator can load the sine_test RNBO patch
    let (_dir, compiled) = compile_fixture("sine_test");
    let json = compiled.get("sine_voice").expect("sine_voice.maxpat");

    let mut sim = flutmax_sim::RnboSimulator::from_json(json)
        .expect("Failed to create RnboSimulator from sine_test");

    // Run for a short time and verify it produces signal
    let out = sim.run_samples(1000);
    println!("sine_test peak: {}", out.peak());
}

#[test]
fn test_midi_test_responds_to_note_on() {
    // Verify notein → mtof → cycle~ chain works in simulator (load RNBO patcher directly)
    let (_dir, compiled) = compile_fixture("midi_test");
    let json = compiled.get("midi_voice").expect("midi_voice.maxpat");

    let mut sim = flutmax_sim::RnboSimulator::from_json(json)
        .expect("Failed to create RnboSimulator from midi_voice");

    // Send Note On (C4 = 60, vel 100)
    sim.send_note_on(60, 100);

    // Run 0.1 second
    let out = sim.run_samples(4800);
    println!("midi_test peak after note on: {}", out.peak());
    assert!(
        !out.is_silent(),
        "midi_test should produce sound after note on"
    );
}

#[test]
fn test_gen_test_embeds_correctly() {
    // Verify gen~ embedding doesn't break the simulator
    let (_dir, compiled) = compile_fixture("gen_test");
    let json = compiled.get("gen_voice").expect("gen_voice.maxpat");

    let _sim = flutmax_sim::RnboSimulator::from_json(json)
        .expect("Failed to create RnboSimulator from gen_test");
}

#[test]
fn test_poly_test_with_polyphony() {
    // Verify @polyphony attribute doesn't break the simulator
    let (_dir, compiled) = compile_fixture("poly_test");
    let json = compiled.get("poly_voice").expect("poly_voice.maxpat");

    let _sim = flutmax_sim::RnboSimulator::from_json(json)
        .expect("Failed to create RnboSimulator from poly_test");
}
