//! Object coverage tests for flutmax-codegen.
//!
//! These tests verify that all builtin Max objects and flutmax aliases
//! can be compiled into valid .maxpat JSON through the full pipeline.

use flutmax_ast::{
    CallArg, Expr, InDecl, LitValue, OutAssignment, OutDecl, OutputPortAccess, PortType, Program,
    Wire,
};
use flutmax_codegen::{build_graph, generate};

// ─── Helper functions ───

/// Build a minimal Program that uses a given object and compile it to .maxpat JSON.
///
/// For signal objects: creates signal in/out ports.
/// For control objects: creates float in/out ports.
/// Verifies that the full pipeline succeeds and returns valid JSON.
fn test_object_compiles(object_name: &str, num_args: u32, is_signal: bool) {
    test_object_compiles_multi(object_name, num_args, is_signal, false);
}

/// When multi_outlet=true, use OutputPortAccess (to avoid E020)
fn test_object_compiles_multi(
    object_name: &str,
    num_args: u32,
    is_signal: bool,
    multi_outlet: bool,
) {
    let port_type = if is_signal {
        PortType::Signal
    } else {
        PortType::Float
    };

    let mut in_decls = Vec::new();
    let mut call_args = Vec::new();

    for i in 0..num_args {
        let name = format!("arg{}", i);
        in_decls.push(InDecl {
            index: i,
            name: name.clone(),
            port_type,
        });
        call_args.push(CallArg::positional(Expr::Ref(name)));
    }

    // Use OutputPortAccess for multi-outlet, otherwise bare Ref
    let out_value = if multi_outlet {
        Expr::OutputPortAccess(OutputPortAccess {
            object: "result".to_string(),
            index: 0,
        })
    } else {
        Expr::Ref("result".to_string())
    };

    let program = Program {
        in_decls,
        out_decls: vec![OutDecl {
            index: 0,
            name: "output".to_string(),
            port_type,
            value: None,
        }],
        wires: vec![Wire {
            name: "result".to_string(),
            value: Expr::Call {
                object: object_name.to_string(),
                args: call_args,
            },
            span: None,
            attrs: vec![],
        }],
        out_assignments: vec![OutAssignment {
            index: 0,
            value: out_value,
            span: None,
        }],
        destructuring_wires: Vec::new(),
        msg_decls: Vec::new(),
        direct_connections: Vec::new(),
            feedback_decls: Vec::new(),
            feedback_assignments: Vec::new(),
            state_decls: Vec::new(),
            state_assignments: Vec::new(),
    };

    let graph = build_graph(&program).unwrap_or_else(|e| {
        panic!(
            "build_graph failed for object '{}': {}",
            object_name, e
        )
    });

    let json_str = generate(&graph).unwrap_or_else(|e| {
        panic!(
            "generate failed for object '{}': {}",
            object_name, e
        )
    });

    // Verify the output is valid JSON
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap_or_else(|e| {
        panic!(
            "output is not valid JSON for object '{}': {}",
            object_name, e
        )
    });

    // Verify basic .maxpat structure
    assert!(
        parsed.get("patcher").is_some(),
        "missing 'patcher' key for object '{}'",
        object_name
    );
    assert!(
        parsed["patcher"].get("boxes").is_some(),
        "missing 'boxes' key for object '{}'",
        object_name
    );
    assert!(
        parsed["patcher"].get("lines").is_some(),
        "missing 'lines' key for object '{}'",
        object_name
    );

    // Verify boxes array is non-empty (at least the object + outlet)
    let boxes = parsed["patcher"]["boxes"].as_array().unwrap();
    assert!(
        boxes.len() >= 2,
        "expected at least 2 boxes (object + outlet) for '{}', got {}",
        object_name,
        boxes.len()
    );
}

/// Build a minimal Program using a flutmax alias and verify it resolves
/// correctly to the expected Max object name in the output.
fn test_object_alias(alias_name: &str, expected_max_name: &str, is_signal: bool) {
    let port_type = if is_signal {
        PortType::Signal
    } else {
        PortType::Float
    };

    let program = Program {
        in_decls: vec![
            InDecl {
                index: 0,
                name: "a".to_string(),
                port_type,
            },
            InDecl {
                index: 1,
                name: "b".to_string(),
                port_type,
            },
        ],
        out_decls: vec![OutDecl {
            index: 0,
            name: "output".to_string(),
            port_type,
            value: None,
        }],
        wires: vec![Wire {
            name: "result".to_string(),
            value: Expr::Call {
                object: alias_name.to_string(),
                args: vec![CallArg::positional(Expr::Ref("a".to_string())), CallArg::positional(Expr::Ref("b".to_string()))],
            },
            span: None,
            attrs: vec![],
        }],
        out_assignments: vec![OutAssignment {
            index: 0,
            value: Expr::Ref("result".to_string()),
            span: None,
        }],
        destructuring_wires: Vec::new(),
        msg_decls: Vec::new(),
        direct_connections: Vec::new(),
            feedback_decls: Vec::new(),
            feedback_assignments: Vec::new(),
            state_decls: Vec::new(),
            state_assignments: Vec::new(),
    };

    let graph = build_graph(&program).unwrap_or_else(|e| {
        panic!(
            "build_graph failed for alias '{}': {}",
            alias_name, e
        )
    });

    let json_str = generate(&graph).unwrap_or_else(|e| {
        panic!(
            "generate failed for alias '{}': {}",
            alias_name, e
        )
    });

    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let boxes = parsed["patcher"]["boxes"].as_array().unwrap();

    // Find the box with the expected Max object name
    let found = boxes.iter().any(|b| {
        b["box"]["text"]
            .as_str()
            .map(|t| {
                let first_token = t.split_whitespace().next().unwrap_or("");
                first_token == expected_max_name
            })
            .unwrap_or(false)
    });

    assert!(
        found,
        "alias '{}' should resolve to Max object '{}', but not found in boxes: {:?}",
        alias_name,
        expected_max_name,
        boxes
            .iter()
            .filter_map(|b| b["box"]["text"].as_str())
            .collect::<Vec<_>>()
    );
}

/// Test that a zero-arg object compiles (e.g., button, loadbang).
fn test_zero_arg_object(object_name: &str, is_signal: bool) {
    let port_type = if is_signal {
        PortType::Signal
    } else {
        PortType::Float
    };

    let program = Program {
        in_decls: Vec::new(),
        out_decls: vec![OutDecl {
            index: 0,
            name: "output".to_string(),
            port_type,
            value: None,
        }],
        wires: vec![Wire {
            name: "result".to_string(),
            value: Expr::Call {
                object: object_name.to_string(),
                args: vec![],
            },
            span: None,
            attrs: vec![],
        }],
        out_assignments: vec![OutAssignment {
            index: 0,
            value: Expr::Ref("result".to_string()),
            span: None,
        }],
        destructuring_wires: Vec::new(),
        msg_decls: Vec::new(),
        direct_connections: Vec::new(),
            feedback_decls: Vec::new(),
            feedback_assignments: Vec::new(),
            state_decls: Vec::new(),
            state_assignments: Vec::new(),
    };

    let graph = build_graph(&program).unwrap_or_else(|e| {
        panic!(
            "build_graph failed for zero-arg object '{}': {}",
            object_name, e
        )
    });

    let json_str = generate(&graph).unwrap_or_else(|e| {
        panic!(
            "generate failed for zero-arg object '{}': {}",
            object_name, e
        )
    });

    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert!(parsed.get("patcher").is_some());
}

/// Test compilation with literal arguments (e.g., cycle~(440)).
fn test_object_with_literal(object_name: &str, lit_val: LitValue, is_signal: bool) {
    let port_type = if is_signal {
        PortType::Signal
    } else {
        PortType::Float
    };

    let program = Program {
        in_decls: Vec::new(),
        out_decls: vec![OutDecl {
            index: 0,
            name: "output".to_string(),
            port_type,
            value: None,
        }],
        wires: vec![Wire {
            name: "result".to_string(),
            value: Expr::Call {
                object: object_name.to_string(),
                args: vec![CallArg::positional(Expr::Lit(lit_val))],
            },
            span: None,
            attrs: vec![],
        }],
        out_assignments: vec![OutAssignment {
            index: 0,
            value: Expr::Ref("result".to_string()),
            span: None,
        }],
        destructuring_wires: Vec::new(),
        msg_decls: Vec::new(),
        direct_connections: Vec::new(),
            feedback_decls: Vec::new(),
            feedback_assignments: Vec::new(),
            state_decls: Vec::new(),
            state_assignments: Vec::new(),
    };

    let graph = build_graph(&program).unwrap();
    let json_str = generate(&graph).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert!(parsed.get("patcher").is_some());
}

// ─── Tier 1: Signal DSP objects (~) ───

#[test]
fn test_cycle_tilde() {
    test_object_compiles("cycle~", 1, true);
}

#[test]
fn test_cycle_tilde_with_literal() {
    test_object_with_literal("cycle~", LitValue::Int(440), true);
}

#[test]
fn test_cycle_tilde_with_float_literal() {
    test_object_with_literal("cycle~", LitValue::Float(440.5), true);
}

#[test]
fn test_mul_tilde() {
    test_object_compiles("*~", 2, true);
}

#[test]
fn test_add_tilde() {
    test_object_compiles("+~", 2, true);
}

#[test]
fn test_sub_tilde() {
    test_object_compiles("-~", 2, true);
}

#[test]
fn test_div_tilde() {
    test_object_compiles("/~", 2, true);
}

#[test]
fn test_phasor_tilde() {
    test_object_compiles("phasor~", 1, true);
}

#[test]
fn test_phasor_tilde_with_literal() {
    test_object_with_literal("phasor~", LitValue::Int(100), true);
}

#[test]
fn test_noise_tilde() {
    test_zero_arg_object("noise~", true);
}

#[test]
fn test_biquad_tilde() {
    test_object_compiles("biquad~", 3, true);
}

#[test]
fn test_line_tilde() {
    test_object_compiles_multi("line~", 1, true, true);
}

// ─── Tier 1: Control arithmetic objects ───

#[test]
fn test_mul_control() {
    test_object_compiles("*", 2, false);
}

#[test]
fn test_add_control() {
    test_object_compiles("+", 2, false);
}

#[test]
fn test_sub_control() {
    test_object_compiles("-", 2, false);
}

#[test]
fn test_div_control() {
    test_object_compiles("/", 2, false);
}

#[test]
fn test_mod_control() {
    test_object_compiles("%", 2, false);
}

// ─── Tier 1: Utility objects ───

#[test]
fn test_loadbang() {
    test_zero_arg_object("loadbang", false);
}

#[test]
fn test_button() {
    test_zero_arg_object("button", false);
}

#[test]
fn test_print() {
    test_object_compiles("print", 1, false);
}

#[test]
fn test_print_with_string_arg() {
    test_object_with_literal("print", LitValue::Str("debug".to_string()), false);
}

// ─── Tier 1: flutmax aliases ───

#[test]
fn test_alias_add() {
    test_object_alias("add", "+", false);
}

#[test]
fn test_alias_sub() {
    test_object_alias("sub", "-", false);
}

#[test]
fn test_alias_mul() {
    test_object_alias("mul", "*", false);
}

#[test]
fn test_alias_dvd() {
    test_object_alias("dvd", "/", false);
}

#[test]
fn test_alias_mod() {
    test_object_alias("mod", "%", false);
}

#[test]
fn test_alias_add_tilde() {
    test_object_alias("add~", "+~", true);
}

#[test]
fn test_alias_sub_tilde() {
    test_object_alias("sub~", "-~", true);
}

#[test]
fn test_alias_mul_tilde() {
    test_object_alias("mul~", "*~", true);
}

#[test]
fn test_alias_dvd_tilde() {
    test_object_alias("dvd~", "/~", true);
}

#[test]
fn test_alias_mod_tilde() {
    test_object_alias("mod~", "%~", true);
}

// ─── Signal chain / composition tests ───

#[test]
fn test_signal_chain_two_objects() {
    // cycle~(440) -> *~(_, 0.5) -> outlet~
    let program = Program {
        in_decls: Vec::new(),
        out_decls: vec![OutDecl {
            index: 0,
            name: "output".to_string(),
            port_type: PortType::Signal,
                value: None,
        }],
        wires: vec![
            Wire {
                name: "osc".to_string(),
                value: Expr::Call {
                    object: "cycle~".to_string(),
                    args: vec![CallArg::positional(Expr::Lit(LitValue::Int(440)))],
                },
                span: None,
                attrs: vec![],
            },
            Wire {
                name: "amp".to_string(),
                value: Expr::Call {
                    object: "*~".to_string(),
                    args: vec![
                        CallArg::positional(Expr::Ref("osc".to_string())),
                        CallArg::positional(Expr::Lit(LitValue::Float(0.5))),
                    ],
                },
                span: None,
                attrs: vec![],
            },
        ],
        out_assignments: vec![OutAssignment {
            index: 0,
            value: Expr::Ref("amp".to_string()),
            span: None,
        }],
        destructuring_wires: Vec::new(),
        msg_decls: Vec::new(),
        direct_connections: Vec::new(),
            feedback_decls: Vec::new(),
            feedback_assignments: Vec::new(),
            state_decls: Vec::new(),
            state_assignments: Vec::new(),
    };

    let graph = build_graph(&program).unwrap();
    let json_str = generate(&graph).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    let boxes = parsed["patcher"]["boxes"].as_array().unwrap();
    let lines = parsed["patcher"]["lines"].as_array().unwrap();

    // outlet~ + cycle~ + *~ = 3 boxes
    assert_eq!(boxes.len(), 3, "signal chain should have 3 boxes");
    // cycle~ -> *~ and *~ -> outlet~ = 2 connections
    assert_eq!(lines.len(), 2, "signal chain should have 2 connections");
}

#[test]
fn test_multiple_outlets() {
    // Two separate outputs
    let program = Program {
        in_decls: vec![InDecl {
            index: 0,
            name: "freq".to_string(),
            port_type: PortType::Float,
        }],
        out_decls: vec![
            OutDecl {
                index: 0,
                name: "left".to_string(),
                port_type: PortType::Signal,
                value: None,
            },
            OutDecl {
                index: 1,
                name: "right".to_string(),
                port_type: PortType::Signal,
                value: None,
            },
        ],
        wires: vec![Wire {
            name: "osc".to_string(),
            value: Expr::Call {
                object: "cycle~".to_string(),
                args: vec![CallArg::positional(Expr::Ref("freq".to_string()))],
            },
            span: None,
            attrs: vec![],
        }],
        out_assignments: vec![
            OutAssignment {
                index: 0,
                value: Expr::Ref("osc".to_string()),
                span: None,
            },
            OutAssignment {
                index: 1,
                value: Expr::Ref("osc".to_string()),
                span: None,
            },
        ],
        destructuring_wires: Vec::new(),
        msg_decls: Vec::new(),
        direct_connections: Vec::new(),
            feedback_decls: Vec::new(),
            feedback_assignments: Vec::new(),
            state_decls: Vec::new(),
            state_assignments: Vec::new(),
    };

    let graph = build_graph(&program).unwrap();
    let json_str = generate(&graph).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    let boxes = parsed["patcher"]["boxes"].as_array().unwrap();
    // inlet + cycle~ + outlet~[0] + outlet~[1] = 4 boxes (or more if trigger is added)
    assert!(
        boxes.len() >= 4,
        "dual outlet should have at least 4 boxes, got {}",
        boxes.len()
    );
}

#[test]
fn test_nested_call() {
    // biquad~(cycle~(440), 1000, 0.7)
    let program = Program {
        in_decls: Vec::new(),
        out_decls: vec![OutDecl {
            index: 0,
            name: "output".to_string(),
            port_type: PortType::Signal,
                value: None,
        }],
        wires: vec![Wire {
            name: "filtered".to_string(),
            value: Expr::Call {
                object: "biquad~".to_string(),
                args: vec![
                    CallArg::positional(Expr::Call {
                        object: "cycle~".to_string(),
                        args: vec![CallArg::positional(Expr::Lit(LitValue::Int(440)))],
                    }),
                    CallArg::positional(Expr::Lit(LitValue::Int(1000))),
                    CallArg::positional(Expr::Lit(LitValue::Float(0.7))),
                ],
            },
            span: None,
            attrs: vec![],
        }],
        out_assignments: vec![OutAssignment {
            index: 0,
            value: Expr::Ref("filtered".to_string()),
            span: None,
        }],
        destructuring_wires: Vec::new(),
        msg_decls: Vec::new(),
        direct_connections: Vec::new(),
            feedback_decls: Vec::new(),
            feedback_assignments: Vec::new(),
            state_decls: Vec::new(),
            state_assignments: Vec::new(),
    };

    let graph = build_graph(&program).unwrap();
    let json_str = generate(&graph).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    let boxes = parsed["patcher"]["boxes"].as_array().unwrap();
    // outlet~ + cycle~ + biquad~ = 3 boxes
    assert_eq!(
        boxes.len(),
        3,
        "nested call should produce 3 boxes, got {}",
        boxes.len()
    );
}

// ─── Unknown / custom objects ───

#[test]
fn test_unknown_object_defaults_to_one_inlet_one_outlet() {
    // An object not in the builtin DB should get default 1 inlet, 1 outlet
    test_object_compiles("my_custom_object", 1, false);
}

#[test]
fn test_unknown_signal_object() {
    // An unknown signal object (ends with ~)
    test_object_compiles("my_filter~", 2, true);
}

// ─── Literal value formatting ───

#[test]
fn test_integer_literal_in_args() {
    let program = Program {
        in_decls: Vec::new(),
        out_decls: vec![OutDecl {
            index: 0,
            name: "output".to_string(),
            port_type: PortType::Signal,
                value: None,
        }],
        wires: vec![Wire {
            name: "osc".to_string(),
            value: Expr::Call {
                object: "cycle~".to_string(),
                args: vec![CallArg::positional(Expr::Lit(LitValue::Int(440)))],
            },
            span: None,
            attrs: vec![],
        }],
        out_assignments: vec![OutAssignment {
            index: 0,
            value: Expr::Ref("osc".to_string()),
            span: None,
        }],
        destructuring_wires: Vec::new(),
        msg_decls: Vec::new(),
        direct_connections: Vec::new(),
            feedback_decls: Vec::new(),
            feedback_assignments: Vec::new(),
            state_decls: Vec::new(),
            state_assignments: Vec::new(),
    };

    let graph = build_graph(&program).unwrap();
    let json_str = generate(&graph).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    // Find the cycle~ box and check its text includes "440"
    let boxes = parsed["patcher"]["boxes"].as_array().unwrap();
    let cycle_box = boxes
        .iter()
        .find(|b| {
            b["box"]["text"]
                .as_str()
                .map(|t| t.starts_with("cycle~"))
                .unwrap_or(false)
        })
        .expect("should have a cycle~ box");
    let text = cycle_box["box"]["text"].as_str().unwrap();
    assert!(
        text.contains("440"),
        "cycle~ text should contain '440', got '{}'",
        text
    );
}

#[test]
fn test_float_literal_in_args() {
    let program = Program {
        in_decls: Vec::new(),
        out_decls: vec![OutDecl {
            index: 0,
            name: "output".to_string(),
            port_type: PortType::Signal,
                value: None,
        }],
        wires: vec![
            Wire {
                name: "osc".to_string(),
                value: Expr::Call {
                    object: "cycle~".to_string(),
                    args: vec![CallArg::positional(Expr::Lit(LitValue::Int(440)))],
                },
                span: None,
                attrs: vec![],
            },
            Wire {
                name: "amp".to_string(),
                value: Expr::Call {
                    object: "*~".to_string(),
                    args: vec![
                        CallArg::positional(Expr::Ref("osc".to_string())),
                        CallArg::positional(Expr::Lit(LitValue::Float(0.5))),
                    ],
                },
                span: None,
                attrs: vec![],
            },
        ],
        out_assignments: vec![OutAssignment {
            index: 0,
            value: Expr::Ref("amp".to_string()),
            span: None,
        }],
        destructuring_wires: Vec::new(),
        msg_decls: Vec::new(),
        direct_connections: Vec::new(),
            feedback_decls: Vec::new(),
            feedback_assignments: Vec::new(),
            state_decls: Vec::new(),
            state_assignments: Vec::new(),
    };

    let graph = build_graph(&program).unwrap();
    let json_str = generate(&graph).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    let boxes = parsed["patcher"]["boxes"].as_array().unwrap();
    let mul_box = boxes
        .iter()
        .find(|b| {
            b["box"]["text"]
                .as_str()
                .map(|t| t.starts_with("*~"))
                .unwrap_or(false)
        })
        .expect("should have a *~ box");
    let text = mul_box["box"]["text"].as_str().unwrap();
    assert!(
        text.contains("0.5"),
        "*~ text should contain '0.5', got '{}'",
        text
    );
}

#[test]
fn test_string_literal_in_args() {
    let program = Program {
        in_decls: Vec::new(),
        out_decls: vec![OutDecl {
            index: 0,
            name: "output".to_string(),
            port_type: PortType::Float,
                value: None,
        }],
        wires: vec![Wire {
            name: "p".to_string(),
            value: Expr::Call {
                object: "print".to_string(),
                args: vec![CallArg::positional(Expr::Lit(LitValue::Str("debug".to_string())))],
            },
            span: None,
            attrs: vec![],
        }],
        out_assignments: vec![OutAssignment {
            index: 0,
            value: Expr::Ref("p".to_string()),
            span: None,
        }],
        destructuring_wires: Vec::new(),
        msg_decls: Vec::new(),
        direct_connections: Vec::new(),
            feedback_decls: Vec::new(),
            feedback_assignments: Vec::new(),
            state_decls: Vec::new(),
            state_assignments: Vec::new(),
    };

    let graph = build_graph(&program).unwrap();
    let json_str = generate(&graph).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    let boxes = parsed["patcher"]["boxes"].as_array().unwrap();
    let print_box = boxes
        .iter()
        .find(|b| {
            b["box"]["text"]
                .as_str()
                .map(|t| t.starts_with("print"))
                .unwrap_or(false)
        })
        .expect("should have a print box");
    let text = print_box["box"]["text"].as_str().unwrap();
    assert!(
        text.contains("debug"),
        "print text should contain 'debug', got '{}'",
        text
    );
}

// ─── Edge cases ───

#[test]
fn test_empty_program_with_only_outlet() {
    // A program with just an outlet, no wires
    let program = Program {
        in_decls: Vec::new(),
        out_decls: vec![OutDecl {
            index: 0,
            name: "output".to_string(),
            port_type: PortType::Float,
                value: None,
        }],
        wires: Vec::new(),
        out_assignments: Vec::new(),
        destructuring_wires: Vec::new(),
        msg_decls: Vec::new(),
        direct_connections: Vec::new(),
            feedback_decls: Vec::new(),
            feedback_assignments: Vec::new(),
            state_decls: Vec::new(),
            state_assignments: Vec::new(),
    };

    let graph = build_graph(&program).unwrap();
    let json_str = generate(&graph).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert!(parsed.get("patcher").is_some());
}

#[test]
fn test_program_with_only_inlet() {
    let program = Program {
        in_decls: vec![InDecl {
            index: 0,
            name: "input".to_string(),
            port_type: PortType::Float,
        }],
        out_decls: Vec::new(),
        wires: Vec::new(),
        out_assignments: Vec::new(),
        destructuring_wires: Vec::new(),
        msg_decls: Vec::new(),
        direct_connections: Vec::new(),
            feedback_decls: Vec::new(),
            feedback_assignments: Vec::new(),
            state_decls: Vec::new(),
            state_assignments: Vec::new(),
    };

    let graph = build_graph(&program).unwrap();
    let json_str = generate(&graph).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert!(parsed.get("patcher").is_some());
}

#[test]
fn test_many_inlets() {
    // Program with 10 inlets
    let mut in_decls = Vec::new();
    for i in 0..10 {
        in_decls.push(InDecl {
            index: i,
            name: format!("in{}", i),
            port_type: PortType::Float,
        });
    }

    let program = Program {
        in_decls,
        out_decls: vec![OutDecl {
            index: 0,
            name: "output".to_string(),
            port_type: PortType::Float,
                value: None,
        }],
        wires: Vec::new(),
        out_assignments: Vec::new(),
        destructuring_wires: Vec::new(),
        msg_decls: Vec::new(),
        direct_connections: Vec::new(),
            feedback_decls: Vec::new(),
            feedback_assignments: Vec::new(),
            state_decls: Vec::new(),
            state_assignments: Vec::new(),
    };

    let graph = build_graph(&program).unwrap();
    let json_str = generate(&graph).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let boxes = parsed["patcher"]["boxes"].as_array().unwrap();
    // 10 inlets + 1 outlet = 11 boxes
    assert_eq!(boxes.len(), 11);
}

#[test]
fn test_many_wires_chain() {
    // Chain of 20 *~ operations
    let mut wires = Vec::new();

    wires.push(Wire {
        name: "w0".to_string(),
        value: Expr::Call {
            object: "cycle~".to_string(),
            args: vec![CallArg::positional(Expr::Lit(LitValue::Int(440)))],
        },
        span: None,
        attrs: vec![],
    });

    for i in 1..20 {
        wires.push(Wire {
            name: format!("w{}", i),
            value: Expr::Call {
                object: "*~".to_string(),
                args: vec![
                    CallArg::positional(Expr::Ref(format!("w{}", i - 1))),
                    CallArg::positional(Expr::Lit(LitValue::Float(0.99))),
                ],
            },
            span: None,
            attrs: vec![],
        });
    }

    let program = Program {
        in_decls: Vec::new(),
        out_decls: vec![OutDecl {
            index: 0,
            name: "output".to_string(),
            port_type: PortType::Signal,
                value: None,
        }],
        wires,
        out_assignments: vec![OutAssignment {
            index: 0,
            value: Expr::Ref("w19".to_string()),
            span: None,
        }],
        destructuring_wires: Vec::new(),
        msg_decls: Vec::new(),
        direct_connections: Vec::new(),
            feedback_decls: Vec::new(),
            feedback_assignments: Vec::new(),
            state_decls: Vec::new(),
            state_assignments: Vec::new(),
    };

    let graph = build_graph(&program).unwrap();
    let json_str = generate(&graph).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    let boxes = parsed["patcher"]["boxes"].as_array().unwrap();
    // 1 cycle~ + 19 *~ + 1 outlet~ = 21 boxes
    assert_eq!(boxes.len(), 21);

    let lines = parsed["patcher"]["lines"].as_array().unwrap();
    // 19 chain connections + 1 to outlet = 20
    assert_eq!(lines.len(), 20);
}

// ─── numinlets / numoutlets verification ───

#[test]
fn test_cycle_tilde_has_correct_numinlets() {
    let program = Program {
        in_decls: Vec::new(),
        out_decls: vec![OutDecl {
            index: 0,
            name: "output".to_string(),
            port_type: PortType::Signal,
                value: None,
        }],
        wires: vec![Wire {
            name: "osc".to_string(),
            value: Expr::Call {
                object: "cycle~".to_string(),
                args: vec![CallArg::positional(Expr::Lit(LitValue::Int(440)))],
            },
            span: None,
            attrs: vec![],
        }],
        out_assignments: vec![OutAssignment {
            index: 0,
            value: Expr::Ref("osc".to_string()),
            span: None,
        }],
        destructuring_wires: Vec::new(),
        msg_decls: Vec::new(),
        direct_connections: Vec::new(),
            feedback_decls: Vec::new(),
            feedback_assignments: Vec::new(),
            state_decls: Vec::new(),
            state_assignments: Vec::new(),
    };

    let graph = build_graph(&program).unwrap();
    let json_str = generate(&graph).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    let boxes = parsed["patcher"]["boxes"].as_array().unwrap();
    let cycle_box = boxes
        .iter()
        .find(|b| {
            b["box"]["text"]
                .as_str()
                .map(|t| t.starts_with("cycle~"))
                .unwrap_or(false)
        })
        .expect("should have cycle~ box");

    let numinlets = cycle_box["box"]["numinlets"].as_u64().unwrap();
    assert_eq!(numinlets, 2, "cycle~ should have 2 inlets");

    let numoutlets = cycle_box["box"]["numoutlets"].as_u64().unwrap();
    assert_eq!(numoutlets, 1, "cycle~ should have 1 outlet");
}

#[test]
fn test_biquad_tilde_has_correct_numinlets() {
    let program = Program {
        in_decls: vec![InDecl {
            index: 0,
            name: "input".to_string(),
            port_type: PortType::Signal,
        }],
        out_decls: vec![OutDecl {
            index: 0,
            name: "output".to_string(),
            port_type: PortType::Signal,
                value: None,
        }],
        wires: vec![Wire {
            name: "filtered".to_string(),
            value: Expr::Call {
                object: "biquad~".to_string(),
                args: vec![
                    CallArg::positional(Expr::Ref("input".to_string())),
                    CallArg::positional(Expr::Lit(LitValue::Float(1.0))),
                    CallArg::positional(Expr::Lit(LitValue::Float(0.0))),
                    CallArg::positional(Expr::Lit(LitValue::Float(0.0))),
                    CallArg::positional(Expr::Lit(LitValue::Float(0.0))),
                    CallArg::positional(Expr::Lit(LitValue::Float(0.0))),
                ],
            },
            span: None,
            attrs: vec![],
        }],
        out_assignments: vec![OutAssignment {
            index: 0,
            value: Expr::Ref("filtered".to_string()),
            span: None,
        }],
        destructuring_wires: Vec::new(),
        msg_decls: Vec::new(),
        direct_connections: Vec::new(),
            feedback_decls: Vec::new(),
            feedback_assignments: Vec::new(),
            state_decls: Vec::new(),
            state_assignments: Vec::new(),
    };

    let graph = build_graph(&program).unwrap();
    let json_str = generate(&graph).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    let boxes = parsed["patcher"]["boxes"].as_array().unwrap();
    let biquad_box = boxes
        .iter()
        .find(|b| {
            b["box"]["text"]
                .as_str()
                .map(|t| t.starts_with("biquad~"))
                .unwrap_or(false)
        })
        .expect("should have biquad~ box");

    let numinlets = biquad_box["box"]["numinlets"].as_u64().unwrap();
    assert_eq!(numinlets, 6, "biquad~ should have 6 inlets");
}
