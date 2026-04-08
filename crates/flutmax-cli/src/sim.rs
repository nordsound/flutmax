//! `flutmax sim` subcommand: run a compiled .maxpat through flutmax-sim
//! and assert audio properties without Max.
//!
//! Example:
//!   flutmax sim build/synth.maxpat \
//!     --param mode=1 --param bow_pressure=0.5 \
//!     --note-on 60 100 \
//!     --duration 1.0 \
//!     --assert-peak '>0.05' \
//!     --assert-not-silent

use flutmax_sim::{AudioOutput, GenSimulator, RnboSimulator};
use std::fs;
use std::process;

/// CLI entry point for `flutmax sim`.
pub fn run(args: &[String]) -> i32 {
    let mut input_path: Option<String> = None;
    let mut params: Vec<(String, f64)> = Vec::new();
    let mut note_on: Vec<(u8, u8)> = Vec::new();
    let mut note_off: Vec<u8> = Vec::new();
    let mut signal_input: Option<f64> = None; // Constant input value
    let mut sample_rate: f64 = 48000.0;
    let mut duration: f64 = 0.5;
    let mut sim_mode: SimMode = SimMode::Auto;
    let mut assertions: Vec<Assertion> = Vec::new();
    let mut print_metrics = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--param" => {
                let v = require_arg(args, i, "--param");
                let (name, value) = parse_kv(&v).unwrap_or_else(|e| {
                    eprintln!("error: invalid --param {}: {}", v, e);
                    process::exit(1);
                });
                params.push((name, value));
                i += 2;
            }
            "--note-on" => {
                if i + 2 >= args.len() {
                    eprintln!("error: --note-on requires <note> <vel>");
                    process::exit(1);
                }
                let n: u8 = args[i + 1].parse().unwrap_or_else(|_| {
                    eprintln!("error: --note-on note must be 0-127");
                    process::exit(1);
                });
                let v: u8 = args[i + 2].parse().unwrap_or_else(|_| {
                    eprintln!("error: --note-on velocity must be 0-127");
                    process::exit(1);
                });
                note_on.push((n, v));
                i += 3;
            }
            "--note-off" => {
                let v = require_arg(args, i, "--note-off");
                let n: u8 = v.parse().unwrap_or_else(|_| {
                    eprintln!("error: --note-off note must be 0-127");
                    process::exit(1);
                });
                note_off.push(n);
                i += 2;
            }
            "--signal-input" => {
                let v = require_arg(args, i, "--signal-input");
                signal_input = Some(v.parse().unwrap_or_else(|_| {
                    eprintln!("error: --signal-input must be a number");
                    process::exit(1);
                }));
                i += 2;
            }
            "--sample-rate" | "--sr" => {
                let v = require_arg(args, i, "--sample-rate");
                sample_rate = v.parse().unwrap_or_else(|_| {
                    eprintln!("error: --sample-rate must be a number");
                    process::exit(1);
                });
                i += 2;
            }
            "--duration" | "-d" => {
                let v = require_arg(args, i, "--duration");
                duration = v.parse().unwrap_or_else(|_| {
                    eprintln!("error: --duration must be a number");
                    process::exit(1);
                });
                i += 2;
            }
            "--mode" => {
                let v = require_arg(args, i, "--mode");
                sim_mode = match v.as_str() {
                    "rnbo" => SimMode::Rnbo,
                    "gen" => SimMode::Gen,
                    "auto" => SimMode::Auto,
                    other => {
                        eprintln!("error: --mode must be rnbo|gen|auto, got '{}'", other);
                        process::exit(1);
                    }
                };
                i += 2;
            }
            "--assert-peak" => {
                let v = require_arg(args, i, "--assert-peak");
                let cmp = parse_comparison(&v).unwrap_or_else(|e| {
                    eprintln!("error: --assert-peak: {}", e);
                    process::exit(1);
                });
                assertions.push(Assertion::Peak(cmp));
                i += 2;
            }
            "--assert-rms" => {
                let v = require_arg(args, i, "--assert-rms");
                let cmp = parse_comparison(&v).unwrap_or_else(|e| {
                    eprintln!("error: --assert-rms: {}", e);
                    process::exit(1);
                });
                assertions.push(Assertion::Rms(cmp));
                i += 2;
            }
            "--assert-silent" => {
                assertions.push(Assertion::Silent);
                i += 1;
            }
            "--assert-not-silent" => {
                assertions.push(Assertion::NotSilent);
                i += 1;
            }
            "--assert-frequency" | "--assert-freq" => {
                if i + 2 >= args.len() {
                    eprintln!("error: --assert-frequency requires <target> <tolerance>");
                    process::exit(1);
                }
                let target: f64 = args[i + 1].parse().unwrap_or_else(|_| {
                    eprintln!("error: --assert-frequency target must be a number");
                    process::exit(1);
                });
                let tolerance: f64 = args[i + 2].parse().unwrap_or_else(|_| {
                    eprintln!("error: --assert-frequency tolerance must be a number");
                    process::exit(1);
                });
                assertions.push(Assertion::Frequency(target, tolerance));
                i += 3;
            }
            "--print-metrics" | "-p" => {
                print_metrics = true;
                i += 1;
            }
            "--help" | "-h" => {
                print_help();
                return 0;
            }
            arg if arg.starts_with('-') => {
                eprintln!("error: unknown option '{}'", arg);
                print_help();
                return 1;
            }
            arg => {
                if input_path.is_some() {
                    eprintln!("error: multiple input paths specified");
                    return 1;
                }
                input_path = Some(arg.to_string());
                i += 1;
            }
        }
    }

    let input = match input_path {
        Some(p) => p,
        None => {
            eprintln!("error: no input .maxpat file specified");
            print_help();
            return 1;
        }
    };

    // Load JSON
    let json = match fs::read_to_string(&input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: failed to read {}: {}", input, e);
            return 1;
        }
    };

    // Auto-detect simulator mode if needed
    let resolved_mode = match sim_mode {
        SimMode::Auto => detect_mode(&json),
        m => m,
    };

    // Run simulation
    let output = match resolved_mode {
        SimMode::Rnbo => run_rnbo(
            &json,
            &params,
            &note_on,
            &note_off,
            signal_input,
            sample_rate,
            duration,
        ),
        SimMode::Gen => run_gen(&json, &params, signal_input, sample_rate, duration),
        SimMode::Auto => unreachable!(),
    };

    let output = match output {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: simulation failed: {}", e);
            return 1;
        }
    };

    // Print metrics if requested
    if print_metrics || assertions.is_empty() {
        let peak = output.peak();
        let rms = output.rms();
        let freq = output.freq_estimate();
        println!("peak: {:.6}", peak);
        println!("rms:  {:.6}", rms);
        println!("freq: {:.1}", freq);
        println!(
            "samples: {}",
            output.channels.first().map(|c| c.len()).unwrap_or(0)
        );
        println!("channels: {}", output.channels.len());
    }

    // Run assertions
    let mut failed = 0;
    for assertion in &assertions {
        match check_assertion(assertion, &output) {
            Ok(()) => {}
            Err(msg) => {
                eprintln!("FAIL: {}", msg);
                failed += 1;
            }
        }
    }

    if failed > 0 {
        eprintln!();
        eprintln!("{} assertion(s) failed", failed);
        1
    } else {
        if !assertions.is_empty() {
            println!("All {} assertions passed", assertions.len());
        }
        0
    }
}

fn print_help() {
    eprintln!("flutmax sim - run a compiled .maxpat through DSP simulator");
    eprintln!();
    eprintln!("USAGE:");
    eprintln!("    flutmax sim <input.maxpat> [options]");
    eprintln!();
    eprintln!("OPTIONS:");
    eprintln!("    --param <name=value>          Set RNBO param");
    eprintln!("    --note-on <note> <vel>        Send MIDI Note On (RNBO mode)");
    eprintln!("    --note-off <note>             Send MIDI Note Off (RNBO mode)");
    eprintln!("    --signal-input <value>        Constant signal input (gen~ in 0)");
    eprintln!("    --sample-rate <hz>            Sample rate (default 48000)");
    eprintln!("    --duration <seconds>          Run duration (default 0.5)");
    eprintln!("    --mode rnbo|gen|auto          Force simulator mode (default auto)");
    eprintln!();
    eprintln!("ASSERTIONS:");
    eprintln!("    --assert-peak <op N>          e.g. '>0.05', '<1.0', '=0.5'");
    eprintln!("    --assert-rms <op N>           Same syntax as --assert-peak");
    eprintln!("    --assert-silent               Output should be silent (peak < 1e-6)");
    eprintln!("    --assert-not-silent           Output should produce sound");
    eprintln!("    --assert-frequency <hz> <tol> Frequency within ±tolerance Hz");
    eprintln!();
    eprintln!("OUTPUT:");
    eprintln!("    --print-metrics, -p           Print peak/rms/freq even with assertions");
    eprintln!();
    eprintln!("EXAMPLES:");
    eprintln!("    flutmax sim build/synth.maxpat --param freq=440 --duration 1.0 -p");
    eprintln!("    flutmax sim build/synth.maxpat --param mode=1 --note-on 60 100 \\");
    eprintln!("        --assert-peak '>0.05' --assert-not-silent");
}

#[derive(Clone, Copy)]
enum SimMode {
    Auto,
    Rnbo,
    Gen,
}

#[derive(Debug)]
enum Comparison {
    Gt(f64),
    Gte(f64),
    Lt(f64),
    Lte(f64),
    Eq(f64),
}

#[derive(Debug)]
enum Assertion {
    Peak(Comparison),
    Rms(Comparison),
    Silent,
    NotSilent,
    Frequency(f64, f64),
}

fn require_arg(args: &[String], i: usize, name: &str) -> String {
    if i + 1 >= args.len() {
        eprintln!("error: {} requires an argument", name);
        process::exit(1);
    }
    args[i + 1].clone()
}

fn parse_kv(s: &str) -> Result<(String, f64), String> {
    let parts: Vec<&str> = s.splitn(2, '=').collect();
    if parts.len() != 2 {
        return Err("expected name=value".into());
    }
    let value: f64 = parts[1].parse().map_err(|e| format!("{}", e))?;
    Ok((parts[0].to_string(), value))
}

fn parse_comparison(s: &str) -> Result<Comparison, String> {
    let s = s.trim();
    let (op, num_str) = if let Some(rest) = s.strip_prefix(">=") {
        (">=", rest)
    } else if let Some(rest) = s.strip_prefix("<=") {
        ("<=", rest)
    } else if let Some(rest) = s.strip_prefix('>') {
        (">", rest)
    } else if let Some(rest) = s.strip_prefix('<') {
        ("<", rest)
    } else if let Some(rest) = s.strip_prefix('=') {
        ("=", rest)
    } else {
        return Err(format!(
            "expected comparison operator (>, <, >=, <=, =), got '{}'",
            s
        ));
    };
    let value: f64 = num_str
        .trim()
        .parse()
        .map_err(|e| format!("invalid number: {}", e))?;
    Ok(match op {
        ">" => Comparison::Gt(value),
        ">=" => Comparison::Gte(value),
        "<" => Comparison::Lt(value),
        "<=" => Comparison::Lte(value),
        "=" => Comparison::Eq(value),
        _ => unreachable!(),
    })
}

fn detect_mode(json: &str) -> SimMode {
    // Look for "classnamespace": "rnbo" or "dsp.gen"
    if json.contains("\"classnamespace\": \"dsp.gen\"")
        || json.contains("\"classnamespace\":\"dsp.gen\"")
    {
        // Check if it's nested inside an rnbo patcher
        if json.contains("\"classnamespace\": \"rnbo\"")
            || json.contains("\"classnamespace\":\"rnbo\"")
        {
            SimMode::Rnbo
        } else {
            SimMode::Gen
        }
    } else if json.contains("\"classnamespace\": \"rnbo\"")
        || json.contains("\"classnamespace\":\"rnbo\"")
    {
        SimMode::Rnbo
    } else {
        // Top-level patcher with rnbo~ or gen~ box embedded
        // Default to RNBO for friendliness
        SimMode::Rnbo
    }
}

fn run_rnbo(
    json: &str,
    params: &[(String, f64)],
    note_on: &[(u8, u8)],
    note_off: &[u8],
    signal_input: Option<f64>,
    _sample_rate: f64,
    duration: f64,
) -> Result<AudioOutput, String> {
    let mut sim = RnboSimulator::from_json(json)
        .map_err(|e| format!("RnboSimulator parse error: {:?}", e))?;

    for (name, value) in params {
        sim.set_param(name, *value);
    }

    for &(n, v) in note_on {
        sim.send_note_on(n, v);
    }
    for &n in note_off {
        sim.send_note_off(n);
    }

    if let Some(_v) = signal_input {
        // RnboSimulator may have set_signal_input; if not, skip
        // Note: this requires set_signal_input to exist on RnboSimulator
    }

    Ok(sim.run_seconds(duration))
}

fn run_gen(
    json: &str,
    params: &[(String, f64)],
    signal_input: Option<f64>,
    _sample_rate: f64,
    duration: f64,
) -> Result<AudioOutput, String> {
    let mut sim =
        GenSimulator::from_json(json).map_err(|e| format!("GenSimulator parse error: {:?}", e))?;

    // For gen~, params are positional inputs (in 1, in 2, ...)
    // params named "in1", "in2", etc., map to indices
    for (name, value) in params {
        if let Some(idx_str) = name.strip_prefix("in") {
            if let Ok(idx) = idx_str.parse::<usize>() {
                if idx > 0 && idx <= sim.num_inputs() {
                    sim.set_input(idx - 1, *value);
                }
            }
        }
    }

    if let Some(v) = signal_input {
        sim.set_input(0, v);
    }

    let n = (duration * 48000.0) as usize;
    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        sim.process_sample();
        samples.push(sim.get_outputs().first().copied().unwrap_or(0.0));
    }
    Ok(AudioOutput {
        channels: vec![samples],
        sample_rate: 48000.0,
    })
}

fn check_assertion(assertion: &Assertion, output: &AudioOutput) -> Result<(), String> {
    match assertion {
        Assertion::Peak(cmp) => {
            let v = output.peak();
            check_cmp("peak", v, cmp)
        }
        Assertion::Rms(cmp) => {
            let v = output.rms();
            check_cmp("rms", v, cmp)
        }
        Assertion::Silent => {
            if output.is_silent() {
                Ok(())
            } else {
                Err(format!("expected silent, got peak={:.6}", output.peak()))
            }
        }
        Assertion::NotSilent => {
            if !output.is_silent() {
                Ok(())
            } else {
                Err("expected sound, got silence".to_string())
            }
        }
        Assertion::Frequency(target, tolerance) => {
            let measured = output.freq_estimate();
            if (measured - target).abs() <= *tolerance {
                Ok(())
            } else {
                Err(format!(
                    "frequency {:.1} not within ±{} of target {:.1}",
                    measured, tolerance, target
                ))
            }
        }
    }
}

fn check_cmp(name: &str, value: f64, cmp: &Comparison) -> Result<(), String> {
    let (passed, op_str, target) = match cmp {
        Comparison::Gt(t) => (value > *t, ">", *t),
        Comparison::Gte(t) => (value >= *t, ">=", *t),
        Comparison::Lt(t) => (value < *t, "<", *t),
        Comparison::Lte(t) => (value <= *t, "<=", *t),
        Comparison::Eq(t) => ((value - t).abs() < 1e-9, "=", *t),
    };
    if passed {
        Ok(())
    } else {
        Err(format!("{} {:.6} not {} {}", name, value, op_str, target))
    }
}
