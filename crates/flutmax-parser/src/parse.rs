use flutmax_ast::{
    AttrPair, AttrValue, CallArg, DestructuringWire, DirectConnection, Expr, FeedbackAssignment,
    FeedbackDecl, InDecl, InputPortAccess, LitValue, MsgDecl, OutAssignment, OutDecl,
    OutputPortAccess, PortType, Program, Span, StateAssignment, StateDecl, Wire,
};
use tree_sitter::{Node, Parser};
use tree_sitter_flutmax::LANGUAGE;

#[derive(Debug)]
pub enum ParseError {
    TreeSitter(String),
    InvalidSyntax {
        message: String,
        line: usize,
        column: usize,
    },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::TreeSitter(msg) => write!(f, "Tree-sitter error: {}", msg),
            ParseError::InvalidSyntax {
                message,
                line,
                column,
            } => {
                write!(f, "Syntax error at {}:{}: {}", line, column, message)
            }
        }
    }
}

impl std::error::Error for ParseError {}

/// Parse a .flutmax source string into an AST Program.
pub fn parse(source: &str) -> Result<Program, ParseError> {
    let mut parser = Parser::new();
    parser
        .set_language(&LANGUAGE.into())
        .map_err(|e| ParseError::TreeSitter(e.to_string()))?;

    let tree = parser
        .parse(source, None)
        .ok_or_else(|| ParseError::TreeSitter("Failed to parse source".to_string()))?;

    let root = tree.root_node();

    // Check for parse errors in the tree
    if root.has_error() {
        if let Some(err_node) = find_error_node(root) {
            return Err(ParseError::InvalidSyntax {
                message: format!("Unexpected syntax near '{}'", node_text(err_node, source)),
                line: err_node.start_position().row + 1,
                column: err_node.start_position().column + 1,
            });
        }
    }

    let mut program = Program::new();
    let mut implicit_in_index: u32 = 0;
    let mut implicit_out_index: u32 = 0;

    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        match child.kind() {
            "port_declaration" => {
                convert_port_declaration(child, source, &mut program, &mut implicit_in_index, &mut implicit_out_index)?;
            }
            "destructuring_wire" => {
                let dw = convert_destructuring_wire(child, source)?;
                program.destructuring_wires.push(dw);
            }
            "wire_declaration" => {
                let wire = convert_wire_declaration(child, source)?;
                program.wires.push(wire);
            }
            "out_assignment" => {
                let assignment = convert_out_assignment(child, source)?;
                program.out_assignments.push(assignment);
            }
            "direct_connection" => {
                let connection = convert_direct_connection(child, source)?;
                program.direct_connections.push(connection);
            }
            "feedback_declaration" => {
                let decl = convert_feedback_declaration(child, source)?;
                program.feedback_decls.push(decl);
            }
            "feedback_assignment" => {
                let assign = convert_feedback_assignment(child, source)?;
                program.feedback_assignments.push(assign);
            }
            "msg_declaration" => {
                let md = convert_msg_declaration(child, source)?;
                program.msg_decls.push(md);
            }
            "state_declaration" => {
                let decl = convert_state_declaration(child, source)?;
                program.state_decls.push(decl);
            }
            "state_assignment" => {
                let assign = convert_state_assignment(child, source)?;
                program.state_assignments.push(assign);
            }
            _ => {
                // Skip comments and other unnamed nodes
            }
        }
    }

    Ok(program)
}

/// Recursively find the first ERROR node in the tree.
fn find_error_node(node: Node) -> Option<Node> {
    if node.is_error() || node.is_missing() {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(err) = find_error_node(child) {
            return Some(err);
        }
    }
    None
}

/// Extract the text of a node from the source.
fn node_text<'a>(node: Node, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}

/// Convert a port_declaration CST node into either InDecl or OutDecl.
/// Supports both explicit (`in 0 (name): type;`) and implicit (`in name: type;`) syntax.
fn convert_port_declaration(
    node: Node,
    source: &str,
    program: &mut Program,
    implicit_in_index: &mut u32,
    implicit_out_index: &mut u32,
) -> Result<(), ParseError> {
    let direction_node = required_field(node, "direction", source)?;
    let name_node = required_field(node, "name", source)?;
    let type_node = required_field(node, "type", source)?;

    let direction = node_text(direction_node, source);
    let name = node_text(name_node, source).to_string();
    let type_text = node_text(type_node, source);

    // Explicit index is optional: present in `in 0 (name): type;`, absent in `in name: type;`
    let index = if let Some(index_node) = node.child_by_field_name("index") {
        parse_u32(index_node, source)?
    } else {
        match direction {
            "in" => {
                let idx = *implicit_in_index;
                *implicit_in_index += 1;
                idx
            }
            "out" => {
                let idx = *implicit_out_index;
                *implicit_out_index += 1;
                idx
            }
            _ => 0,
        }
    };

    let port_type = PortType::from_str(type_text).ok_or_else(|| ParseError::InvalidSyntax {
        message: format!("Unknown port type '{}'", type_text),
        line: type_node.start_position().row + 1,
        column: type_node.start_position().column + 1,
    })?;

    // Check for optional inline value expression (for out declarations: `out audio: signal = expr;`)
    let value = if direction == "out" {
        if let Some(value_node) = node.child_by_field_name("value") {
            Some(convert_expression(value_node, source)?)
        } else {
            None
        }
    } else {
        None
    };

    match direction {
        "in" => {
            program.in_decls.push(InDecl {
                index,
                name,
                port_type,
            });
        }
        "out" => {
            program.out_decls.push(OutDecl {
                index,
                name,
                port_type,
                value,
            });
        }
        _ => {
            return Err(ParseError::InvalidSyntax {
                message: format!("Expected 'in' or 'out', got '{}'", direction),
                line: direction_node.start_position().row + 1,
                column: direction_node.start_position().column + 1,
            });
        }
    }

    Ok(())
}

/// Generate a Span from a CST node.
fn make_span(node: Node) -> Option<Span> {
    Some(Span {
        start_line: node.start_position().row + 1,
        start_column: node.start_position().column + 1,
        end_line: node.end_position().row + 1,
        end_column: node.end_position().column + 1,
    })
}

/// Convert a destructuring_wire CST node into a DestructuringWire.
fn convert_destructuring_wire(node: Node, source: &str) -> Result<DestructuringWire, ParseError> {
    let names_node = required_field(node, "names", source)?;
    let value_node = required_field(node, "value", source)?;

    let mut names = Vec::new();
    let mut cursor = names_node.walk();
    for child in names_node.named_children(&mut cursor) {
        if child.kind() == "plain_identifier" {
            names.push(node_text(child, source).to_string());
        }
    }

    let value = convert_expression(value_node, source)?;
    let span = make_span(node);

    Ok(DestructuringWire { names, value, span })
}

/// Convert a wire_declaration CST node into a Wire.
fn convert_wire_declaration(node: Node, source: &str) -> Result<Wire, ParseError> {
    let name_node = required_field(node, "name", source)?;
    let value_node = required_field(node, "value", source)?;

    let name = node_text(name_node, source).to_string();
    let value = convert_expression(value_node, source)?;
    let span = make_span(node);

    let attrs = if let Some(attr_node) = node.child_by_field_name("attrs") {
        convert_attr_chain(attr_node, source)?
    } else {
        vec![]
    };

    Ok(Wire {
        name,
        value,
        span,
        attrs,
    })
}

/// Convert an out_assignment CST node into an OutAssignment.
fn convert_out_assignment(node: Node, source: &str) -> Result<OutAssignment, ParseError> {
    let index_node = required_field(node, "index", source)?;
    let value_node = required_field(node, "value", source)?;

    let index = parse_u32(index_node, source)?;
    let value = convert_expression(value_node, source)?;
    let span = make_span(node);

    Ok(OutAssignment { index, value, span })
}

/// Convert a direct_connection CST node into a DirectConnection.
fn convert_direct_connection(node: Node, source: &str) -> Result<DirectConnection, ParseError> {
    let target_node = required_field(node, "target", source)?;
    let value_node = required_field(node, "value", source)?;

    let target = convert_input_port_access(target_node, source)?;
    let value = convert_expression(value_node, source)?;

    Ok(DirectConnection { target, value })
}

/// Convert a feedback_declaration CST node into a FeedbackDecl.
fn convert_feedback_declaration(node: Node, source: &str) -> Result<FeedbackDecl, ParseError> {
    let name_node = required_field(node, "name", source)?;
    let type_node = required_field(node, "type", source)?;

    let name = node_text(name_node, source).to_string();
    let type_text = node_text(type_node, source);

    let port_type = PortType::from_str(type_text).ok_or_else(|| ParseError::InvalidSyntax {
        message: format!("Unknown port type '{}'", type_text),
        line: type_node.start_position().row + 1,
        column: type_node.start_position().column + 1,
    })?;

    let span = make_span(node);

    Ok(FeedbackDecl {
        name,
        port_type,
        span,
    })
}

/// Convert a feedback_assignment CST node into a FeedbackAssignment.
fn convert_feedback_assignment(node: Node, source: &str) -> Result<FeedbackAssignment, ParseError> {
    let target_node = required_field(node, "target", source)?;
    let value_node = required_field(node, "value", source)?;

    let target = node_text(target_node, source).to_string();
    let value = convert_expression(value_node, source)?;
    let span = make_span(node);

    Ok(FeedbackAssignment {
        target,
        value,
        span,
    })
}

/// Convert a msg_declaration CST node into a MsgDecl.
fn convert_msg_declaration(node: Node, source: &str) -> Result<MsgDecl, ParseError> {
    let name_node = required_field(node, "name", source)?;
    let content_node = required_field(node, "content", source)?;

    let name = node_text(name_node, source).to_string();

    // Extract string content (strip quotes and unescape)
    let raw = node_text(content_node, source);
    let inner = &raw[1..raw.len() - 1];
    let content = unescape_string(inner);

    let span = make_span(node);

    let attrs = if let Some(attr_node) = node.child_by_field_name("attrs") {
        convert_attr_chain(attr_node, source)?
    } else {
        vec![]
    };

    Ok(MsgDecl {
        name,
        content,
        span,
        attrs,
    })
}

/// Convert a state_declaration CST node into a StateDecl.
fn convert_state_declaration(node: Node, source: &str) -> Result<StateDecl, ParseError> {
    let name_node = required_field(node, "name", source)?;
    let type_node = required_field(node, "type", source)?;
    let init_node = required_field(node, "init", source)?;

    let name = node_text(name_node, source).to_string();
    let type_text = node_text(type_node, source);

    let port_type = PortType::from_str(type_text).ok_or_else(|| ParseError::InvalidSyntax {
        message: format!("Unknown control type '{}'", type_text),
        line: type_node.start_position().row + 1,
        column: type_node.start_position().column + 1,
    })?;

    let init_value = convert_expression(init_node, source)?;
    let span = make_span(node);

    Ok(StateDecl {
        name,
        port_type,
        init_value,
        span,
    })
}

/// Convert a state_assignment CST node into a StateAssignment.
fn convert_state_assignment(node: Node, source: &str) -> Result<StateAssignment, ParseError> {
    let target_node = required_field(node, "target", source)?;
    let value_node = required_field(node, "value", source)?;

    let name = node_text(target_node, source).to_string();
    let value = convert_expression(value_node, source)?;
    let span = make_span(node);

    Ok(StateAssignment { name, value, span })
}

/// Convert an input_port_access CST node into an InputPortAccess.
fn convert_input_port_access(node: Node, source: &str) -> Result<InputPortAccess, ParseError> {
    let object_node = required_field(node, "object", source)?;
    let index_node = required_field(node, "index", source)?;

    let object = node_text(object_node, source).to_string();
    let index = parse_u32(index_node, source)?;

    Ok(InputPortAccess { object, index })
}

/// Convert an output_port_access CST node into an OutputPortAccess.
fn convert_output_port_access(node: Node, source: &str) -> Result<OutputPortAccess, ParseError> {
    let object_node = required_field(node, "object", source)?;
    let index_node = required_field(node, "index", source)?;

    let object = node_text(object_node, source).to_string();
    let index = parse_u32(index_node, source)?;

    Ok(OutputPortAccess { object, index })
}

/// Convert an _expression CST node into an Expr.
fn convert_expression(node: Node, source: &str) -> Result<Expr, ParseError> {
    match node.kind() {
        "object_call" => convert_object_call(node, source),
        "output_port_access" => {
            let opa = convert_output_port_access(node, source)?;
            Ok(Expr::OutputPortAccess(opa))
        }
        "tuple_expression" => {
            let mut elements = Vec::new();
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                elements.push(convert_expression(child, source)?);
            }
            Ok(Expr::Tuple(elements))
        }
        "identifier" => {
            // identifier has a child that is either tilde_identifier or plain_identifier
            let text = node_text(node, source).to_string();
            Ok(Expr::Ref(text))
        }
        "number" => {
            let text = node_text(node, source);
            if text.contains('.') || text.contains('e') || text.contains('E') {
                let val: f64 = text.parse().map_err(|_| ParseError::InvalidSyntax {
                    message: format!("Invalid float literal '{}'", text),
                    line: node.start_position().row + 1,
                    column: node.start_position().column + 1,
                })?;
                Ok(Expr::Lit(LitValue::Float(val)))
            } else {
                let val: i64 = text.parse().map_err(|_| ParseError::InvalidSyntax {
                    message: format!("Invalid integer literal '{}'", text),
                    line: node.start_position().row + 1,
                    column: node.start_position().column + 1,
                })?;
                Ok(Expr::Lit(LitValue::Int(val)))
            }
        }
        "string" => {
            let raw = node_text(node, source);
            // Strip surrounding quotes
            let inner = &raw[1..raw.len() - 1];
            // Handle basic escape sequences
            let unescaped = unescape_string(inner);
            Ok(Expr::Lit(LitValue::Str(unescaped)))
        }
        _ => Err(ParseError::InvalidSyntax {
            message: format!("Unexpected expression kind '{}'", node.kind()),
            line: node.start_position().row + 1,
            column: node.start_position().column + 1,
        }),
    }
}

/// Convert an object_call CST node into an Expr::Call.
fn convert_object_call(node: Node, source: &str) -> Result<Expr, ParseError> {
    let object_node = required_field(node, "object", source)?;

    // object_name contains a child (tilde_identifier, operator_tilde, or plain_identifier)
    // We just extract the full text of the object_name node.
    let object = node_text(object_node, source).to_string();

    let mut args = Vec::new();

    if let Some(arg_list_node) = node.child_by_field_name("arguments") {
        let mut cursor = arg_list_node.walk();
        for arg_child in arg_list_node.named_children(&mut cursor) {
            let expr = convert_expression(arg_child, source)?;
            args.push(CallArg::positional(expr));
        }
    }

    Ok(Expr::Call { object, args })
}

/// Convert an attr_chain CST node into a Vec<AttrPair>.
fn convert_attr_chain(node: Node, source: &str) -> Result<Vec<AttrPair>, ParseError> {
    // attr_chain has a "pairs" field pointing to attr_list
    let pairs_node = required_field(node, "pairs", source)?;
    convert_attr_list(pairs_node, source)
}

/// Convert an attr_list CST node into a Vec<AttrPair>.
fn convert_attr_list(node: Node, source: &str) -> Result<Vec<AttrPair>, ParseError> {
    let mut pairs = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "attr_pair" {
            pairs.push(convert_attr_pair(child, source)?);
        }
    }
    Ok(pairs)
}

/// Convert an attr_pair CST node into an AttrPair.
fn convert_attr_pair(node: Node, source: &str) -> Result<AttrPair, ParseError> {
    let key_node = required_field(node, "key", source)?;
    let value_node = required_field(node, "value", source)?;

    let key = node_text(key_node, source).to_string();
    let value = convert_attr_value(value_node, source)?;

    Ok(AttrPair { key, value })
}

/// Convert an _attr_value CST node into an AttrValue.
fn convert_attr_value(node: Node, source: &str) -> Result<AttrValue, ParseError> {
    match node.kind() {
        "number" => {
            let text = node_text(node, source);
            if text.contains('.') || text.contains('e') || text.contains('E') {
                // Trailing-dot floats (e.g., "0.", "100.") are valid
                let val: f64 = text.parse().map_err(|_| ParseError::InvalidSyntax {
                    message: format!("Invalid float literal '{}'", text),
                    line: node.start_position().row + 1,
                    column: node.start_position().column + 1,
                })?;
                Ok(AttrValue::Float(val))
            } else {
                let val: i64 = text.parse().map_err(|_| ParseError::InvalidSyntax {
                    message: format!("Invalid integer literal '{}'", text),
                    line: node.start_position().row + 1,
                    column: node.start_position().column + 1,
                })?;
                Ok(AttrValue::Int(val))
            }
        }
        "string" => {
            let raw = node_text(node, source);
            let inner = &raw[1..raw.len() - 1];
            let unescaped = unescape_string(inner);
            Ok(AttrValue::Str(unescaped))
        }
        "plain_identifier" => {
            let text = node_text(node, source).to_string();
            Ok(AttrValue::Ident(text))
        }
        _ => Err(ParseError::InvalidSyntax {
            message: format!("Unexpected attr value kind '{}'", node.kind()),
            line: node.start_position().row + 1,
            column: node.start_position().column + 1,
        }),
    }
}

/// Unescape basic string escape sequences.
fn unescape_string(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some('\\') => result.push('\\'),
                Some('"') => result.push('"'),
                Some(other) => {
                    result.push('\\');
                    result.push(other);
                }
                None => result.push('\\'),
            }
        } else {
            result.push(ch);
        }
    }
    result
}

/// Get a required field from a node, returning ParseError if missing.
fn required_field<'a>(
    node: Node<'a>,
    field_name: &str,
    source: &str,
) -> Result<Node<'a>, ParseError> {
    node.child_by_field_name(field_name)
        .ok_or_else(|| ParseError::InvalidSyntax {
            message: format!(
                "Missing required field '{}' in '{}'",
                field_name,
                node_text(node, source)
            ),
            line: node.start_position().row + 1,
            column: node.start_position().column + 1,
        })
}

/// Parse a node's text as u32.
fn parse_u32(node: Node, source: &str) -> Result<u32, ParseError> {
    let text = node_text(node, source);
    text.parse().map_err(|_| ParseError::InvalidSyntax {
        message: format!("Expected integer, got '{}'", text),
        line: node.start_position().row + 1,
        column: node.start_position().column + 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flutmax_ast::{DirectConnection, Expr, InDecl, InputPortAccess, LitValue, OutDecl, PortType};

    #[test]
    fn test_l1_minimal() {
        let source = r#"
out 0 (audio): signal;
wire osc = cycle~(440);
out[0] = osc;
"#;
        let prog = parse(source).expect("parse failed");

        // out_decls
        assert_eq!(prog.out_decls.len(), 1);
        assert_eq!(
            prog.out_decls[0],
            OutDecl {
                index: 0,
                name: "audio".to_string(),
                port_type: PortType::Signal,
                value: None,
            }
        );

        // wires
        assert_eq!(prog.wires.len(), 1);
        assert_eq!(prog.wires[0].name, "osc");
        assert_eq!(
            prog.wires[0].value,
            Expr::Call {
                object: "cycle~".to_string(),
                args: vec![CallArg::positional(Expr::Lit(LitValue::Int(440)))],
            }
        );
        assert!(prog.wires[0].span.is_some());

        // out_assignments
        assert_eq!(prog.out_assignments.len(), 1);
        assert_eq!(prog.out_assignments[0].index, 0);
        assert_eq!(
            prog.out_assignments[0].value,
            Expr::Ref("osc".to_string())
        );
        assert!(prog.out_assignments[0].span.is_some());

        // No in_decls or direct_connections
        assert!(prog.in_decls.is_empty());
        assert!(prog.direct_connections.is_empty());
    }

    #[test]
    fn test_l2_simple_synth() {
        let source = r#"
// L2_simple_synth.flutmax
in 0 (freq): float;
out 0 (audio): signal;

wire osc = cycle~(freq);
wire amp = mul~(osc, 0.5);

out[0] = amp;
"#;
        let prog = parse(source).expect("parse failed");

        // in_decls
        assert_eq!(prog.in_decls.len(), 1);
        assert_eq!(
            prog.in_decls[0],
            InDecl {
                index: 0,
                name: "freq".to_string(),
                port_type: PortType::Float,
            }
        );

        // out_decls
        assert_eq!(prog.out_decls.len(), 1);
        assert_eq!(
            prog.out_decls[0],
            OutDecl {
                index: 0,
                name: "audio".to_string(),
                port_type: PortType::Signal,
                value: None,
            }
        );

        // wires
        assert_eq!(prog.wires.len(), 2);
        assert_eq!(prog.wires[0].name, "osc");
        assert_eq!(
            prog.wires[0].value,
            Expr::Call {
                object: "cycle~".to_string(),
                args: vec![CallArg::positional(Expr::Ref("freq".to_string()))],
            }
        );
        assert!(prog.wires[0].span.is_some());
        assert_eq!(prog.wires[1].name, "amp");
        assert_eq!(
            prog.wires[1].value,
            Expr::Call {
                object: "mul~".to_string(),
                args: vec![
                    CallArg::positional(Expr::Ref("osc".to_string())),
                    CallArg::positional(Expr::Lit(LitValue::Float(0.5))),
                ],
            }
        );
        assert!(prog.wires[1].span.is_some());

        // out_assignments
        assert_eq!(prog.out_assignments.len(), 1);
        assert_eq!(prog.out_assignments[0].index, 0);
        assert_eq!(
            prog.out_assignments[0].value,
            Expr::Ref("amp".to_string())
        );
        assert!(prog.out_assignments[0].span.is_some());
    }

    #[test]
    fn test_l3b_control_fanout() {
        let source = r#"
wire trigger = button();
wire counter = counter(trigger);
wire msg = print(counter);

node_a.in[0] = trigger;
node_b.in[0] = trigger;
"#;
        let prog = parse(source).expect("parse failed");

        // wires
        assert_eq!(prog.wires.len(), 3);
        assert_eq!(prog.wires[0].name, "trigger");
        assert_eq!(
            prog.wires[0].value,
            Expr::Call {
                object: "button".to_string(),
                args: vec![],
            }
        );
        assert_eq!(prog.wires[1].name, "counter");
        assert_eq!(
            prog.wires[1].value,
            Expr::Call {
                object: "counter".to_string(),
                args: vec![CallArg::positional(Expr::Ref("trigger".to_string()))],
            }
        );
        assert_eq!(prog.wires[2].name, "msg");
        assert_eq!(
            prog.wires[2].value,
            Expr::Call {
                object: "print".to_string(),
                args: vec![CallArg::positional(Expr::Ref("counter".to_string()))],
            }
        );

        // direct_connections
        assert_eq!(prog.direct_connections.len(), 2);
        assert_eq!(
            prog.direct_connections[0],
            DirectConnection {
                target: InputPortAccess {
                    object: "node_a".to_string(),
                    index: 0,
                },
                value: Expr::Ref("trigger".to_string()),
            }
        );
        assert_eq!(
            prog.direct_connections[1],
            DirectConnection {
                target: InputPortAccess {
                    object: "node_b".to_string(),
                    index: 0,
                },
                value: Expr::Ref("trigger".to_string()),
            }
        );
    }

    #[test]
    fn test_multiple_in_out_ports() {
        let source = r#"
in 0 (input_sig): signal;
in 1 (cutoff): float;
in 2 (q_factor): float;

out 0 (lowpass): signal;
out 1 (highpass): signal;
"#;
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.in_decls.len(), 3);
        assert_eq!(prog.in_decls[0].name, "input_sig");
        assert_eq!(prog.in_decls[0].port_type, PortType::Signal);
        assert_eq!(prog.in_decls[1].name, "cutoff");
        assert_eq!(prog.in_decls[1].port_type, PortType::Float);
        assert_eq!(prog.in_decls[2].name, "q_factor");
        assert_eq!(prog.in_decls[2].port_type, PortType::Float);

        assert_eq!(prog.out_decls.len(), 2);
        assert_eq!(prog.out_decls[0].name, "lowpass");
        assert_eq!(prog.out_decls[1].name, "highpass");
    }

    #[test]
    fn test_nested_object_call() {
        let source = r#"
wire sig = biquad~(cycle~(440), 1000, 0.7);
"#;
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.wires.len(), 1);
        assert_eq!(prog.wires[0].name, "sig");
        assert_eq!(
            prog.wires[0].value,
            Expr::Call {
                object: "biquad~".to_string(),
                args: vec![
                    CallArg::positional(Expr::Call {
                        object: "cycle~".to_string(),
                        args: vec![CallArg::positional(Expr::Lit(LitValue::Int(440)))],
                    }),
                    CallArg::positional(Expr::Lit(LitValue::Int(1000))),
                    CallArg::positional(Expr::Lit(LitValue::Float(0.7))),
                ],
            }
        );
        assert!(prog.wires[0].span.is_some());
    }

    #[test]
    fn test_string_literal() {
        let source = r#"
wire msg = print("hello world");
"#;
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.wires.len(), 1);
        assert_eq!(
            prog.wires[0].value,
            Expr::Call {
                object: "print".to_string(),
                args: vec![CallArg::positional(Expr::Lit(LitValue::Str("hello world".to_string())))],
            }
        );
    }

    #[test]
    fn test_zero_arg_object_call() {
        let source = r#"
wire btn = button();
"#;
        let prog = parse(source).expect("parse failed");

        assert_eq!(
            prog.wires[0].value,
            Expr::Call {
                object: "button".to_string(),
                args: vec![],
            }
        );
    }

    #[test]
    fn test_empty_source() {
        let source = "";
        let prog = parse(source).expect("parse failed");
        assert!(prog.in_decls.is_empty());
        assert!(prog.out_decls.is_empty());
        assert!(prog.wires.is_empty());
        assert!(prog.out_assignments.is_empty());
        assert!(prog.direct_connections.is_empty());
    }

    #[test]
    fn test_comments_ignored() {
        let source = r#"
// This is a comment
wire osc = cycle~(440);
// Another comment
out 0 (audio): signal;
"#;
        let prog = parse(source).expect("parse failed");
        assert_eq!(prog.wires.len(), 1);
        assert_eq!(prog.out_decls.len(), 1);
    }

    #[test]
    fn test_span_populated() {
        let source = "wire osc = cycle~(440);";
        let prog = parse(source).expect("parse failed");
        assert!(prog.wires[0].span.is_some());
        let span = prog.wires[0].span.as_ref().unwrap();
        assert_eq!(span.start_line, 1);
        assert_eq!(span.start_column, 1);
    }

    #[test]
    fn test_out_assignment_span_populated() {
        let source = "out 0 (audio): signal;\nwire osc = cycle~(440);\nout[0] = osc;";
        let prog = parse(source).expect("parse failed");
        assert!(prog.out_assignments[0].span.is_some());
        let span = prog.out_assignments[0].span.as_ref().unwrap();
        assert_eq!(span.start_line, 3);
    }

    // ─── Tuple / Destructuring tests ───

    #[test]
    fn test_tuple_expression() {
        let source = "wire t = (a, b, c);";
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.wires.len(), 1);
        assert_eq!(prog.wires[0].name, "t");
        assert_eq!(
            prog.wires[0].value,
            Expr::Tuple(vec![
                Expr::Ref("a".to_string()),
                Expr::Ref("b".to_string()),
                Expr::Ref("c".to_string()),
            ])
        );
    }

    #[test]
    fn test_tuple_two_elements() {
        let source = "wire pair = (x, y);";
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.wires.len(), 1);
        assert_eq!(
            prog.wires[0].value,
            Expr::Tuple(vec![
                Expr::Ref("x".to_string()),
                Expr::Ref("y".to_string()),
            ])
        );
    }

    #[test]
    fn test_tuple_with_literals() {
        let source = "wire nums = (1, 2, 3);";
        let prog = parse(source).expect("parse failed");

        assert_eq!(
            prog.wires[0].value,
            Expr::Tuple(vec![
                Expr::Lit(LitValue::Int(1)),
                Expr::Lit(LitValue::Int(2)),
                Expr::Lit(LitValue::Int(3)),
            ])
        );
    }

    #[test]
    fn test_destructuring_wire() {
        let source = "wire (a, b, c) = unpack(coords);";
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.destructuring_wires.len(), 1);
        let dw = &prog.destructuring_wires[0];
        assert_eq!(dw.names, vec!["a", "b", "c"]);
        assert_eq!(
            dw.value,
            Expr::Call {
                object: "unpack".to_string(),
                args: vec![CallArg::positional(Expr::Ref("coords".to_string()))],
            }
        );
        assert!(dw.span.is_some());
    }

    #[test]
    fn test_destructuring_wire_two_names() {
        let source = "wire (x, y) = data;";
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.destructuring_wires.len(), 1);
        let dw = &prog.destructuring_wires[0];
        assert_eq!(dw.names, vec!["x", "y"]);
        assert_eq!(dw.value, Expr::Ref("data".to_string()));
    }

    #[test]
    fn test_destructuring_wire_with_tuple_value() {
        let source = "wire (a, b) = (x, y);";
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.destructuring_wires.len(), 1);
        let dw = &prog.destructuring_wires[0];
        assert_eq!(dw.names, vec!["a", "b"]);
        assert_eq!(
            dw.value,
            Expr::Tuple(vec![
                Expr::Ref("x".to_string()),
                Expr::Ref("y".to_string()),
            ])
        );
    }

    #[test]
    fn test_l4_tuple_full() {
        let source = r#"
in 0 (x): float;
in 1 (y): float;
in 2 (z): float;
out 0 (coords): list;

wire packed = (x, y, z);
out[0] = packed;
"#;
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.in_decls.len(), 3);
        assert_eq!(prog.out_decls.len(), 1);
        assert_eq!(prog.wires.len(), 1);
        assert_eq!(prog.wires[0].name, "packed");
        assert_eq!(
            prog.wires[0].value,
            Expr::Tuple(vec![
                Expr::Ref("x".to_string()),
                Expr::Ref("y".to_string()),
                Expr::Ref("z".to_string()),
            ])
        );
        assert_eq!(prog.out_assignments.len(), 1);
    }

    #[test]
    fn test_l5_destructure_full() {
        let source = r#"
in 0 (coords): list;
out 0 (x): float;
out 1 (y): float;

wire (a, b) = unpack(coords);
out[0] = a;
out[1] = b;
"#;
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.in_decls.len(), 1);
        assert_eq!(prog.out_decls.len(), 2);
        assert_eq!(prog.destructuring_wires.len(), 1);
        assert_eq!(prog.destructuring_wires[0].names, vec!["a", "b"]);
        assert_eq!(prog.out_assignments.len(), 2);
    }

    // ─── Feedback tests ───

    #[test]
    fn test_feedback_declaration() {
        let source = "feedback fb: signal;";
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.feedback_decls.len(), 1);
        assert_eq!(prog.feedback_decls[0].name, "fb");
        assert_eq!(prog.feedback_decls[0].port_type, PortType::Signal);
        assert!(prog.feedback_decls[0].span.is_some());
    }

    #[test]
    fn test_feedback_assignment() {
        let source = "feedback fb = tapin~(mixed, 1000);";
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.feedback_assignments.len(), 1);
        assert_eq!(prog.feedback_assignments[0].target, "fb");
        assert_eq!(
            prog.feedback_assignments[0].value,
            Expr::Call {
                object: "tapin~".to_string(),
                args: vec![
                    CallArg::positional(Expr::Ref("mixed".to_string())),
                    CallArg::positional(Expr::Lit(LitValue::Int(1000))),
                ],
            }
        );
        assert!(prog.feedback_assignments[0].span.is_some());
    }

    #[test]
    fn test_feedback_full_patch() {
        let source = r#"
in 0 (input): signal;
out 0 (output): signal;

feedback fb: signal;
wire delayed = tapout~(fb, 500);
wire mixed = add~(input, mul~(delayed, 0.3));
feedback fb = tapin~(mixed, 1000);
out[0] = mixed;
"#;
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.in_decls.len(), 1);
        assert_eq!(prog.out_decls.len(), 1);
        assert_eq!(prog.feedback_decls.len(), 1);
        assert_eq!(prog.feedback_decls[0].name, "fb");
        assert_eq!(prog.feedback_decls[0].port_type, PortType::Signal);
        assert_eq!(prog.wires.len(), 2);
        assert_eq!(prog.wires[0].name, "delayed");
        assert_eq!(prog.wires[1].name, "mixed");
        assert_eq!(prog.feedback_assignments.len(), 1);
        assert_eq!(prog.feedback_assignments[0].target, "fb");
        assert_eq!(prog.out_assignments.len(), 1);
    }

    // ─── OutputPortAccess tests ───

    #[test]
    fn test_output_port_access_in_wire() {
        let source = "wire x = node.out[0];";
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.wires.len(), 1);
        assert_eq!(prog.wires[0].name, "x");
        match &prog.wires[0].value {
            Expr::OutputPortAccess(flutmax_ast::OutputPortAccess { object, index }) => {
                assert_eq!(object, "node");
                assert_eq!(*index, 0);
            }
            other => panic!("expected OutputPortAccess, got {:?}", other),
        }
    }

    #[test]
    fn test_output_port_access_in_call_arg() {
        let source = "wire y = mul~(node.out[0], 0.5);";
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.wires.len(), 1);
        if let Expr::Call { args, .. } = &prog.wires[0].value {
            assert_eq!(args.len(), 2);
            match &args[0].value {
                Expr::OutputPortAccess(flutmax_ast::OutputPortAccess { object, index }) => {
                    assert_eq!(object, "node");
                    assert_eq!(*index, 0);
                }
                other => panic!("expected OutputPortAccess, got {:?}", other),
            }
        } else {
            panic!("expected Call");
        }
    }

    #[test]
    fn test_output_port_access_higher_index() {
        let source = "wire z = node.out[2];";
        let prog = parse(source).expect("parse failed");

        match &prog.wires[0].value {
            Expr::OutputPortAccess(flutmax_ast::OutputPortAccess { object, index }) => {
                assert_eq!(object, "node");
                assert_eq!(*index, 2);
            }
            other => panic!("expected OutputPortAccess, got {:?}", other),
        }
    }

    #[test]
    fn test_input_port_access_in_direct_connection() {
        let source = "node_a.in[0] = trigger;";
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.direct_connections.len(), 1);
        assert_eq!(prog.direct_connections[0].target.object, "node_a");
        assert_eq!(prog.direct_connections[0].target.index, 0);
    }

    #[test]
    fn test_in_not_in_expression() {
        // .in[N] cannot be used as an expression -> parse error
        let source = "wire x = node.in[0];";
        let result = parse(source);
        assert!(result.is_err(), ".in[N] should not be valid in expression position");
    }

    // ─── State tests ───

    #[test]
    fn test_state_declaration_int() {
        let source = "state counter: int = 0;";
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.state_decls.len(), 1);
        assert_eq!(prog.state_decls[0].name, "counter");
        assert_eq!(prog.state_decls[0].port_type, PortType::Int);
        assert_eq!(prog.state_decls[0].init_value, Expr::Lit(LitValue::Int(0)));
        assert!(prog.state_decls[0].span.is_some());
    }

    #[test]
    fn test_state_declaration_float() {
        let source = "state volume: float = 0.5;";
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.state_decls.len(), 1);
        assert_eq!(prog.state_decls[0].name, "volume");
        assert_eq!(prog.state_decls[0].port_type, PortType::Float);
        assert_eq!(
            prog.state_decls[0].init_value,
            Expr::Lit(LitValue::Float(0.5))
        );
    }

    #[test]
    fn test_state_assignment() {
        let source = "state counter = next;";
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.state_assignments.len(), 1);
        assert_eq!(prog.state_assignments[0].name, "counter");
        assert_eq!(
            prog.state_assignments[0].value,
            Expr::Ref("next".to_string())
        );
        assert!(prog.state_assignments[0].span.is_some());
    }

    #[test]
    fn test_state_assignment_with_call() {
        let source = "state counter = add(counter, 1);";
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.state_assignments.len(), 1);
        assert_eq!(prog.state_assignments[0].name, "counter");
        assert_eq!(
            prog.state_assignments[0].value,
            Expr::Call {
                object: "add".to_string(),
                args: vec![
                    CallArg::positional(Expr::Ref("counter".to_string())),
                    CallArg::positional(Expr::Lit(LitValue::Int(1))),
                ],
            }
        );
    }

    #[test]
    fn test_state_full_counter_patch() {
        let source = r#"
state counter: int = 0;
wire next = add(counter, 1);
state counter = next;
out 0 (count): int;
out[0] = next;
"#;
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.state_decls.len(), 1);
        assert_eq!(prog.state_decls[0].name, "counter");
        assert_eq!(prog.state_decls[0].port_type, PortType::Int);
        assert_eq!(prog.wires.len(), 1);
        assert_eq!(prog.wires[0].name, "next");
        assert_eq!(prog.state_assignments.len(), 1);
        assert_eq!(prog.state_assignments[0].name, "counter");
        assert_eq!(prog.out_decls.len(), 1);
        assert_eq!(prog.out_assignments.len(), 1);
    }

    // ─── Dotted identifier tests ───

    #[test]
    fn test_dotted_identifier_object_call() {
        let source = r#"
wire vid = jit.gl.videoplane();
"#;
        let prog = parse(source).expect("parse failed");
        assert_eq!(prog.wires.len(), 1);
        assert_eq!(prog.wires[0].name, "vid");
        assert_eq!(
            prog.wires[0].value,
            Expr::Call {
                object: "jit.gl.videoplane".to_string(),
                args: vec![],
            }
        );
    }

    #[test]
    fn test_dotted_identifier_with_args() {
        let source = r#"
wire dial = live.dial(0.5);
"#;
        let prog = parse(source).expect("parse failed");
        assert_eq!(prog.wires.len(), 1);
        assert_eq!(
            prog.wires[0].value,
            Expr::Call {
                object: "live.dial".to_string(),
                args: vec![CallArg::positional(Expr::Lit(LitValue::Float(0.5)))],
            }
        );
    }

    #[test]
    fn test_dotted_identifier_does_not_conflict_with_port_access() {
        let source = r#"
wire node = cycle~(440);
wire x = node.out[0];
node.in[0] = 440;
"#;
        let prog = parse(source).expect("parse failed");
        assert_eq!(prog.wires.len(), 2);
        // Second wire should be OutputPortAccess, not a dotted identifier call
        assert_eq!(
            prog.wires[1].value,
            Expr::OutputPortAccess(OutputPortAccess {
                object: "node".to_string(),
                index: 0,
            })
        );
        // Direct connection with input port access
        assert_eq!(prog.direct_connections.len(), 1);
        assert_eq!(prog.direct_connections[0].target.object, "node");
        assert_eq!(prog.direct_connections[0].target.index, 0);
    }

    // ─── Hyphenated identifier tests ───

    #[test]
    fn test_hyphenated_identifier() {
        let source = r#"
wire x = drunk-walk(10);
"#;
        let prog = parse(source).expect("parse failed");
        assert_eq!(prog.wires.len(), 1);
        assert_eq!(
            prog.wires[0].value,
            Expr::Call {
                object: "drunk-walk".to_string(),
                args: vec![CallArg::positional(Expr::Lit(LitValue::Int(10)))],
            }
        );
    }

    // ─── Message declaration tests ───

    #[test]
    fn test_msg_declaration() {
        let source = r#"
msg click = "bang";
"#;
        let prog = parse(source).expect("parse failed");
        assert_eq!(prog.msg_decls.len(), 1);
        assert_eq!(prog.msg_decls[0].name, "click");
        assert_eq!(prog.msg_decls[0].content, "bang");
        assert!(prog.msg_decls[0].span.is_some());
    }

    #[test]
    fn test_msg_declaration_with_template() {
        let source = r#"
msg format = "set $1 $2";
"#;
        let prog = parse(source).expect("parse failed");
        assert_eq!(prog.msg_decls.len(), 1);
        assert_eq!(prog.msg_decls[0].name, "format");
        assert_eq!(prog.msg_decls[0].content, "set $1 $2");
    }

    #[test]
    fn test_msg_declaration_multiple() {
        let source = r#"
msg bang_msg = "bang";
msg set_msg = "set 42";
"#;
        let prog = parse(source).expect("parse failed");
        assert_eq!(prog.msg_decls.len(), 2);
        assert_eq!(prog.msg_decls[0].name, "bang_msg");
        assert_eq!(prog.msg_decls[0].content, "bang");
        assert_eq!(prog.msg_decls[1].name, "set_msg");
        assert_eq!(prog.msg_decls[1].content, "set 42");
    }

    #[test]
    fn test_msg_with_wire_and_connection() {
        let source = r#"
msg click = "bang";
wire btn = button();
btn.in[0] = click;
"#;
        let prog = parse(source).expect("parse failed");
        assert_eq!(prog.msg_decls.len(), 1);
        assert_eq!(prog.wires.len(), 1);
        assert_eq!(prog.direct_connections.len(), 1);
        assert_eq!(
            prog.direct_connections[0].value,
            Expr::Ref("click".to_string())
        );
    }

    // ================================================
    // .attr() chain tests
    // ================================================

    #[test]
    fn test_wire_with_attrs() {
        let source = r#"wire w = flonum(x).attr(minimum: 0., maximum: 100.);"#;
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.wires.len(), 1);
        let wire = &prog.wires[0];
        assert_eq!(wire.name, "w");
        assert_eq!(wire.attrs.len(), 2);
        assert_eq!(wire.attrs[0].key, "minimum");
        assert_eq!(wire.attrs[0].value, AttrValue::Float(0.0));
        assert_eq!(wire.attrs[1].key, "maximum");
        assert_eq!(wire.attrs[1].value, AttrValue::Float(100.0));
    }

    #[test]
    fn test_wire_without_attrs() {
        let source = r#"wire osc = cycle~(440);"#;
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.wires.len(), 1);
        assert!(prog.wires[0].attrs.is_empty());
    }

    #[test]
    fn test_wire_with_string_attr() {
        let source = r#"wire dial = live.dial().attr(parameter_longname: "Cutoff");"#;
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.wires.len(), 1);
        let wire = &prog.wires[0];
        assert_eq!(wire.attrs.len(), 1);
        assert_eq!(wire.attrs[0].key, "parameter_longname");
        assert_eq!(
            wire.attrs[0].value,
            AttrValue::Str("Cutoff".to_string())
        );
    }

    #[test]
    fn test_wire_with_ident_attr() {
        let source = r#"wire osc = cycle~(freq).attr(phase: half);"#;
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.wires.len(), 1);
        let wire = &prog.wires[0];
        assert_eq!(wire.attrs.len(), 1);
        assert_eq!(wire.attrs[0].key, "phase");
        assert_eq!(
            wire.attrs[0].value,
            AttrValue::Ident("half".to_string())
        );
    }

    #[test]
    fn test_wire_with_int_attr() {
        let source = r#"wire w = flonum(x).attr(minimum: 0, maximum: 100);"#;
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.wires.len(), 1);
        let wire = &prog.wires[0];
        assert_eq!(wire.attrs.len(), 2);
        assert_eq!(wire.attrs[0].value, AttrValue::Int(0));
        assert_eq!(wire.attrs[1].value, AttrValue::Int(100));
    }

    #[test]
    fn test_msg_with_attrs() {
        let source = r#"msg click = "bang".attr(patching_rect: 100.);"#;
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.msg_decls.len(), 1);
        let msg = &prog.msg_decls[0];
        assert_eq!(msg.name, "click");
        assert_eq!(msg.content, "bang");
        assert_eq!(msg.attrs.len(), 1);
        assert_eq!(msg.attrs[0].key, "patching_rect");
        assert_eq!(msg.attrs[0].value, AttrValue::Float(100.0));
    }

    #[test]
    fn test_msg_without_attrs() {
        let source = r#"msg click = "bang";"#;
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.msg_decls.len(), 1);
        assert!(prog.msg_decls[0].attrs.is_empty());
    }

    #[test]
    fn test_wire_multiline_attrs() {
        let source = r#"
wire dial = live.dial().attr(
    parameter_longname: "Cutoff",
    parameter_shortname: "Cut",
    minimum: 20.,
    maximum: 20000.
);
"#;
        let prog = parse(source).expect("parse failed");

        assert_eq!(prog.wires.len(), 1);
        let wire = &prog.wires[0];
        assert_eq!(wire.attrs.len(), 4);
        assert_eq!(wire.attrs[0].key, "parameter_longname");
        assert_eq!(wire.attrs[1].key, "parameter_shortname");
        assert_eq!(wire.attrs[2].key, "minimum");
        assert_eq!(wire.attrs[3].key, "maximum");
    }

    // ─── Implicit port index tests ───

    #[test]
    fn test_implicit_in_single() {
        let source = "in freq: float;";
        let prog = parse(source).expect("parse failed");
        assert_eq!(prog.in_decls.len(), 1);
        assert_eq!(prog.in_decls[0].index, 0);
        assert_eq!(prog.in_decls[0].name, "freq");
        assert_eq!(prog.in_decls[0].port_type, PortType::Float);
    }

    #[test]
    fn test_implicit_in_multiple() {
        let source = "in freq: float;\nin cutoff: float;";
        let prog = parse(source).expect("parse failed");
        assert_eq!(prog.in_decls.len(), 2);
        assert_eq!(prog.in_decls[0].index, 0);
        assert_eq!(prog.in_decls[0].name, "freq");
        assert_eq!(prog.in_decls[1].index, 1);
        assert_eq!(prog.in_decls[1].name, "cutoff");
    }

    #[test]
    fn test_implicit_out_single() {
        let source = "out audio: signal;";
        let prog = parse(source).expect("parse failed");
        assert_eq!(prog.out_decls.len(), 1);
        assert_eq!(prog.out_decls[0].index, 0);
        assert_eq!(prog.out_decls[0].name, "audio");
        assert_eq!(prog.out_decls[0].port_type, PortType::Signal);
    }

    #[test]
    fn test_implicit_out_multiple() {
        let source = "out left: signal;\nout right: signal;";
        let prog = parse(source).expect("parse failed");
        assert_eq!(prog.out_decls.len(), 2);
        assert_eq!(prog.out_decls[0].index, 0);
        assert_eq!(prog.out_decls[0].name, "left");
        assert_eq!(prog.out_decls[1].index, 1);
        assert_eq!(prog.out_decls[1].name, "right");
    }

    #[test]
    fn test_implicit_separate_counters_in_out() {
        let source = "in a: float;\nout x: signal;\nin b: float;\nout y: signal;";
        let prog = parse(source).expect("parse failed");
        assert_eq!(prog.in_decls.len(), 2);
        assert_eq!(prog.in_decls[0].index, 0);
        assert_eq!(prog.in_decls[1].index, 1);
        assert_eq!(prog.out_decls.len(), 2);
        assert_eq!(prog.out_decls[0].index, 0);
        assert_eq!(prog.out_decls[1].index, 1);
    }

    #[test]
    fn test_explicit_index_still_works() {
        let source = "in 5 (freq): float;";
        let prog = parse(source).expect("parse failed");
        assert_eq!(prog.in_decls.len(), 1);
        assert_eq!(prog.in_decls[0].index, 5);
        assert_eq!(prog.in_decls[0].name, "freq");
    }

    #[test]
    fn test_implicit_full_patch() {
        let source = r#"
in carrier_freq: float;
in harmonicity: float;
out fm_signal: signal;

wire osc = cycle~(carrier_freq);
out[0] = osc;
"#;
        let prog = parse(source).expect("parse failed");
        assert_eq!(prog.in_decls.len(), 2);
        assert_eq!(prog.in_decls[0].index, 0);
        assert_eq!(prog.in_decls[0].name, "carrier_freq");
        assert_eq!(prog.in_decls[1].index, 1);
        assert_eq!(prog.in_decls[1].name, "harmonicity");
        assert_eq!(prog.out_decls.len(), 1);
        assert_eq!(prog.out_decls[0].index, 0);
        assert_eq!(prog.out_decls[0].name, "fm_signal");
        assert_eq!(prog.wires.len(), 1);
        assert_eq!(prog.out_assignments.len(), 1);
    }

    // ── E52: Out declaration with inline value (tree-sitter) ──

    #[test]
    fn test_out_decl_inline_value_implicit_ts() {
        let source = r#"
wire osc = cycle~(440);
out audio: signal = osc;
"#;
        let prog = parse(source).expect("parse failed");
        assert_eq!(prog.out_decls.len(), 1);
        assert_eq!(prog.out_decls[0].index, 0);
        assert_eq!(prog.out_decls[0].name, "audio");
        assert_eq!(prog.out_decls[0].port_type, PortType::Signal);
        assert_eq!(prog.out_decls[0].value, Some(Expr::Ref("osc".to_string())));
        assert!(prog.out_assignments.is_empty());
    }

    #[test]
    fn test_out_decl_inline_value_explicit_ts() {
        let source = r#"
wire osc = cycle~(440);
out 0 (audio): signal = osc;
"#;
        let prog = parse(source).expect("parse failed");
        assert_eq!(prog.out_decls.len(), 1);
        assert_eq!(prog.out_decls[0].index, 0);
        assert_eq!(prog.out_decls[0].name, "audio");
        assert_eq!(prog.out_decls[0].port_type, PortType::Signal);
        assert_eq!(prog.out_decls[0].value, Some(Expr::Ref("osc".to_string())));
        assert!(prog.out_assignments.is_empty());
    }

    #[test]
    fn test_out_decl_without_value_ts() {
        let source = r#"
out audio: signal;
wire osc = cycle~(440);
out[0] = osc;
"#;
        let prog = parse(source).expect("parse failed");
        assert_eq!(prog.out_decls.len(), 1);
        assert_eq!(prog.out_decls[0].value, None);
        assert_eq!(prog.out_assignments.len(), 1);
    }
}
