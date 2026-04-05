//! Parser edge case tests.
//!
//! These tests verify the parser handles various edge cases correctly,
//! including deeply nested calls, many wires, long identifiers,
//! comments, and all type names.

use flutmax_parser::parse;

// ─── Deep nesting ───

#[test]
fn deeply_nested_two_levels() {
    let source = "wire x = a(b(440));";
    let prog = parse(source);
    assert!(prog.is_ok(), "two-level nesting should parse");
    assert_eq!(prog.unwrap().wires.len(), 1);
}

#[test]
fn deeply_nested_three_levels() {
    let source = "wire x = a(b(c(440)));";
    let prog = parse(source);
    assert!(prog.is_ok(), "three-level nesting should parse");
}

#[test]
fn deeply_nested_four_levels() {
    let source = "wire x = a(b(c(d(440))));";
    let prog = parse(source);
    assert!(prog.is_ok(), "four-level nesting should parse");
}

#[test]
fn deeply_nested_five_levels() {
    let source = "wire x = a(b(c(d(e(440)))));";
    let prog = parse(source);
    assert!(prog.is_ok(), "five-level nesting should parse");
}

#[test]
fn nested_signal_objects() {
    let source = "wire x = mul~(cycle~(440), 0.5);";
    let prog = parse(source).unwrap();
    assert_eq!(prog.wires.len(), 1);
}

#[test]
fn nested_mixed_signal_control() {
    // Signal object with nested control object
    let source = "wire x = cycle~(mul(440, 2));";
    let prog = parse(source).unwrap();
    assert_eq!(prog.wires.len(), 1);
}

// ─── Many wires ───

#[test]
fn many_wires_10() {
    let mut source = String::new();
    for i in 0..10 {
        source.push_str(&format!("wire w{} = cycle~({});\n", i, 440 + i));
    }
    let prog = parse(&source).unwrap();
    assert_eq!(prog.wires.len(), 10);
}

#[test]
fn many_wires_50() {
    let mut source = String::new();
    for i in 0..50 {
        source.push_str(&format!("wire w{} = cycle~({});\n", i, 440 + i));
    }
    let prog = parse(&source).unwrap();
    assert_eq!(prog.wires.len(), 50);
}

#[test]
fn many_wires_100() {
    let mut source = String::new();
    for i in 0..100 {
        source.push_str(&format!("wire w{} = cycle~({});\n", i, 440 + i));
    }
    let prog = parse(&source).unwrap();
    assert_eq!(prog.wires.len(), 100);
}

#[test]
fn chained_wires_50() {
    let mut source = String::from("wire w0 = cycle~(440);\n");
    for i in 1..50 {
        source.push_str(&format!("wire w{} = mul~(w{}, 0.99);\n", i, i - 1));
    }
    let prog = parse(&source).unwrap();
    assert_eq!(prog.wires.len(), 50);
}

// ─── Long identifiers ───

#[test]
fn long_identifier_32_chars() {
    let name = "a".repeat(32);
    let source = format!("wire {} = cycle~(440);", name);
    let prog = parse(&source).unwrap();
    assert_eq!(prog.wires[0].name, name);
}

#[test]
fn long_identifier_64_chars() {
    let name = "a".repeat(64);
    let source = format!("wire {} = cycle~(440);", name);
    let prog = parse(&source).unwrap();
    assert_eq!(prog.wires[0].name, name);
}

#[test]
fn long_identifier_128_chars() {
    let name = "a".repeat(128);
    let source = format!("wire {} = cycle~(440);", name);
    let prog = parse(&source).unwrap();
    assert_eq!(prog.wires[0].name, name);
}

#[test]
fn long_identifier_256_chars() {
    let name = "a".repeat(256);
    let source = format!("wire {} = cycle~(440);", name);
    let prog = parse(&source).unwrap();
    assert_eq!(prog.wires[0].name, name);
}

#[test]
fn identifier_with_underscores() {
    let source = "wire my_long_variable_name = cycle~(440);";
    let prog = parse(source).unwrap();
    assert_eq!(prog.wires[0].name, "my_long_variable_name");
}

#[test]
fn identifier_with_numbers() {
    let source = "wire osc123 = cycle~(440);";
    let prog = parse(source).unwrap();
    assert_eq!(prog.wires[0].name, "osc123");
}

// ─── Comments and whitespace ───

#[test]
fn empty_lines_and_comments() {
    let source = "\n\n// comment\n\nwire x = cycle~(440);\n\n// end\n";
    let prog = parse(source).unwrap();
    assert_eq!(prog.wires.len(), 1);
}

#[test]
fn comment_only_source() {
    let source = "// just a comment\n// another comment\n";
    let prog = parse(source).unwrap();
    assert!(prog.wires.is_empty());
    assert!(prog.in_decls.is_empty());
    assert!(prog.out_decls.is_empty());
}

#[test]
fn many_blank_lines() {
    let mut source = String::new();
    for _ in 0..20 {
        source.push('\n');
    }
    source.push_str("wire x = cycle~(440);");
    for _ in 0..20 {
        source.push('\n');
    }
    let prog = parse(&source).unwrap();
    assert_eq!(prog.wires.len(), 1);
}

#[test]
fn comment_after_statement() {
    let source = "wire x = cycle~(440); // inline comment is not in grammar but after ;";
    // The parser should either accept this or reject gracefully.
    // Since the semicolon ends the statement, trailing text might be ignored
    // or cause an error. We just check it doesn't panic.
    let _ = parse(source);
}

#[test]
fn comment_between_declarations() {
    let source = r#"
in 0 (freq): float;
// This is a comment between declarations
out 0 (audio): signal;
// Another comment
wire osc = cycle~(freq);
out[0] = osc;
"#;
    let prog = parse(source).unwrap();
    assert_eq!(prog.in_decls.len(), 1);
    assert_eq!(prog.out_decls.len(), 1);
    assert_eq!(prog.wires.len(), 1);
    assert_eq!(prog.out_assignments.len(), 1);
}

#[test]
fn unicode_in_comments() {
    let source = "// 日本語コメント\nwire x = cycle~(440);";
    let prog = parse(source).unwrap();
    assert_eq!(prog.wires.len(), 1);
}

#[test]
fn unicode_emoji_in_comments() {
    let source = "// 🎵 Music note comment\nwire x = cycle~(440);";
    let prog = parse(source).unwrap();
    assert_eq!(prog.wires.len(), 1);
}

// ─── All type names ───

#[test]
fn type_signal() {
    let source = "in 0 (test): signal;";
    let prog = parse(source).unwrap();
    assert_eq!(prog.in_decls[0].port_type, flutmax_ast::PortType::Signal);
}

#[test]
fn type_float() {
    let source = "in 0 (test): float;";
    let prog = parse(source).unwrap();
    assert_eq!(prog.in_decls[0].port_type, flutmax_ast::PortType::Float);
}

#[test]
fn type_int() {
    let source = "in 0 (test): int;";
    let prog = parse(source).unwrap();
    assert_eq!(prog.in_decls[0].port_type, flutmax_ast::PortType::Int);
}

#[test]
fn type_bang() {
    let source = "in 0 (test): bang;";
    let prog = parse(source).unwrap();
    assert_eq!(prog.in_decls[0].port_type, flutmax_ast::PortType::Bang);
}

#[test]
fn type_list() {
    let source = "in 0 (test): list;";
    let prog = parse(source).unwrap();
    assert_eq!(prog.in_decls[0].port_type, flutmax_ast::PortType::List);
}

#[test]
fn type_symbol() {
    let source = "in 0 (test): symbol;";
    let prog = parse(source).unwrap();
    assert_eq!(prog.in_decls[0].port_type, flutmax_ast::PortType::Symbol);
}

#[test]
fn all_types_as_out_decl() {
    for type_name in &["signal", "float", "int", "bang", "list", "symbol"] {
        let source = format!("out 0 (test): {};", type_name);
        let prog = parse(&source).unwrap_or_else(|e| {
            panic!("failed to parse out decl with type '{}': {}", type_name, e)
        });
        assert_eq!(prog.out_decls.len(), 1);
    }
}

// ─── Port indices ───

#[test]
fn port_index_zero() {
    let source = "in 0 (test): float;";
    let prog = parse(source).unwrap();
    assert_eq!(prog.in_decls[0].index, 0);
}

#[test]
fn port_index_large() {
    let source = "in 99 (test): float;";
    let prog = parse(source).unwrap();
    assert_eq!(prog.in_decls[0].index, 99);
}

#[test]
fn multiple_port_indices() {
    let source = r#"
in 0 (a): float;
in 1 (b): float;
in 2 (c): float;
in 3 (d): float;
in 4 (e): float;
"#;
    let prog = parse(source).unwrap();
    assert_eq!(prog.in_decls.len(), 5);
    for (i, decl) in prog.in_decls.iter().enumerate() {
        assert_eq!(decl.index, i as u32);
    }
}

// ─── Literal values ───

#[test]
fn integer_literal() {
    let source = "wire x = cycle~(440);";
    let prog = parse(source).unwrap();
    match &prog.wires[0].value {
        flutmax_ast::Expr::Call { args, .. } => {
            assert_eq!(
                args[0].value,
                flutmax_ast::Expr::Lit(flutmax_ast::LitValue::Int(440))
            );
        }
        _ => panic!("expected Call"),
    }
}

#[test]
fn negative_integer_literal() {
    // The grammar may or may not support negative literals directly.
    // We check it doesn't panic.
    let source = "wire x = cycle~(-440);";
    let _ = parse(source);
}

#[test]
fn float_literal() {
    let source = "wire x = mul~(cycle~(440), 0.5);";
    let prog = parse(source).unwrap();
    // The outer call is mul~
    match &prog.wires[0].value {
        flutmax_ast::Expr::Call { args, .. } => {
            assert_eq!(
                args[1].value,
                flutmax_ast::Expr::Lit(flutmax_ast::LitValue::Float(0.5))
            );
        }
        _ => panic!("expected Call"),
    }
}

#[test]
fn zero_literal() {
    let source = "wire x = cycle~(0);";
    let prog = parse(source).unwrap();
    match &prog.wires[0].value {
        flutmax_ast::Expr::Call { args, .. } => {
            assert_eq!(
                args[0].value,
                flutmax_ast::Expr::Lit(flutmax_ast::LitValue::Int(0))
            );
        }
        _ => panic!("expected Call"),
    }
}

#[test]
fn large_integer_literal() {
    let source = "wire x = cycle~(48000);";
    let prog = parse(source).unwrap();
    match &prog.wires[0].value {
        flutmax_ast::Expr::Call { args, .. } => {
            assert_eq!(
                args[0].value,
                flutmax_ast::Expr::Lit(flutmax_ast::LitValue::Int(48000))
            );
        }
        _ => panic!("expected Call"),
    }
}

#[test]
fn string_literal_basic() {
    let source = r#"wire x = print("hello");"#;
    let prog = parse(source).unwrap();
    match &prog.wires[0].value {
        flutmax_ast::Expr::Call { args, .. } => {
            assert_eq!(
                args[0].value,
                flutmax_ast::Expr::Lit(flutmax_ast::LitValue::Str("hello".to_string()))
            );
        }
        _ => panic!("expected Call"),
    }
}

#[test]
fn string_literal_with_spaces() {
    let source = r#"wire x = print("hello world");"#;
    let prog = parse(source).unwrap();
    match &prog.wires[0].value {
        flutmax_ast::Expr::Call { args, .. } => {
            assert_eq!(
                args[0].value,
                flutmax_ast::Expr::Lit(flutmax_ast::LitValue::Str("hello world".to_string()))
            );
        }
        _ => panic!("expected Call"),
    }
}

// ─── Multiple arguments ───

#[test]
fn zero_args() {
    let source = "wire x = button();";
    let prog = parse(source).unwrap();
    match &prog.wires[0].value {
        flutmax_ast::Expr::Call { args, .. } => {
            assert_eq!(args.len(), 0);
        }
        _ => panic!("expected Call"),
    }
}

#[test]
fn one_arg() {
    let source = "wire x = cycle~(440);";
    let prog = parse(source).unwrap();
    match &prog.wires[0].value {
        flutmax_ast::Expr::Call { args, .. } => {
            assert_eq!(args.len(), 1);
        }
        _ => panic!("expected Call"),
    }
}

#[test]
fn two_args() {
    let source = "wire x = mul~(a, b);";
    // This would fail at compile (undefined refs) but should parse fine
    let prog = parse(source).unwrap();
    match &prog.wires[0].value {
        flutmax_ast::Expr::Call { args, .. } => {
            assert_eq!(args.len(), 2);
        }
        _ => panic!("expected Call"),
    }
}

#[test]
fn three_args() {
    let source = "wire x = biquad~(a, b, c);";
    let prog = parse(source).unwrap();
    match &prog.wires[0].value {
        flutmax_ast::Expr::Call { args, .. } => {
            assert_eq!(args.len(), 3);
        }
        _ => panic!("expected Call"),
    }
}

#[test]
fn six_args() {
    let source = "wire x = biquad~(a, b, c, d, e, f);";
    let prog = parse(source).unwrap();
    match &prog.wires[0].value {
        flutmax_ast::Expr::Call { args, .. } => {
            assert_eq!(args.len(), 6);
        }
        _ => panic!("expected Call"),
    }
}

#[test]
fn mixed_arg_types() {
    let source = r#"wire x = foo(a, 440, 0.5, "hello");"#;
    let prog = parse(source).unwrap();
    match &prog.wires[0].value {
        flutmax_ast::Expr::Call { args, .. } => {
            assert_eq!(args.len(), 4);
            assert!(matches!(args[0].value, flutmax_ast::Expr::Ref(_)));
            assert!(matches!(
                args[1].value,
                flutmax_ast::Expr::Lit(flutmax_ast::LitValue::Int(440))
            ));
            assert!(matches!(
                args[2].value,
                flutmax_ast::Expr::Lit(flutmax_ast::LitValue::Float(_))
            ));
            assert!(matches!(
                args[3].value,
                flutmax_ast::Expr::Lit(flutmax_ast::LitValue::Str(_))
            ));
        }
        _ => panic!("expected Call"),
    }
}

// ─── Object name variants ───

#[test]
fn tilde_object_name() {
    let source = "wire x = cycle~(440);";
    let prog = parse(source).unwrap();
    match &prog.wires[0].value {
        flutmax_ast::Expr::Call { object, .. } => {
            assert_eq!(object, "cycle~");
        }
        _ => panic!("expected Call"),
    }
}

#[test]
fn operator_tilde_star() {
    let source = "wire x = mul~(a, b);";
    let prog = parse(source).unwrap();
    match &prog.wires[0].value {
        flutmax_ast::Expr::Call { object, .. } => {
            assert_eq!(object, "mul~");
        }
        _ => panic!("expected Call"),
    }
}

#[test]
fn plain_object_name() {
    let source = "wire x = button();";
    let prog = parse(source).unwrap();
    match &prog.wires[0].value {
        flutmax_ast::Expr::Call { object, .. } => {
            assert_eq!(object, "button");
        }
        _ => panic!("expected Call"),
    }
}

// ─── Direct connections ───

#[test]
fn single_direct_connection() {
    let source = "node_a.in[0] = trigger;";
    let prog = parse(source).unwrap();
    assert_eq!(prog.direct_connections.len(), 1);
    assert_eq!(prog.direct_connections[0].target.object, "node_a");
    assert_eq!(prog.direct_connections[0].target.index, 0);
}

#[test]
fn multiple_direct_connections() {
    let source = r#"
wire trigger = button();
node_a.in[0] = trigger;
node_b.in[0] = trigger;
node_c.in[2] = trigger;
"#;
    let prog = parse(source).unwrap();
    assert_eq!(prog.direct_connections.len(), 3);
    assert_eq!(prog.direct_connections[2].target.object, "node_c");
    assert_eq!(prog.direct_connections[2].target.index, 2);
}

// ─── Out assignments ───

#[test]
fn out_assignment_index_0() {
    let source = "out 0 (audio): signal;\nwire x = cycle~(440);\nout[0] = x;";
    let prog = parse(source).unwrap();
    assert_eq!(prog.out_assignments.len(), 1);
    assert_eq!(prog.out_assignments[0].index, 0);
}

#[test]
fn out_assignment_index_1() {
    let source = r#"
out 0 (left): signal;
out 1 (right): signal;
wire x = cycle~(440);
out[0] = x;
out[1] = x;
"#;
    let prog = parse(source).unwrap();
    assert_eq!(prog.out_assignments.len(), 2);
    assert_eq!(prog.out_assignments[0].index, 0);
    assert_eq!(prog.out_assignments[1].index, 1);
}

// ─── Span information ───

#[test]
fn wire_span_is_populated() {
    let source = "wire x = cycle~(440);";
    let prog = parse(source).unwrap();
    let span = prog.wires[0].span.as_ref().unwrap();
    assert_eq!(span.start_line, 1);
    assert_eq!(span.start_column, 1);
}

#[test]
fn multi_line_span() {
    let source = "wire a = cycle~(440);\nwire b = mul~(a, 0.5);";
    let prog = parse(source).unwrap();
    let span_a = prog.wires[0].span.as_ref().unwrap();
    let span_b = prog.wires[1].span.as_ref().unwrap();
    assert_eq!(span_a.start_line, 1);
    assert_eq!(span_b.start_line, 2);
}

// ─── Error handling ───

#[test]
fn invalid_syntax_wire_no_value() {
    let source = "wire a = ;";
    let result = parse(source);
    assert!(result.is_err(), "should fail on 'wire a = ;'");
}

#[test]
fn invalid_syntax_missing_semicolon() {
    let source = "wire a = cycle~(440)";
    let result = parse(source);
    // May or may not error depending on grammar, but should not panic
    let _ = result;
}

#[test]
fn invalid_syntax_unknown_keyword() {
    let source = "let x = cycle~(440);";
    let result = parse(source);
    // Should either error or parse as something (depends on grammar)
    // Key: it must not panic
    let _ = result;
}

#[test]
fn completely_empty_input() {
    let prog = parse("").unwrap();
    assert!(prog.wires.is_empty());
    assert!(prog.in_decls.is_empty());
    assert!(prog.out_decls.is_empty());
    assert!(prog.out_assignments.is_empty());
    assert!(prog.direct_connections.is_empty());
}

#[test]
fn whitespace_only_input() {
    let prog = parse("   \n  \n  ").unwrap();
    assert!(prog.wires.is_empty());
}

// ─── Comprehensive program ───

#[test]
fn full_program_with_all_features() {
    let source = r#"
// FM Synthesizer
in 0 (freq): float;
in 1 (mod_ratio): float;
in 2 (mod_depth): float;
out 0 (audio): signal;

wire mod_freq = mul(freq, mod_ratio);
wire modulator = cycle~(mod_freq);
wire mod_scaled = mul~(modulator, mod_depth);
wire carrier = cycle~(freq);
wire output = mul~(carrier, 0.5);
out[0] = output;
"#;
    let prog = parse(source).unwrap();
    assert_eq!(prog.in_decls.len(), 3);
    assert_eq!(prog.out_decls.len(), 1);
    assert_eq!(prog.wires.len(), 5);
    assert_eq!(prog.out_assignments.len(), 1);
}
