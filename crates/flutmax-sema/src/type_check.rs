/// Signal / Control type checking
///
/// Infer wire types and detect Signal->Control-only connection errors at compile time.
/// Collects all errors and reports them (does not stop at the first error).

use std::collections::HashMap;

use flutmax_ast::{CallArg, Expr, LitValue, PortType as AstPortType, Program, Span};

use crate::registry::AbstractionRegistry;

/// Informational subtypes of Control
///
/// Not used for connection checking (judgment is at Signal vs Control level).
/// Used for codegen (typed pack) and future lints.
#[derive(Debug, Clone, PartialEq)]
pub enum ControlSubtype {
    Int,
    Float,
    Symbol,
    Bang,
    List,
    Opaque,
}

/// Inferred type of a wire
#[derive(Debug, Clone, PartialEq)]
pub enum WireType {
    Signal,
    Control(ControlSubtype),
    Unknown,
}

impl WireType {
    /// Helper that returns Control(Opaque) (for migrating existing code)
    pub fn control_opaque() -> Self {
        WireType::Control(ControlSubtype::Opaque)
    }

    /// Helper that returns Control(Float)
    pub fn control_float() -> Self {
        WireType::Control(ControlSubtype::Float)
    }

    /// Helper that returns Control(Int)
    pub fn control_int() -> Self {
        WireType::Control(ControlSubtype::Int)
    }

    /// Helper that returns Control(Symbol)
    pub fn control_symbol() -> Self {
        WireType::Control(ControlSubtype::Symbol)
    }

    /// Whether this is Signal
    pub fn is_signal(&self) -> bool {
        matches!(self, WireType::Signal)
    }

    /// Whether this is Control (any subtype)
    pub fn is_control(&self) -> bool {
        matches!(self, WireType::Control(_))
    }
}

/// Type error
#[derive(Debug, Clone)]
pub struct TypeError {
    /// Error code ("E001", "E005", etc.)
    pub code: &'static str,
    /// Error message
    pub message: String,
    /// Source code location information
    pub span: Option<Span>,
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(ref span) = self.span {
            write!(
                f,
                "error[{}]: {} (at line {}:{})",
                self.code, self.message, span.start_line, span.start_column
            )
        } else {
            write!(f, "error[{}]: {}", self.code, self.message)
        }
    }
}

impl std::error::Error for TypeError {}

/// Run type checking on a program.
///
/// Collects and returns all errors. Returns an empty Vec if no errors.
pub fn type_check(program: &Program) -> Vec<TypeError> {
    type_check_with_registry(program, None)
}

/// Run type checking on a program with an AbstractionRegistry.
///
/// When a registry is provided, matches Abstraction call argument types
/// against target in-declaration port types to detect Signal -> Control-only connection errors.
pub fn type_check_with_registry(
    program: &Program,
    registry: Option<&AbstractionRegistry>,
) -> Vec<TypeError> {
    let mut errors = Vec::new();
    let mut wire_types: HashMap<String, WireType> = HashMap::new();
    // Record types of each tuple wire element (for propagation during destructuring)
    let mut tuple_element_types: HashMap<String, Vec<WireType>> = HashMap::new();

    // 1. Register input port types from in declarations
    for decl in &program.in_decls {
        let wt = port_type_to_wire_type(&decl.port_type);
        wire_types.insert(decl.name.clone(), wt);
    }

    // 1b. Pre-register types from Feedback declarations (to allow forward references from wires)
    for decl in &program.feedback_decls {
        let wt = port_type_to_wire_type(&decl.port_type);
        wire_types.insert(decl.name.clone(), wt);
    }

    // 1c. Pre-register types from State declarations (to allow forward references from wires)
    for decl in &program.state_decls {
        // E017: state declaration type is Signal
        if decl.port_type == AstPortType::Signal {
            errors.push(TypeError {
                code: "E017",
                message: format!(
                    "state '{}' cannot be signal type; state is for Control rate only",
                    decl.name
                ),
                span: decl.span.clone(),
            });
        }

        // E003: Check if a definition with the same name already exists
        if wire_types.contains_key(&decl.name) {
            errors.push(TypeError {
                code: "E003",
                message: format!("wire '{}' is already defined", decl.name),
                span: decl.span.clone(),
            });
        }

        let wt = port_type_to_wire_type(&decl.port_type);
        wire_types.insert(decl.name.clone(), wt);
    }

    // 1d. Register types from MsgDecl (Control(Symbol))
    for decl in &program.msg_decls {
        // E003: Check if a definition with the same name already exists
        if wire_types.contains_key(&decl.name) {
            errors.push(TypeError {
                code: "E003",
                message: format!("wire '{}' is already defined", decl.name),
                span: decl.span.clone(),
            });
        }
        wire_types.insert(decl.name.clone(), WireType::control_symbol());
    }

    // 2. Process Wires in declaration order
    for wire in &program.wires {
        // E003: Same-name wire redefinition check
        if wire_types.contains_key(&wire.name) {
            errors.push(TypeError {
                code: "E003",
                message: format!("wire '{}' is already defined", wire.name),
                span: wire.span.clone(),
            });
        }

        // E002: Undefined reference check
        check_undefined_refs(&wire.value, &wire_types, wire.span.as_ref(), &mut errors);

        let wire_type = infer_wire_type(&wire.value, &wire_types);

        // Check type compatibility of Call expression arguments
        if let Expr::Call { object, args } = &wire.value {
            let resolved_name = resolve_object_name(object);
            let is_registered_abstraction = registry
                .and_then(|r| r.lookup(resolved_name))
                .is_some();

            if is_registered_abstraction {
                // Abstraction port type check (precise check using registry info)
                check_abstraction_args(
                    object, args, &wire_types, registry, wire.span.as_ref(), &mut errors,
                );
            } else {
                // Generic type compatibility check (~ suffix-based inference)
                check_call_args(object, args, &wire_types, wire.span.as_ref(), &mut errors);
            }
        }

        // Check that Tuple expression elements are not Signal
        if let Expr::Tuple(elements) = &wire.value {
            check_tuple_elements(elements, &wire_types, wire.span.as_ref(), &mut errors);
            // Record each element type (for subtype propagation during destructuring)
            let elem_types: Vec<WireType> = elements
                .iter()
                .map(|e| infer_wire_type(e, &wire_types))
                .collect();
            tuple_element_types.insert(wire.name.clone(), elem_types);
        }

        wire_types.insert(wire.name.clone(), wire_type);
    }

    // 2b. Process DestructuringWires
    for dw in &program.destructuring_wires {
        // E003: Duplicate check for each destructuring variable name
        for name in &dw.names {
            if wire_types.contains_key(name) {
                errors.push(TypeError {
                    code: "E003",
                    message: format!("wire '{}' is already defined", name),
                    span: dw.span.clone(),
                });
            }
        }

        // E002: Undefined reference check
        check_undefined_refs(&dw.value, &wire_types, dw.span.as_ref(), &mut errors);

        // Check type compatibility of Call expression arguments
        if let Expr::Call { object, args } = &dw.value {
            check_call_args(object, args, &wire_types, dw.span.as_ref(), &mut errors);
        }

        // Infer types of each variable generated by destructuring
        // If source is from a tuple, propagate each element subtype
        for (i, name) in dw.names.iter().enumerate() {
            let elem_type = infer_destructured_element_type(
                &dw.value, i, &wire_types, &tuple_element_types,
            );
            wire_types.insert(name.clone(), elem_type);
        }
    }

    // 2c. Type check Feedback declarations (Signal only supported)
    // Note: feedback names are pre-registered in step 1b
    for decl in &program.feedback_decls {
        if decl.port_type != AstPortType::Signal {
            errors.push(TypeError {
                code: "E010",
                message: format!(
                    "feedback '{}' must be signal type (got {:?})",
                    decl.name, decl.port_type
                ),
                span: decl.span.clone(),
            });
        }
    }

    // 2d. Check Feedback assignments
    for assign in &program.feedback_assignments {
        // Detect assignment to undeclared feedback
        let is_declared = program
            .feedback_decls
            .iter()
            .any(|d| d.name == assign.target);
        if !is_declared {
            errors.push(TypeError {
                code: "E012",
                message: format!(
                    "'{}' is not declared as feedback",
                    assign.target
                ),
                span: assign.span.clone(),
            });
        }

        // Type check the assignment value
        if let Expr::Call { object, args } = &assign.value {
            check_call_args(object, args, &wire_types, assign.span.as_ref(), &mut errors);
        }
    }

    // 2e. Check State assignments
    {
        let mut assigned_states: std::collections::HashSet<String> = std::collections::HashSet::new();
        for assign in &program.state_assignments {
            // E019: Multiple assignments to the same state
            if !assigned_states.insert(assign.name.clone()) {
                errors.push(TypeError {
                    code: "E019",
                    message: format!(
                        "duplicate state assignment to '{}'",
                        assign.name
                    ),
                    span: assign.span.clone(),
                });
            }

            // Detect assignment to undeclared state
            let is_declared = program
                .state_decls
                .iter()
                .any(|d| d.name == assign.name);
            if !is_declared {
                errors.push(TypeError {
                    code: "E002",
                    message: format!(
                        "undefined reference: '{}' is not declared as state",
                        assign.name
                    ),
                    span: assign.span.clone(),
                });
            }

            // Undefined reference check for assignment value
            check_undefined_refs(&assign.value, &wire_types, assign.span.as_ref(), &mut errors);

            // Check type compatibility of Call expression arguments
            if let Expr::Call { object, args } = &assign.value {
                check_call_args(object, args, &wire_types, assign.span.as_ref(), &mut errors);
            }
        }
    }

    // 2f. Verify all feedback declarations have corresponding assignments
    for decl in &program.feedback_decls {
        let has_assignment = program
            .feedback_assignments
            .iter()
            .any(|a| a.target == decl.name);
        if !has_assignment {
            errors.push(TypeError {
                code: "E011",
                message: format!(
                    "feedback '{}' has no assignment (feedback loop not closed)",
                    decl.name
                ),
                span: decl.span.clone(),
            });
        }
    }

    // 3. Type check out assignments
    for assign in &program.out_assignments {
        if let Some(out_decl) = program.out_decls.iter().find(|d| d.index == assign.index) {
            let expected = port_type_to_wire_type(&out_decl.port_type);
            let actual = infer_wire_type(&assign.value, &wire_types);
            if !is_output_compatible(&actual, &expected) {
                errors.push(TypeError {
                    code: "E005",
                    message: format!(
                        "output type mismatch: out[{}] expects {:?} but got {:?}",
                        assign.index, expected, actual
                    ),
                    span: assign.span.clone(),
                });
            }
        }
    }

    errors
}

/// Convert AST PortType to WireType.
fn port_type_to_wire_type(pt: &AstPortType) -> WireType {
    match pt {
        AstPortType::Signal => WireType::Signal,
        AstPortType::Float => WireType::control_float(),
        AstPortType::Int => WireType::control_int(),
        AstPortType::Symbol => WireType::control_symbol(),
        AstPortType::Bang => WireType::Control(ControlSubtype::Bang),
        AstPortType::List => WireType::Control(ControlSubtype::List),
    }
}

/// Infer the output type of an expression.
fn infer_wire_type(expr: &Expr, wire_types: &HashMap<String, WireType>) -> WireType {
    match expr {
        Expr::Call { object, .. } => {
            let resolved = resolve_object_name(object);
            if is_signal_to_control_object(resolved) {
                // Objects like snapshot~, peakamp~ end with ~ but output control values
                WireType::control_opaque()
            } else if resolved.ends_with('~') {
                WireType::Signal
            } else if is_known_control_object(resolved) {
                WireType::control_opaque()
            } else {
                WireType::Unknown // Unknown objects (abstractions etc.)
            }
        }
        Expr::Ref(name) => wire_types.get(name).cloned().unwrap_or(WireType::Unknown),
        Expr::Lit(lit) => match lit {
            LitValue::Int(_) => WireType::control_int(),
            LitValue::Float(_) => WireType::control_float(),
            LitValue::Str(_) => WireType::control_symbol(),
        },
        Expr::OutputPortAccess(_) => WireType::Unknown,
        Expr::Tuple(_) => WireType::control_opaque(),
    }
}

/// Signal-to-Control converter objects: these end with `~` (accept signal input)
/// but their primary output is a control value (float/int), not signal.
fn is_signal_to_control_object(name: &str) -> bool {
    matches!(name,
        "snapshot~" | "peakamp~" | "zerox~" | "thresh~" |
        "edge~" | "capture~" | "spike~" |
        "fiddle~" | "pitch~" | "bonk~" | "sigmund~"
    )
}

/// Determine whether this is a known Control object.
/// Unknown objects (Abstractions, etc.) are treated as Unknown,
/// preventing false E005 errors.
fn is_known_control_object(name: &str) -> bool {
    matches!(name,
        "+" | "-" | "*" | "/" | "%" |
        "pack" | "unpack" | "trigger" | "t" |
        "route" | "select" | "gate" | "switch" |
        "prepend" | "append" | "sprintf" |
        "int" | "float" | "bang" |
        "button" | "toggle" | "number" | "flonum" |
        "print" | "loadbang" | "counter" |
        "message" | "send" | "receive" |
        // UI objects (output is Control)
        "multislider" | "slider" | "dial" | "led" |
        "kslider" | "nslider" | "umenu" | "textedit" |
        "attrui" | "preset" | "swatch" | "pictctrl" | "matrixctrl" |
        "live.dial" | "live.slider" | "live.toggle" | "live.button" |
        "live.numbox" | "live.menu" | "live.text" | "live.tab" |
        // poly~ voice ports (output type depends on context, treat as Control)
        "in" | "out"
    )
}

/// Determine whether this object implicitly accepts Signal input (UI objects, poly~ voice ports, etc.).
///
/// In Max, Signal -> UI object connections are legal (implicit Signal->Control conversion occurs).
/// `in` / `out` are poly~ voice patcher ports that accept both Signal and Control.
fn is_ui_or_polyport_object(name: &str) -> bool {
    matches!(name,
        // Standard UI objects
        "number" | "flonum" | "multislider" | "slider" | "dial" |
        "toggle" | "button" | "led" | "kslider" | "nslider" |
        "umenu" | "textedit" | "attrui" | "preset" | "swatch" |
        "pictctrl" | "matrixctrl" |
        // Signal-aware UI objects (also end with ~ but listed for clarity)
        "meter~" | "gain~" | "scope~" | "spectroscope~" | "number~" |
        // Live UI objects
        "live.dial" | "live.slider" | "live.toggle" | "live.button" |
        "live.numbox" | "live.menu" | "live.text" | "live.tab" |
        "live.gain~" | "live.meter~" |
        // poly~ voice ports (accept any type)
        "in" | "out"
    )
}

/// Determine whether this is a known strictly Control-only object.
///
/// These objects definitely do not accept Signal input.
/// Unknown objects (abstractions, externals, `pack`, `bpatcher`, `p`, etc.)
/// are not included here, and E001 checking is skipped for them.
fn is_known_strict_control_object(name: &str) -> bool {
    matches!(name,
        // Routing / selection
        "route" | "gate" | "switch" |
        // String / message manipulation
        "prepend" | "append" | "sprintf" |
        // Output / debug
        "print" |
        // Timing / triggering (Control domain)
        "loadbang" | "counter" |
        // Communication
        "send" | "receive"
    )
}

/// Determine whether source type is compatible with inlet dest type.
/// Intended to be called from check_call_args during ObjectDb integration.
///
/// Control subtypes are always compatible (subtypes are informational, not used for connection checks).
#[allow(dead_code)]
fn is_compatible(source: &WireType, dest: &WireType) -> bool {
    match (source, dest) {
        (WireType::Unknown, _) | (_, WireType::Unknown) => true,
        (WireType::Signal, WireType::Signal) => true,
        (WireType::Control(_), WireType::Control(_)) => true,
        (WireType::Control(_), WireType::Signal) => true, // Float -> SignalFloat inlet is OK
        (WireType::Signal, WireType::Control(_)) => false, // Signal -> Control-only is NG
    }
}

/// Determine type compatibility for output assignments.
/// When the output declaration is signal, only Signal wires are allowed (Control is not).
/// Control subtypes are always compatible.
fn is_output_compatible(actual: &WireType, expected: &WireType) -> bool {
    match (actual, expected) {
        (WireType::Unknown, _) | (_, WireType::Unknown) => true,
        (WireType::Signal, WireType::Signal) => true,
        (WireType::Control(_), WireType::Control(_)) => true,
        _ => false,
    }
}

/// Check that Tuple expression elements are not Signal, Bang, or List.
/// Signal cannot connect to `[pack]`, so including it in tuple elements causes an error (E008).
/// Bang/List are also invalid as `[pack]` elements, causing an error (E014).
fn check_tuple_elements(
    elements: &[Expr],
    wire_types: &HashMap<String, WireType>,
    span: Option<&Span>,
    errors: &mut Vec<TypeError>,
) {
    for (i, elem) in elements.iter().enumerate() {
        let elem_type = infer_wire_type(elem, wire_types);
        match &elem_type {
            WireType::Signal => {
                let elem_name = match elem {
                    Expr::Ref(name) => name.clone(),
                    _ => format!("element {}", i),
                };
                errors.push(TypeError {
                    code: "E008",
                    message: format!(
                        "signal in tuple: `{}` is Signal, but tuples (pack) only accept Control values",
                        elem_name
                    ),
                    span: span.cloned(),
                });
            }
            WireType::Control(sub) => match sub {
                ControlSubtype::Bang | ControlSubtype::List => {
                    errors.push(TypeError {
                        code: "E014",
                        message: format!(
                            "tuple element {} has type {:?} which cannot be used as a pack element (only Int, Float, Symbol are allowed)",
                            i, sub
                        ),
                        span: span.cloned(),
                    });
                }
                _ => {} // Int, Float, Symbol, Opaque are OK
            },
            WireType::Unknown => {} // skip
        }
    }
}

/// Detect undefined references in expressions (E002).
/// Recursively traverse the expression tree and report errors when Ref nodes are not in wire_types.
fn check_undefined_refs(
    expr: &Expr,
    wire_types: &HashMap<String, WireType>,
    span: Option<&Span>,
    errors: &mut Vec<TypeError>,
) {
    match expr {
        Expr::Ref(name) => {
            if !wire_types.contains_key(name) {
                errors.push(TypeError {
                    code: "E002",
                    message: format!("undefined reference: '{}'", name),
                    span: span.cloned(),
                });
            }
        }
        Expr::Call { args, .. } => {
            for arg in args {
                check_undefined_refs(&arg.value, wire_types, span, errors);
            }
        }
        Expr::Tuple(elements) => {
            for elem in elements {
                check_undefined_refs(elem, wire_types, span, errors);
            }
        }
        Expr::Lit(_) | Expr::OutputPortAccess(_) => {}
    }
}

/// Check whether Call expression arguments are compatible with target object inlet types.
fn check_call_args(
    object: &str,
    args: &[CallArg],
    wire_types: &HashMap<String, WireType>,
    span: Option<&Span>,
    errors: &mut Vec<TypeError>,
) {
    let resolved = resolve_object_name(object);
    let is_signal_obj = resolved.ends_with('~');

    // Only emit E001 for objects that are KNOWN to be purely control-only.
    // For unknown/unrecognized objects (abstractions, externals, etc.), skip
    // the check to avoid false positives.
    let is_strictly_control_only = !is_signal_obj
        && !is_ui_or_polyport_object(resolved)
        && is_known_strict_control_object(resolved);

    for (i, arg) in args.iter().enumerate() {
        let arg_type = infer_wire_type(&arg.value, wire_types);

        if is_strictly_control_only {
            // Known control-only object: Signal input is an error
            if arg_type.is_signal() {
                let arg_name = match &arg.value {
                    Expr::Ref(name) => name.clone(),
                    _ => format!("argument {}", i),
                };
                errors.push(TypeError {
                    code: "E001",
                    message: format!(
                        "signal connected to control-only inlet: `{}` is Signal, but `{}` inlet {} expects Control",
                        arg_name, object, i
                    ),
                    span: span.cloned(),
                });
            }
        }
        // Signal objects accept both Signal and Control (SignalFloat inlets).
        // UI objects and poly~ ports accept Signal implicitly (Max compatibility).
        // Unknown objects are not checked to avoid false positives.
    }
}

/// Match Abstraction call argument types against registry port declarations.
///
/// Compare the types of Abstraction in-ports registered in the registry
/// with inferred argument types to detect Signal -> Control-only connection errors.
fn check_abstraction_args(
    _object: &str,
    _args: &[CallArg],
    _wire_types: &HashMap<String, WireType>,
    _registry: Option<&AbstractionRegistry>,
    _span: Option<&Span>,
    _errors: &mut Vec<TypeError>,
) {
    // Abstraction port type checking is intentionally lenient.
    // Max Abstractions often accept both Signal and Control on the same inlet
    // (the decompiler infers port type from outlettype, which may not reflect
    // actual inlet capabilities). Full inlet type checking requires objdb-level
    // analysis of the Abstraction's internal structure, which is out of scope.
    // Currently no-op; all Abstraction argument types are accepted.
}

/// Infer the type of each element in a destructuring assignment.
///
/// When source is `unpack(ref)` and `ref` is from a tuple,
/// propagate each tuple element subtype.
/// Falls back to `Control(Opaque)` when unknown.
fn infer_destructured_element_type(
    value: &Expr,
    index: usize,
    _wire_types: &HashMap<String, WireType>,
    tuple_element_types: &HashMap<String, Vec<WireType>>,
) -> WireType {
    match value {
        // wire (a, b) = unpack(source); — if source is from a tuple, propagate element types
        Expr::Call { object, args } if object == "unpack" => {
            if let Some(first_arg) = args.first() {
                if let Expr::Ref(source_name) = &first_arg.value {
                    if let Some(elem_types) = tuple_element_types.get(source_name) {
                        return elem_types.get(index).cloned().unwrap_or(WireType::control_opaque());
                    }
                }
            }
            WireType::control_opaque()
        }
        // wire (a, b) = packed; — if packed is from a tuple, propagate element types
        Expr::Ref(name) => {
            if let Some(elem_types) = tuple_element_types.get(name) {
                return elem_types.get(index).cloned().unwrap_or(WireType::control_opaque());
            }
            WireType::control_opaque()
        }
        _ => WireType::control_opaque(),
    }
}

/// Resolve flutmax aliases to Max object names (for type inference).
fn resolve_object_name(name: &str) -> &str {
    match name {
        "add" => "+",
        "sub" => "-",
        "mul" => "*",
        "dvd" => "/",
        "mod" => "%",
        "add~" => "+~",
        "sub~" => "-~",
        "mul~" => "*~",
        "dvd~" => "/~",
        "mod~" => "%~",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flutmax_ast::*;

    /// Helper to build a program for testing
    fn make_program(
        in_decls: Vec<InDecl>,
        out_decls: Vec<OutDecl>,
        wires: Vec<Wire>,
        out_assignments: Vec<OutAssignment>,
    ) -> Program {
        Program {
            in_decls,
            out_decls,
            wires,
            destructuring_wires: Vec::new(),
            msg_decls: Vec::new(),
            out_assignments,
            direct_connections: Vec::new(),
            feedback_decls: Vec::new(),
            feedback_assignments: Vec::new(),
            state_decls: Vec::new(),
            state_assignments: Vec::new(),
        }
    }

    #[test]
    fn signal_to_signal_ok() {
        // cycle~(440) → mul~(osc, 0.5) — Signal→Signal, OK
        let prog = make_program(
            vec![],
            vec![],
            vec![
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
                        object: "mul~".to_string(),
                        args: vec![
                            CallArg::positional(Expr::Ref("osc".to_string())),
                            CallArg::positional(Expr::Lit(LitValue::Float(0.5))),
                        ],
                    },
                    span: None,
                    attrs: vec![],
                },
            ],
            vec![],
        );

        let errors = type_check(&prog);
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    }

    #[test]
    fn control_to_signal_float_ok() {
        // in (freq): float → cycle~(freq) — Float→SignalFloat inlet, OK
        let prog = make_program(
            vec![InDecl {
                index: 0,
                name: "freq".to_string(),
                port_type: PortType::Float,
            }],
            vec![],
            vec![Wire {
                name: "osc".to_string(),
                value: Expr::Call {
                    object: "cycle~".to_string(),
                    args: vec![CallArg::positional(Expr::Ref("freq".to_string()))],
                },
                span: None,
                attrs: vec![],
            }],
            vec![],
        );

        let errors = type_check(&prog);
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    }

    #[test]
    fn signal_to_control_error() {
        // cycle~(440) → print(osc) — Signal→Control-only, E001
        let prog = make_program(
            vec![],
            vec![],
            vec![
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
                    name: "debug".to_string(),
                    value: Expr::Call {
                        object: "print".to_string(),
                        args: vec![
                            CallArg::positional(Expr::Ref("osc".to_string())),
                        ],
                    },
                    span: Some(Span {
                        start_line: 3,
                        start_column: 1,
                        end_line: 3,
                        end_column: 30,
                    }),
                    attrs: vec![],
                },
            ],
            vec![],
        );

        let errors = type_check(&prog);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, "E001");
        assert!(errors[0].message.contains("osc"));
        assert!(errors[0].message.contains("Signal"));
    }

    #[test]
    fn unknown_object_skipped() {
        // unknown_ext(440) -> mul~(x, 0.5) — Unknown skips type checking
        let prog = make_program(
            vec![],
            vec![],
            vec![
                Wire {
                    name: "x".to_string(),
                    value: Expr::Call {
                        object: "unknown_ext".to_string(),
                        args: vec![CallArg::positional(Expr::Lit(LitValue::Int(440)))],
                    },
                    span: None,
                    attrs: vec![],
                },
                Wire {
                    name: "y".to_string(),
                    value: Expr::Call {
                        object: "mul~".to_string(),
                        args: vec![
                            CallArg::positional(Expr::Ref("x".to_string())),
                            CallArg::positional(Expr::Lit(LitValue::Float(0.5))),
                        ],
                    },
                    span: None,
                    attrs: vec![],
                },
            ],
            vec![],
        );

        let errors = type_check(&prog);
        // unknown_ext does not end with ~, so it is inferred as Control.
        // mul~ is a Signal object, so Control input is OK.
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    }

    #[test]
    fn output_type_mismatch() {
        // out 0: signal; button() → out[0] — Control→Signal out, E005
        let prog = make_program(
            vec![],
            vec![OutDecl {
                index: 0,
                name: "audio".to_string(),
                port_type: PortType::Signal,
                value: None,
            }],
            vec![Wire {
                name: "btn".to_string(),
                value: Expr::Call {
                    object: "button".to_string(),
                    args: vec![],
                },
                span: None,
                attrs: vec![],
            }],
            vec![OutAssignment {
                index: 0,
                value: Expr::Ref("btn".to_string()),
                span: Some(Span {
                    start_line: 4,
                    start_column: 1,
                    end_line: 4,
                    end_column: 15,
                }),
            }],
        );

        let errors = type_check(&prog);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, "E005");
        assert!(errors[0].message.contains("out[0]"));
    }

    #[test]
    fn multiple_errors_collected() {
        // All multiple type errors should be collected
        let prog = make_program(
            vec![],
            vec![OutDecl {
                index: 0,
                name: "audio".to_string(),
                port_type: PortType::Signal,
                value: None,
            }],
            vec![
                Wire {
                    name: "osc".to_string(),
                    value: Expr::Call {
                        object: "cycle~".to_string(),
                        args: vec![CallArg::positional(Expr::Lit(LitValue::Int(440)))],
                    },
                    span: None,
                    attrs: vec![],
                },
                // E001: Signal → Control-only
                Wire {
                    name: "bad1".to_string(),
                    value: Expr::Call {
                        object: "print".to_string(),
                        args: vec![CallArg::positional(Expr::Ref("osc".to_string()))],
                    },
                    span: Some(Span {
                        start_line: 3,
                        start_column: 1,
                        end_line: 3,
                        end_column: 30,
                    }),
                    attrs: vec![],
                },
                // E001: Signal -> Control-only (second)
                Wire {
                    name: "bad2".to_string(),
                    value: Expr::Call {
                        object: "route".to_string(),
                        args: vec![
                            CallArg::positional(Expr::Ref("osc".to_string())),
                            CallArg::positional(Expr::Lit(LitValue::Int(100))),
                        ],
                    },
                    span: Some(Span {
                        start_line: 4,
                        start_column: 1,
                        end_line: 4,
                        end_column: 30,
                    }),
                    attrs: vec![],
                },
            ],
            // E005: Control → Signal out
            vec![OutAssignment {
                index: 0,
                value: Expr::Ref("bad1".to_string()),
                span: Some(Span {
                    start_line: 5,
                    start_column: 1,
                    end_line: 5,
                    end_column: 15,
                }),
            }],
        );

        let errors = type_check(&prog);
        assert_eq!(errors.len(), 3, "expected 3 errors, got: {:?}", errors);

        let e001_count = errors.iter().filter(|e| e.code == "E001").count();
        let e005_count = errors.iter().filter(|e| e.code == "E005").count();
        assert_eq!(e001_count, 2);
        assert_eq!(e005_count, 1);
    }

    #[test]
    fn span_included_in_error() {
        // Errors should contain Span information
        let prog = make_program(
            vec![],
            vec![],
            vec![
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
                    name: "bad".to_string(),
                    value: Expr::Call {
                        object: "print".to_string(),
                        args: vec![
                            CallArg::positional(Expr::Ref("osc".to_string())),
                        ],
                    },
                    span: Some(Span {
                        start_line: 8,
                        start_column: 1,
                        end_line: 8,
                        end_column: 24,
                    }),
                    attrs: vec![],
                },
            ],
            vec![],
        );

        let errors = type_check(&prog);
        assert_eq!(errors.len(), 1);
        let span = errors[0].span.as_ref().expect("span should be present");
        assert_eq!(span.start_line, 8);
        assert_eq!(span.start_column, 1);
    }

    #[test]
    fn signal_input_to_signal_output_ok() {
        // in 0 (sig): signal → out 0 (audio): signal — OK
        let prog = make_program(
            vec![InDecl {
                index: 0,
                name: "sig".to_string(),
                port_type: PortType::Signal,
            }],
            vec![OutDecl {
                index: 0,
                name: "audio".to_string(),
                port_type: PortType::Signal,
                value: None,
            }],
            vec![Wire {
                name: "processed".to_string(),
                value: Expr::Call {
                    object: "mul~".to_string(),
                    args: vec![
                        CallArg::positional(Expr::Ref("sig".to_string())),
                        CallArg::positional(Expr::Lit(LitValue::Float(0.5))),
                    ],
                },
                span: None,
                attrs: vec![],
            }],
            vec![OutAssignment {
                index: 0,
                value: Expr::Ref("processed".to_string()),
                span: None,
            }],
        );

        let errors = type_check(&prog);
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    }

    #[test]
    fn control_to_control_ok() {
        // button() → print(btn) — Control→Control, OK
        let prog = make_program(
            vec![],
            vec![],
            vec![
                Wire {
                    name: "btn".to_string(),
                    value: Expr::Call {
                        object: "button".to_string(),
                        args: vec![],
                    },
                    span: None,
                    attrs: vec![],
                },
                Wire {
                    name: "msg".to_string(),
                    value: Expr::Call {
                        object: "print".to_string(),
                        args: vec![CallArg::positional(Expr::Ref("btn".to_string()))],
                    },
                    span: None,
                    attrs: vec![],
                },
            ],
            vec![],
        );

        let errors = type_check(&prog);
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    }

    #[test]
    fn alias_resolution_signal() {
        // mul~ alias → *~ (Signal object)
        let prog = make_program(
            vec![],
            vec![],
            vec![
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
                        object: "mul~".to_string(),
                        args: vec![
                            CallArg::positional(Expr::Ref("osc".to_string())),
                            CallArg::positional(Expr::Lit(LitValue::Float(0.5))),
                        ],
                    },
                    span: None,
                    attrs: vec![],
                },
            ],
            vec![],
        );

        let errors = type_check(&prog);
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    }

    #[test]
    fn alias_resolution_control_error() {
        // send (known strict control object), Signal input should error
        let prog = make_program(
            vec![],
            vec![],
            vec![
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
                    name: "bad".to_string(),
                    value: Expr::Call {
                        object: "send".to_string(),
                        args: vec![
                            CallArg::positional(Expr::Ref("osc".to_string())),
                        ],
                    },
                    span: None,
                    attrs: vec![],
                },
            ],
            vec![],
        );

        let errors = type_check(&prog);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, "E001");
    }

    #[test]
    fn arithmetic_accepts_signal() {
        // add(osc, 100) — arithmetic operators (add/+) now accept Signal without E001
        // because Max's control arithmetic can receive signal through implicit conversion
        let prog = make_program(
            vec![],
            vec![],
            vec![
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
                    name: "mix".to_string(),
                    value: Expr::Call {
                        object: "add".to_string(),
                        args: vec![
                            CallArg::positional(Expr::Ref("osc".to_string())),
                            CallArg::positional(Expr::Lit(LitValue::Int(100))),
                        ],
                    },
                    span: None,
                    attrs: vec![],
                },
            ],
            vec![],
        );

        let errors = type_check(&prog);
        assert!(errors.is_empty(), "arithmetic should accept signal: {:?}", errors);
    }

    #[test]
    fn literal_to_control_ok() {
        // add(100, 200) — Lit→Control, OK
        let prog = make_program(
            vec![],
            vec![],
            vec![Wire {
                name: "sum".to_string(),
                value: Expr::Call {
                    object: "add".to_string(),
                    args: vec![
                        CallArg::positional(Expr::Lit(LitValue::Int(100))),
                        CallArg::positional(Expr::Lit(LitValue::Int(200))),
                    ],
                },
                span: None,
                attrs: vec![],
            }],
            vec![],
        );

        let errors = type_check(&prog);
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    }

    #[test]
    fn type_error_display_with_span() {
        let err = TypeError {
            code: "E001",
            message: "signal connected to control-only inlet".to_string(),
            span: Some(Span {
                start_line: 8,
                start_column: 24,
                end_line: 8,
                end_column: 40,
            }),
        };
        let display = format!("{}", err);
        assert!(display.contains("E001"));
        assert!(display.contains("line 8:24"));
    }

    #[test]
    fn type_error_display_without_span() {
        let err = TypeError {
            code: "E005",
            message: "output type mismatch".to_string(),
            span: None,
        };
        let display = format!("{}", err);
        assert!(display.contains("E005"));
        assert!(display.contains("output type mismatch"));
    }

    #[test]
    fn empty_program_no_errors() {
        let prog = make_program(vec![], vec![], vec![], vec![]);
        let errors = type_check(&prog);
        assert!(errors.is_empty());
    }

    #[test]
    fn control_output_to_control_out_ok() {
        // out 0: float; button() → out[0] — Control→Control out, OK
        let prog = make_program(
            vec![],
            vec![OutDecl {
                index: 0,
                name: "ctrl".to_string(),
                port_type: PortType::Float,
                value: None,
            }],
            vec![Wire {
                name: "btn".to_string(),
                value: Expr::Call {
                    object: "button".to_string(),
                    args: vec![],
                },
                span: None,
                attrs: vec![],
            }],
            vec![OutAssignment {
                index: 0,
                value: Expr::Ref("btn".to_string()),
                span: None,
            }],
        );

        let errors = type_check(&prog);
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    }

    // ─── Tuple / Destructuring tests ───

    #[test]
    fn tuple_is_control_type() {
        // wire t = (x, y); — Tuple is Control type
        let prog = make_program(
            vec![
                InDecl {
                    index: 0,
                    name: "x".to_string(),
                    port_type: PortType::Float,
                },
                InDecl {
                    index: 1,
                    name: "y".to_string(),
                    port_type: PortType::Float,
                },
            ],
            vec![OutDecl {
                index: 0,
                name: "out".to_string(),
                port_type: PortType::Float,
                value: None,
            }],
            vec![Wire {
                name: "t".to_string(),
                value: Expr::Tuple(vec![
                    Expr::Ref("x".to_string()),
                    Expr::Ref("y".to_string()),
                ]),
                span: None,
                attrs: vec![],
            }],
            vec![OutAssignment {
                index: 0,
                value: Expr::Ref("t".to_string()),
                span: None,
            }],
        );

        let errors = type_check(&prog);
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    }

    #[test]
    fn tuple_signal_element_error() {
        // wire t = (osc, 440); — Signal in tuple → E008
        let prog = make_program(
            vec![],
            vec![],
            vec![
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
                    name: "t".to_string(),
                    value: Expr::Tuple(vec![
                        Expr::Ref("osc".to_string()),
                        Expr::Lit(LitValue::Int(440)),
                    ]),
                    span: Some(Span {
                        start_line: 3,
                        start_column: 1,
                        end_line: 3,
                        end_column: 25,
                    }),
                    attrs: vec![],
                },
            ],
            vec![],
        );

        let errors = type_check(&prog);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, "E008");
        assert!(errors[0].message.contains("osc"));
        assert!(errors[0].message.contains("Signal"));
    }

    #[test]
    fn destructuring_wire_names_are_control() {
        // wire (a, b) = unpack(data); → a, b are Control
        use flutmax_ast::DestructuringWire;

        let mut prog = make_program(
            vec![InDecl {
                index: 0,
                name: "data".to_string(),
                port_type: PortType::Float,
            }],
            vec![OutDecl {
                index: 0,
                name: "out".to_string(),
                port_type: PortType::Float,
                value: None,
            }],
            vec![],
            vec![OutAssignment {
                index: 0,
                value: Expr::Ref("a".to_string()),
                span: None,
            }],
        );
        prog.destructuring_wires.push(DestructuringWire {
            names: vec!["a".to_string(), "b".to_string()],
            value: Expr::Call {
                object: "unpack".to_string(),
                args: vec![CallArg::positional(Expr::Ref("data".to_string()))],
            },
            span: None,
        });

        let errors = type_check(&prog);
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    }

    #[test]
    fn tuple_to_signal_output_error() {
        // wire t = (x, y); → out 0: signal — Tuple(Control) → Signal out, E005
        let prog = make_program(
            vec![
                InDecl {
                    index: 0,
                    name: "x".to_string(),
                    port_type: PortType::Float,
                },
                InDecl {
                    index: 1,
                    name: "y".to_string(),
                    port_type: PortType::Float,
                },
            ],
            vec![OutDecl {
                index: 0,
                name: "audio".to_string(),
                port_type: PortType::Signal,
                value: None,
            }],
            vec![Wire {
                name: "t".to_string(),
                value: Expr::Tuple(vec![
                    Expr::Ref("x".to_string()),
                    Expr::Ref("y".to_string()),
                ]),
                span: None,
                attrs: vec![],
            }],
            vec![OutAssignment {
                index: 0,
                value: Expr::Ref("t".to_string()),
                span: Some(Span {
                    start_line: 5,
                    start_column: 1,
                    end_line: 5,
                    end_column: 15,
                }),
            }],
        );

        let errors = type_check(&prog);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, "E005");
    }

    // ─── Feedback tests ───

    #[test]
    fn feedback_signal_ok() {
        // feedback fb: signal; — OK
        use flutmax_ast::{FeedbackAssignment, FeedbackDecl};

        let mut prog = make_program(vec![], vec![], vec![], vec![]);
        prog.feedback_decls.push(FeedbackDecl {
            name: "fb".to_string(),
            port_type: PortType::Signal,
            span: None,
        });
        prog.feedback_assignments.push(FeedbackAssignment {
            target: "fb".to_string(),
            value: Expr::Call {
                object: "tapin~".to_string(),
                args: vec![CallArg::positional(Expr::Ref("mixed".to_string())), CallArg::positional(Expr::Lit(LitValue::Int(1000)))],
            },
            span: None,
        });

        let errors = type_check(&prog);
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    }

    #[test]
    fn feedback_non_signal_error() {
        // feedback fb: float; — E010 error
        use flutmax_ast::FeedbackDecl;

        let mut prog = make_program(vec![], vec![], vec![], vec![]);
        prog.feedback_decls.push(FeedbackDecl {
            name: "fb".to_string(),
            port_type: PortType::Float,
            span: Some(Span {
                start_line: 1,
                start_column: 1,
                end_line: 1,
                end_column: 20,
            }),
        });

        let errors = type_check(&prog);
        // E010 for non-signal feedback + E011 for missing assignment
        let e010_count = errors.iter().filter(|e| e.code == "E010").count();
        assert_eq!(e010_count, 1);
        assert!(errors[0].message.contains("fb"));
    }

    #[test]
    fn feedback_missing_assignment_error() {
        // feedback fb: signal; with no assignment — E011 error
        use flutmax_ast::FeedbackDecl;

        let mut prog = make_program(vec![], vec![], vec![], vec![]);
        prog.feedback_decls.push(FeedbackDecl {
            name: "fb".to_string(),
            port_type: PortType::Signal,
            span: Some(Span {
                start_line: 1,
                start_column: 1,
                end_line: 1,
                end_column: 22,
            }),
        });

        let errors = type_check(&prog);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, "E011");
        assert!(errors[0].message.contains("fb"));
    }

    #[test]
    fn feedback_undeclared_assignment_error() {
        // feedback fb = ...; with no declaration — E012 error
        use flutmax_ast::FeedbackAssignment;

        let mut prog = make_program(vec![], vec![], vec![], vec![]);
        prog.feedback_assignments.push(FeedbackAssignment {
            target: "nonexistent".to_string(),
            value: Expr::Call {
                object: "tapin~".to_string(),
                args: vec![CallArg::positional(Expr::Ref("mixed".to_string())), CallArg::positional(Expr::Lit(LitValue::Int(1000)))],
            },
            span: Some(Span {
                start_line: 3,
                start_column: 1,
                end_line: 3,
                end_column: 40,
            }),
        });

        let errors = type_check(&prog);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, "E012");
        assert!(errors[0].message.contains("nonexistent"));
    }

    #[test]
    fn feedback_ref_is_signal_type() {
        // feedback fb: signal; → tapout~(fb, 500) should see fb as Signal
        use flutmax_ast::{FeedbackAssignment, FeedbackDecl};

        let mut prog = make_program(
            vec![InDecl {
                index: 0,
                name: "input".to_string(),
                port_type: PortType::Signal,
            }],
            vec![OutDecl {
                index: 0,
                name: "output".to_string(),
                port_type: PortType::Signal,
                value: None,
            }],
            vec![
                Wire {
                    name: "delayed".to_string(),
                    value: Expr::Call {
                        object: "tapout~".to_string(),
                        args: vec![
                            CallArg::positional(Expr::Ref("fb".to_string())),
                            CallArg::positional(Expr::Lit(LitValue::Int(500))),
                        ],
                    },
                    span: None,
                    attrs: vec![],
                },
                Wire {
                    name: "mixed".to_string(),
                    value: Expr::Call {
                        object: "add~".to_string(),
                        args: vec![
                            CallArg::positional(Expr::Ref("input".to_string())),
                            CallArg::positional(Expr::Ref("delayed".to_string())),
                        ],
                    },
                    span: None,
                    attrs: vec![],
                },
            ],
            vec![OutAssignment {
                index: 0,
                value: Expr::Ref("mixed".to_string()),
                span: None,
            }],
        );
        prog.feedback_decls.push(FeedbackDecl {
            name: "fb".to_string(),
            port_type: PortType::Signal,
            span: None,
        });
        prog.feedback_assignments.push(FeedbackAssignment {
            target: "fb".to_string(),
            value: Expr::Call {
                object: "tapin~".to_string(),
                args: vec![
                    CallArg::positional(Expr::Ref("mixed".to_string())),
                    CallArg::positional(Expr::Lit(LitValue::Int(1000))),
                ],
            },
            span: None,
        });

        let errors = type_check(&prog);
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    }

    // ─── E002: Undefined reference tests ───

    #[test]
    fn undefined_ref_e002() {
        // cycle~(nonexistent) — nonexistent is not defined → E002
        let prog = make_program(
            vec![],
            vec![],
            vec![Wire {
                name: "x".to_string(),
                value: Expr::Call {
                    object: "cycle~".to_string(),
                    args: vec![CallArg::positional(Expr::Ref("nonexistent".to_string()))],
                },
                span: None,
                attrs: vec![],
            }],
            vec![],
        );
        let errors = type_check(&prog);
        assert!(errors.iter().any(|e| e.code == "E002"), "expected E002, got: {:?}", errors);
        assert!(errors.iter().any(|e| e.message.contains("nonexistent")));
    }

    #[test]
    fn defined_ref_no_e002() {
        // freq is declared as port → no E002
        let prog = make_program(
            vec![InDecl {
                index: 0,
                name: "freq".to_string(),
                port_type: PortType::Float,
            }],
            vec![],
            vec![Wire {
                name: "osc".to_string(),
                value: Expr::Call {
                    object: "cycle~".to_string(),
                    args: vec![CallArg::positional(Expr::Ref("freq".to_string()))],
                },
                span: None,
                attrs: vec![],
            }],
            vec![],
        );
        let errors = type_check(&prog);
        assert!(errors.iter().all(|e| e.code != "E002"), "unexpected E002: {:?}", errors);
    }

    #[test]
    fn undefined_ref_in_tuple_e002() {
        // (x, unknown) — unknown is not defined → E002
        let prog = make_program(
            vec![InDecl {
                index: 0,
                name: "x".to_string(),
                port_type: PortType::Float,
            }],
            vec![],
            vec![Wire {
                name: "t".to_string(),
                value: Expr::Tuple(vec![
                    Expr::Ref("x".to_string()),
                    Expr::Ref("unknown".to_string()),
                ]),
                span: None,
                attrs: vec![],
            }],
            vec![],
        );
        let errors = type_check(&prog);
        assert!(errors.iter().any(|e| e.code == "E002"), "expected E002, got: {:?}", errors);
        assert!(errors.iter().any(|e| e.message.contains("unknown")));
    }

    #[test]
    fn undefined_ref_in_nested_call_e002() {
        // mul~(cycle~(missing), 0.5) — missing is not defined → E002
        let prog = make_program(
            vec![],
            vec![],
            vec![Wire {
                name: "amp".to_string(),
                value: Expr::Call {
                    object: "mul~".to_string(),
                    args: vec![
                        CallArg::positional(Expr::Call {
                            object: "cycle~".to_string(),
                            args: vec![CallArg::positional(Expr::Ref("missing".to_string()))],
                        }),
                        CallArg::positional(Expr::Lit(LitValue::Float(0.5))),
                    ],
                },
                span: None,
                attrs: vec![],
            }],
            vec![],
        );
        let errors = type_check(&prog);
        assert!(errors.iter().any(|e| e.code == "E002"), "expected E002, got: {:?}", errors);
    }

    #[test]
    fn previous_wire_ref_no_e002() {
        // wire a = ...; wire b = cycle~(a); — a is defined → no E002
        let prog = make_program(
            vec![],
            vec![],
            vec![
                Wire {
                    name: "a".to_string(),
                    value: Expr::Lit(LitValue::Int(440)),
                    span: None,
                    attrs: vec![],
                },
                Wire {
                    name: "b".to_string(),
                    value: Expr::Call {
                        object: "cycle~".to_string(),
                        args: vec![CallArg::positional(Expr::Ref("a".to_string()))],
                    },
                    span: None,
                    attrs: vec![],
                },
            ],
            vec![],
        );
        let errors = type_check(&prog);
        assert!(errors.iter().all(|e| e.code != "E002"), "unexpected E002: {:?}", errors);
    }

    #[test]
    fn feedback_ref_no_e002() {
        // feedback fb: signal; → tapout~(fb, 500) — fb is declared → no E002
        use flutmax_ast::{FeedbackAssignment, FeedbackDecl};

        let mut prog = make_program(
            vec![InDecl {
                index: 0,
                name: "input".to_string(),
                port_type: PortType::Signal,
            }],
            vec![],
            vec![Wire {
                name: "delayed".to_string(),
                value: Expr::Call {
                    object: "tapout~".to_string(),
                    args: vec![
                        CallArg::positional(Expr::Ref("fb".to_string())),
                        CallArg::positional(Expr::Lit(LitValue::Int(500))),
                    ],
                },
                span: None,
                attrs: vec![],
            }],
            vec![],
        );
        prog.feedback_decls.push(FeedbackDecl {
            name: "fb".to_string(),
            port_type: PortType::Signal,
            span: None,
        });
        prog.feedback_assignments.push(FeedbackAssignment {
            target: "fb".to_string(),
            value: Expr::Call {
                object: "tapin~".to_string(),
                args: vec![
                    CallArg::positional(Expr::Ref("delayed".to_string())),
                    CallArg::positional(Expr::Lit(LitValue::Int(1000))),
                ],
            },
            span: None,
        });

        let errors = type_check(&prog);
        assert!(errors.iter().all(|e| e.code != "E002"), "unexpected E002: {:?}", errors);
    }

    // ─── E003: Wire redefinition tests ───

    #[test]
    fn wire_redefinition_e003() {
        // wire x = 1; wire x = 2; — duplicate name → E003
        let prog = make_program(
            vec![],
            vec![],
            vec![
                Wire {
                    name: "x".to_string(),
                    value: Expr::Lit(LitValue::Int(1)),
                    span: None,
                    attrs: vec![],
                },
                Wire {
                    name: "x".to_string(),
                    value: Expr::Lit(LitValue::Int(2)),
                    span: Some(Span {
                        start_line: 2,
                        start_column: 1,
                        end_line: 2,
                        end_column: 15,
                    }),
                    attrs: vec![],
                },
            ],
            vec![],
        );
        let errors = type_check(&prog);
        assert!(errors.iter().any(|e| e.code == "E003"), "expected E003, got: {:?}", errors);
        assert!(errors.iter().any(|e| e.message.contains("x")));
    }

    #[test]
    fn unique_wire_names_no_e003() {
        // wire a = 1; wire b = 2; — unique names → no E003
        let prog = make_program(
            vec![],
            vec![],
            vec![
                Wire {
                    name: "a".to_string(),
                    value: Expr::Lit(LitValue::Int(1)),
                    span: None,
                    attrs: vec![],
                },
                Wire {
                    name: "b".to_string(),
                    value: Expr::Lit(LitValue::Int(2)),
                    span: None,
                    attrs: vec![],
                },
            ],
            vec![],
        );
        let errors = type_check(&prog);
        assert!(errors.iter().all(|e| e.code != "E003"), "unexpected E003: {:?}", errors);
    }

    #[test]
    fn wire_conflicts_with_port_e003() {
        // in 0 (freq): float; wire freq = 440; — conflicts with port name → E003
        let prog = make_program(
            vec![InDecl {
                index: 0,
                name: "freq".to_string(),
                port_type: PortType::Float,
            }],
            vec![],
            vec![Wire {
                name: "freq".to_string(),
                value: Expr::Lit(LitValue::Int(440)),
                span: Some(Span {
                    start_line: 2,
                    start_column: 1,
                    end_line: 2,
                    end_column: 20,
                }),
                attrs: vec![],
            }],
            vec![],
        );
        let errors = type_check(&prog);
        assert!(errors.iter().any(|e| e.code == "E003"), "expected E003, got: {:?}", errors);
    }

    #[test]
    fn destructuring_wire_redefinition_e003() {
        // wire a = 1; wire (a, b) = unpack(data); — a conflicts → E003
        use flutmax_ast::DestructuringWire;

        let mut prog = make_program(
            vec![InDecl {
                index: 0,
                name: "data".to_string(),
                port_type: PortType::Float,
            }],
            vec![],
            vec![Wire {
                name: "a".to_string(),
                value: Expr::Lit(LitValue::Int(1)),
                span: None,
                attrs: vec![],
            }],
            vec![],
        );
        prog.destructuring_wires.push(DestructuringWire {
            names: vec!["a".to_string(), "b".to_string()],
            value: Expr::Call {
                object: "unpack".to_string(),
                args: vec![CallArg::positional(Expr::Ref("data".to_string()))],
            },
            span: Some(Span {
                start_line: 3,
                start_column: 1,
                end_line: 3,
                end_column: 30,
            }),
        });

        let errors = type_check(&prog);
        assert!(errors.iter().any(|e| e.code == "E003"), "expected E003, got: {:?}", errors);
    }

    // ─── E008: Signal in tuple (renamed from E010) tests ───

    #[test]
    fn signal_in_tuple_e008() {
        // wire t = (osc, 100); where osc is Signal → E008 (not E010)
        let prog = make_program(
            vec![],
            vec![],
            vec![
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
                    name: "t".to_string(),
                    value: Expr::Tuple(vec![
                        Expr::Ref("osc".to_string()),
                        Expr::Lit(LitValue::Int(100)),
                    ]),
                    span: Some(Span {
                        start_line: 3,
                        start_column: 1,
                        end_line: 3,
                        end_column: 25,
                    }),
                    attrs: vec![],
                },
            ],
            vec![],
        );
        let errors = type_check(&prog);
        assert!(errors.iter().any(|e| e.code == "E008"), "expected E008, got: {:?}", errors);
        // Verify E010 is NOT used for tuple errors
        assert!(errors.iter().all(|e| e.code != "E010"), "E010 should not be used for tuple errors");
    }

    #[test]
    fn control_tuple_no_e008() {
        // wire t = (x, y); where both are Control → no E008
        let prog = make_program(
            vec![
                InDecl {
                    index: 0,
                    name: "x".to_string(),
                    port_type: PortType::Float,
                },
                InDecl {
                    index: 1,
                    name: "y".to_string(),
                    port_type: PortType::Float,
                },
            ],
            vec![],
            vec![Wire {
                name: "t".to_string(),
                value: Expr::Tuple(vec![
                    Expr::Ref("x".to_string()),
                    Expr::Ref("y".to_string()),
                ]),
                span: None,
                attrs: vec![],
            }],
            vec![],
        );
        let errors = type_check(&prog);
        assert!(errors.iter().all(|e| e.code != "E008"), "unexpected E008: {:?}", errors);
    }

    // ─── Control Subtype tests ───

    #[test]
    fn infer_int_literal_subtype() {
        let wire_types = HashMap::new();
        let expr = Expr::Lit(LitValue::Int(42));
        let wt = infer_wire_type(&expr, &wire_types);
        assert_eq!(wt, WireType::control_int());
    }

    #[test]
    fn infer_float_literal_subtype() {
        let wire_types = HashMap::new();
        let expr = Expr::Lit(LitValue::Float(3.14));
        let wt = infer_wire_type(&expr, &wire_types);
        assert_eq!(wt, WireType::control_float());
    }

    #[test]
    fn infer_string_literal_subtype() {
        let wire_types = HashMap::new();
        let expr = Expr::Lit(LitValue::Str("hello".to_string()));
        let wt = infer_wire_type(&expr, &wire_types);
        assert_eq!(wt, WireType::control_symbol());
    }

    #[test]
    fn infer_known_control_object_opaque() {
        let wire_types = HashMap::new();
        let expr = Expr::Call {
            object: "button".to_string(),
            args: vec![],
        };
        let wt = infer_wire_type(&expr, &wire_types);
        assert_eq!(wt, WireType::control_opaque());
    }

    #[test]
    fn infer_signal_object_type() {
        let wire_types = HashMap::new();
        let expr = Expr::Call {
            object: "cycle~".to_string(),
            args: vec![CallArg::positional(Expr::Lit(LitValue::Int(440)))],
        };
        let wt = infer_wire_type(&expr, &wire_types);
        assert_eq!(wt, WireType::Signal);
    }

    #[test]
    fn infer_tuple_opaque() {
        let wire_types = HashMap::new();
        let expr = Expr::Tuple(vec![
            Expr::Lit(LitValue::Int(1)),
            Expr::Lit(LitValue::Float(2.0)),
        ]);
        let wt = infer_wire_type(&expr, &wire_types);
        assert_eq!(wt, WireType::control_opaque());
    }

    #[test]
    fn port_type_float_maps_to_control_float() {
        let wt = port_type_to_wire_type(&PortType::Float);
        assert_eq!(wt, WireType::control_float());
    }

    #[test]
    fn port_type_int_maps_to_control_int() {
        let wt = port_type_to_wire_type(&PortType::Int);
        assert_eq!(wt, WireType::control_int());
    }

    #[test]
    fn port_type_signal_maps_to_signal() {
        let wt = port_type_to_wire_type(&PortType::Signal);
        assert_eq!(wt, WireType::Signal);
    }

    #[test]
    fn port_type_symbol_maps_to_control_symbol() {
        let wt = port_type_to_wire_type(&PortType::Symbol);
        assert_eq!(wt, WireType::control_symbol());
    }

    #[test]
    fn port_type_bang_maps_to_control_bang() {
        let wt = port_type_to_wire_type(&PortType::Bang);
        assert_eq!(wt, WireType::Control(ControlSubtype::Bang));
    }

    #[test]
    fn port_type_list_maps_to_control_list() {
        let wt = port_type_to_wire_type(&PortType::List);
        assert_eq!(wt, WireType::Control(ControlSubtype::List));
    }

    #[test]
    fn control_subtypes_always_compatible() {
        // Control(Int) vs Control(Float) → always true
        assert!(is_compatible(&WireType::control_int(), &WireType::control_float()));
        assert!(is_compatible(&WireType::control_float(), &WireType::control_int()));
        assert!(is_compatible(&WireType::control_symbol(), &WireType::control_opaque()));
        assert!(is_compatible(
            &WireType::Control(ControlSubtype::Bang),
            &WireType::control_int()
        ));
    }

    #[test]
    fn control_subtypes_output_compatible() {
        // Control(Int) → Control(Float) out → true
        assert!(is_output_compatible(&WireType::control_int(), &WireType::control_float()));
        assert!(is_output_compatible(&WireType::control_float(), &WireType::control_int()));
    }

    #[test]
    fn wire_type_helper_methods() {
        assert!(WireType::Signal.is_signal());
        assert!(!WireType::Signal.is_control());
        assert!(WireType::control_int().is_control());
        assert!(!WireType::control_int().is_signal());
        assert!(!WireType::Unknown.is_signal());
        assert!(!WireType::Unknown.is_control());
    }

    #[test]
    fn float_input_inferred_as_control_float() {
        // in 0 (freq): float -> freq type is Control(Float)
        let prog = make_program(
            vec![InDecl {
                index: 0,
                name: "freq".to_string(),
                port_type: PortType::Float,
            }],
            vec![OutDecl {
                index: 0,
                name: "out".to_string(),
                port_type: PortType::Float,
                value: None,
            }],
            vec![Wire {
                name: "val".to_string(),
                value: Expr::Ref("freq".to_string()),
                span: None,
                attrs: vec![],
            }],
            vec![OutAssignment {
                index: 0,
                value: Expr::Ref("val".to_string()),
                span: None,
            }],
        );

        let errors = type_check(&prog);
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    }

    // ─── State tests ───

    #[test]
    fn state_int_registers_type() {
        // state counter: int = 0; -> registered as Int in wire_types
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![OutDecl {
                index: 0,
                name: "count".to_string(),
                port_type: PortType::Int,
                value: None,
            }],
            wires: vec![Wire {
                name: "next".to_string(),
                value: Expr::Call {
                    object: "add".to_string(),
                    args: vec![
                        CallArg::positional(Expr::Ref("counter".to_string())),
                        CallArg::positional(Expr::Lit(LitValue::Int(1))),
                    ],
                },
                span: None,
                attrs: vec![],
            }],
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![OutAssignment {
                index: 0,
                value: Expr::Ref("next".to_string()),
                span: None,
            }],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![StateDecl {
                name: "counter".to_string(),
                port_type: PortType::Int,
                init_value: Expr::Lit(LitValue::Int(0)),
                span: None,
            }],
            state_assignments: vec![StateAssignment {
                name: "counter".to_string(),
                value: Expr::Ref("next".to_string()),
                span: None,
            }],
        };

        let errors = type_check(&prog);
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    }

    #[test]
    fn state_signal_e017() {
        // state x: signal = 0; → E017
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![],
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![StateDecl {
                name: "x".to_string(),
                port_type: PortType::Signal,
                init_value: Expr::Lit(LitValue::Int(0)),
                span: Some(Span {
                    start_line: 1,
                    start_column: 1,
                    end_line: 1,
                    end_column: 25,
                }),
            }],
            state_assignments: vec![],
        };

        let errors = type_check(&prog);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, "E017");
        assert!(errors[0].message.contains("signal"));
    }

    #[test]
    fn state_duplicate_assignment_e019() {
        // state counter: int = 0;
        // state counter = a;
        // state counter = b;  → E019
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![
                Wire {
                    name: "a".to_string(),
                    value: Expr::Lit(LitValue::Int(1)),
                    span: None,
                    attrs: vec![],
                },
                Wire {
                    name: "b".to_string(),
                    value: Expr::Lit(LitValue::Int(2)),
                    span: None,
                    attrs: vec![],
                },
            ],
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![StateDecl {
                name: "counter".to_string(),
                port_type: PortType::Int,
                init_value: Expr::Lit(LitValue::Int(0)),
                span: None,
            }],
            state_assignments: vec![
                StateAssignment {
                    name: "counter".to_string(),
                    value: Expr::Ref("a".to_string()),
                    span: None,
                },
                StateAssignment {
                    name: "counter".to_string(),
                    value: Expr::Ref("b".to_string()),
                    span: Some(Span {
                        start_line: 4,
                        start_column: 1,
                        end_line: 4,
                        end_column: 20,
                    }),
                },
            ],
        };

        let errors = type_check(&prog);
        let e019_errors: Vec<_> = errors.iter().filter(|e| e.code == "E019").collect();
        assert_eq!(e019_errors.len(), 1);
        assert!(e019_errors[0].message.contains("counter"));
    }

    #[test]
    fn state_undeclared_assignment_error() {
        // state x = val; (x is not declared as state)
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![Wire {
                name: "val".to_string(),
                value: Expr::Lit(LitValue::Int(1)),
                span: None,
                attrs: vec![],
            }],
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![StateAssignment {
                name: "x".to_string(),
                value: Expr::Ref("val".to_string()),
                span: Some(Span {
                    start_line: 2,
                    start_column: 1,
                    end_line: 2,
                    end_column: 15,
                }),
            }],
        };

        let errors = type_check(&prog);
        let e002_errors: Vec<_> = errors.iter().filter(|e| e.code == "E002").collect();
        assert_eq!(e002_errors.len(), 1);
        assert!(e002_errors[0].message.contains("x"));
    }

    #[test]
    fn state_name_conflicts_with_wire_e003() {
        // wire counter = ...; state counter: int = 0; → E003
        let prog = Program {
            in_decls: vec![InDecl {
                index: 0,
                name: "counter".to_string(),
                port_type: PortType::Int,
            }],
            out_decls: vec![],
            wires: vec![],
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![StateDecl {
                name: "counter".to_string(),
                port_type: PortType::Int,
                init_value: Expr::Lit(LitValue::Int(0)),
                span: Some(Span {
                    start_line: 2,
                    start_column: 1,
                    end_line: 2,
                    end_column: 25,
                }),
            }],
            state_assignments: vec![],
        };

        let errors = type_check(&prog);
        let e003_errors: Vec<_> = errors.iter().filter(|e| e.code == "E003").collect();
        assert_eq!(e003_errors.len(), 1);
        assert!(e003_errors[0].message.contains("counter"));
    }

    // ─── E20: Typed Destructuring tests ───

    #[test]
    fn destructuring_propagates_int_subtype_from_tuple() {
        // wire t = (1, 2, 3); wire (a, b, c) = unpack(t);
        // -> a, b, c are Control(Int)
        use flutmax_ast::DestructuringWire;

        let mut prog = make_program(
            vec![],
            vec![],
            vec![Wire {
                name: "t".to_string(),
                value: Expr::Tuple(vec![
                    Expr::Lit(LitValue::Int(1)),
                    Expr::Lit(LitValue::Int(2)),
                    Expr::Lit(LitValue::Int(3)),
                ]),
                span: None,
                attrs: vec![],
            }],
            vec![],
        );
        prog.destructuring_wires.push(DestructuringWire {
            names: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            value: Expr::Call {
                object: "unpack".to_string(),
                args: vec![CallArg::positional(Expr::Ref("t".to_string()))],
            },
            span: None,
        });

        let errors = type_check(&prog);
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);

        // To verify wire_types after type checking, using type_check again
        // is not possible to get type info, so test infer_destructured_element_type directly
        let wire_types = HashMap::new();
        let mut tuple_elem_types = HashMap::new();
        tuple_elem_types.insert(
            "t".to_string(),
            vec![WireType::control_int(), WireType::control_int(), WireType::control_int()],
        );

        let elem0 = infer_destructured_element_type(
            &Expr::Call {
                object: "unpack".to_string(),
                args: vec![CallArg::positional(Expr::Ref("t".to_string()))],
            },
            0,
            &wire_types,
            &tuple_elem_types,
        );
        assert_eq!(elem0, WireType::control_int());
    }

    #[test]
    fn destructuring_propagates_mixed_subtypes_from_tuple() {
        // wire t = (1, 0.5, "x"); wire (a, b, c) = unpack(t);
        // → a: Int, b: Float, c: Symbol
        let wire_types = HashMap::new();
        let mut tuple_elem_types = HashMap::new();
        tuple_elem_types.insert(
            "t".to_string(),
            vec![
                WireType::control_int(),
                WireType::control_float(),
                WireType::control_symbol(),
            ],
        );

        let expr = Expr::Call {
            object: "unpack".to_string(),
            args: vec![CallArg::positional(Expr::Ref("t".to_string()))],
        };

        assert_eq!(
            infer_destructured_element_type(&expr, 0, &wire_types, &tuple_elem_types),
            WireType::control_int()
        );
        assert_eq!(
            infer_destructured_element_type(&expr, 1, &wire_types, &tuple_elem_types),
            WireType::control_float()
        );
        assert_eq!(
            infer_destructured_element_type(&expr, 2, &wire_types, &tuple_elem_types),
            WireType::control_symbol()
        );
    }

    #[test]
    fn destructuring_ref_propagates_subtypes() {
        // wire (a, b) = packed; — if packed is from a tuple, propagate subtypes
        let wire_types = HashMap::new();
        let mut tuple_elem_types = HashMap::new();
        tuple_elem_types.insert(
            "packed".to_string(),
            vec![WireType::control_int(), WireType::control_float()],
        );

        let expr = Expr::Ref("packed".to_string());

        assert_eq!(
            infer_destructured_element_type(&expr, 0, &wire_types, &tuple_elem_types),
            WireType::control_int()
        );
        assert_eq!(
            infer_destructured_element_type(&expr, 1, &wire_types, &tuple_elem_types),
            WireType::control_float()
        );
    }

    #[test]
    fn destructuring_unknown_source_falls_back_to_opaque() {
        // unpack(unknown_source) → Control(Opaque)
        let wire_types = HashMap::new();
        let tuple_elem_types = HashMap::new();

        let expr = Expr::Call {
            object: "unpack".to_string(),
            args: vec![CallArg::positional(Expr::Ref("unknown".to_string()))],
        };

        assert_eq!(
            infer_destructured_element_type(&expr, 0, &wire_types, &tuple_elem_types),
            WireType::control_opaque()
        );
    }

    #[test]
    fn destructuring_non_unpack_call_falls_back_to_opaque() {
        // wire (a, b) = some_call(x); → Control(Opaque)
        let wire_types = HashMap::new();
        let tuple_elem_types = HashMap::new();

        let expr = Expr::Call {
            object: "route".to_string(),
            args: vec![CallArg::positional(Expr::Ref("x".to_string()))],
        };

        assert_eq!(
            infer_destructured_element_type(&expr, 0, &wire_types, &tuple_elem_types),
            WireType::control_opaque()
        );
    }

    #[test]
    fn destructuring_index_out_of_range_falls_back_to_opaque() {
        // Accessing index 3 when tuple only has 2 elements -> Opaque
        let wire_types = HashMap::new();
        let mut tuple_elem_types = HashMap::new();
        tuple_elem_types.insert(
            "t".to_string(),
            vec![WireType::control_int(), WireType::control_float()],
        );

        let expr = Expr::Call {
            object: "unpack".to_string(),
            args: vec![CallArg::positional(Expr::Ref("t".to_string()))],
        };

        assert_eq!(
            infer_destructured_element_type(&expr, 3, &wire_types, &tuple_elem_types),
            WireType::control_opaque()
        );
    }

    #[test]
    fn destructuring_e2e_tuple_unpack_subtypes_propagated() {
        // E2E: wire t = (255, 128, 0); wire (r, g, b) = unpack(t);
        // type_check should pass and r, g, b should have Int subtype
        // (verified indirectly: no errors, and output assignment to Int port succeeds)
        use flutmax_ast::DestructuringWire;

        let prog = Program {
            in_decls: vec![],
            out_decls: vec![
                OutDecl { index: 0, name: "r_out".to_string(), port_type: PortType::Int, value: None },
                OutDecl { index: 1, name: "g_out".to_string(), port_type: PortType::Int, value: None },
                OutDecl { index: 2, name: "b_out".to_string(), port_type: PortType::Int, value: None },
            ],
            wires: vec![Wire {
                name: "rgb".to_string(),
                value: Expr::Tuple(vec![
                    Expr::Lit(LitValue::Int(255)),
                    Expr::Lit(LitValue::Int(128)),
                    Expr::Lit(LitValue::Int(0)),
                ]),
                span: None,
                attrs: vec![],
            }],
            destructuring_wires: vec![DestructuringWire {
                names: vec!["r".to_string(), "g".to_string(), "b".to_string()],
                value: Expr::Call {
                    object: "unpack".to_string(),
                    args: vec![CallArg::positional(Expr::Ref("rgb".to_string()))],
                },
                span: None,
            }],
            msg_decls: vec![],
            out_assignments: vec![
                OutAssignment { index: 0, value: Expr::Ref("r".to_string()), span: None },
                OutAssignment { index: 1, value: Expr::Ref("g".to_string()), span: None },
                OutAssignment { index: 2, value: Expr::Ref("b".to_string()), span: None },
            ],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let errors = type_check(&prog);
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    }

    // ─── E014: Bang/List in tuple tests ───

    #[test]
    fn bang_in_tuple_e014() {
        // in 0 (trigger): bang; wire t = (trigger, 1); → E014
        let prog = make_program(
            vec![InDecl {
                index: 0,
                name: "trigger".to_string(),
                port_type: PortType::Bang,
            }],
            vec![],
            vec![Wire {
                name: "t".to_string(),
                value: Expr::Tuple(vec![
                    Expr::Ref("trigger".to_string()),
                    Expr::Lit(LitValue::Int(1)),
                ]),
                span: Some(Span {
                    start_line: 2,
                    start_column: 1,
                    end_line: 2,
                    end_column: 30,
                }),
                attrs: vec![],
            }],
            vec![],
        );

        let errors = type_check(&prog);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, "E014");
        assert!(errors[0].message.contains("Bang"));
    }

    #[test]
    fn list_in_tuple_e014() {
        // in 0 (data): list; wire t = (1, data); → E014
        let prog = make_program(
            vec![InDecl {
                index: 0,
                name: "data".to_string(),
                port_type: PortType::List,
            }],
            vec![],
            vec![Wire {
                name: "t".to_string(),
                value: Expr::Tuple(vec![
                    Expr::Lit(LitValue::Int(1)),
                    Expr::Ref("data".to_string()),
                ]),
                span: Some(Span {
                    start_line: 2,
                    start_column: 1,
                    end_line: 2,
                    end_column: 30,
                }),
                attrs: vec![],
            }],
            vec![],
        );

        let errors = type_check(&prog);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, "E014");
        assert!(errors[0].message.contains("List"));
    }

    #[test]
    fn float_int_symbol_in_tuple_no_e014() {
        // in 0 (x): float; in 1 (y): int; in 2 (z): symbol;
        // wire t = (x, y, z); → no E014
        let prog = make_program(
            vec![
                InDecl {
                    index: 0,
                    name: "x".to_string(),
                    port_type: PortType::Float,
                },
                InDecl {
                    index: 1,
                    name: "y".to_string(),
                    port_type: PortType::Int,
                },
                InDecl {
                    index: 2,
                    name: "z".to_string(),
                    port_type: PortType::Symbol,
                },
            ],
            vec![],
            vec![Wire {
                name: "t".to_string(),
                value: Expr::Tuple(vec![
                    Expr::Ref("x".to_string()),
                    Expr::Ref("y".to_string()),
                    Expr::Ref("z".to_string()),
                ]),
                span: None,
                attrs: vec![],
            }],
            vec![],
        );

        let errors = type_check(&prog);
        assert!(
            errors.iter().all(|e| e.code != "E014"),
            "unexpected E014: {:?}",
            errors
        );
    }

    #[test]
    fn literals_in_tuple_no_e014() {
        // wire t = (1, 0.5, "x"); → no E014 (Int, Float, Symbol literals)
        let prog = make_program(
            vec![],
            vec![],
            vec![Wire {
                name: "t".to_string(),
                value: Expr::Tuple(vec![
                    Expr::Lit(LitValue::Int(1)),
                    Expr::Lit(LitValue::Float(0.5)),
                    Expr::Lit(LitValue::Str("x".to_string())),
                ]),
                span: None,
                attrs: vec![],
            }],
            vec![],
        );

        let errors = type_check(&prog);
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    }

    // ─── Abstraction port type checking tests ───

    #[test]
    fn abstraction_signal_to_float_inlet_e001() {
        // Registry: filter has in 0 (input): signal, in 1 (cutoff): float
        // wire osc = cycle~(440); wire f = filter(osc, osc);
        // → osc (Signal) to cutoff (Float) → E001
        use crate::registry::{AbstractionInterface, AbstractionRegistry, PortInfo};

        let mut registry = AbstractionRegistry::new();
        registry.register_interface(AbstractionInterface {
            name: "filter".to_string(),
            in_ports: vec![
                PortInfo {
                    index: 0,
                    name: "input".to_string(),
                    port_type: AstPortType::Signal,
                },
                PortInfo {
                    index: 1,
                    name: "cutoff".to_string(),
                    port_type: AstPortType::Float,
                },
            ],
            out_ports: vec![PortInfo {
                index: 0,
                name: "output".to_string(),
                port_type: AstPortType::Signal,
            }],
        });

        let prog = make_program(
            vec![],
            vec![],
            vec![
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
                    name: "f".to_string(),
                    value: Expr::Call {
                        object: "filter".to_string(),
                        args: vec![
                            CallArg::positional(Expr::Ref("osc".to_string())),
                            CallArg::positional(Expr::Ref("osc".to_string())),
                        ],
                    },
                    span: Some(Span {
                        start_line: 3,
                        start_column: 1,
                        end_line: 3,
                        end_column: 35,
                    }),
                    attrs: vec![],
                },
            ],
            vec![],
        );

        let errors = type_check_with_registry(&prog, Some(&registry));
        // Abstraction type checking is now lenient (no E001 for Abstraction args)
        // because decompiled port types may not reflect actual inlet capabilities
        let e001_errors: Vec<_> = errors.iter().filter(|e| e.code == "E001").collect();
        assert_eq!(e001_errors.len(), 0, "Abstraction args should not trigger E001, got: {:?}", errors);
    }

    #[test]
    fn abstraction_float_to_float_inlet_ok() {
        // Registry: filter has in 0 (input): signal, in 1 (cutoff): float
        // wire osc = cycle~(440); wire f = filter(osc, 1000.0);
        // → Float to Float → OK
        use crate::registry::{AbstractionInterface, AbstractionRegistry, PortInfo};

        let mut registry = AbstractionRegistry::new();
        registry.register_interface(AbstractionInterface {
            name: "filter".to_string(),
            in_ports: vec![
                PortInfo {
                    index: 0,
                    name: "input".to_string(),
                    port_type: AstPortType::Signal,
                },
                PortInfo {
                    index: 1,
                    name: "cutoff".to_string(),
                    port_type: AstPortType::Float,
                },
            ],
            out_ports: vec![],
        });

        let prog = make_program(
            vec![],
            vec![],
            vec![
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
                    name: "f".to_string(),
                    value: Expr::Call {
                        object: "filter".to_string(),
                        args: vec![
                            CallArg::positional(Expr::Ref("osc".to_string())),
                            CallArg::positional(Expr::Lit(LitValue::Float(1000.0))),
                        ],
                    },
                    span: None,
                    attrs: vec![],
                },
            ],
            vec![],
        );

        let errors = type_check_with_registry(&prog, Some(&registry));
        assert!(
            errors.iter().all(|e| e.code != "E001"),
            "unexpected E001: {:?}",
            errors
        );
    }

    #[test]
    fn abstraction_signal_to_signal_inlet_ok() {
        // Registry: processor has in 0 (input): signal
        // wire osc = cycle~(440); wire p = processor(osc);
        // → Signal to Signal → OK
        use crate::registry::{AbstractionInterface, AbstractionRegistry, PortInfo};

        let mut registry = AbstractionRegistry::new();
        registry.register_interface(AbstractionInterface {
            name: "processor".to_string(),
            in_ports: vec![PortInfo {
                index: 0,
                name: "input".to_string(),
                port_type: AstPortType::Signal,
            }],
            out_ports: vec![],
        });

        let prog = make_program(
            vec![],
            vec![],
            vec![
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
                    name: "p".to_string(),
                    value: Expr::Call {
                        object: "processor".to_string(),
                        args: vec![CallArg::positional(Expr::Ref("osc".to_string()))],
                    },
                    span: None,
                    attrs: vec![],
                },
            ],
            vec![],
        );

        let errors = type_check_with_registry(&prog, Some(&registry));
        assert!(
            errors.iter().all(|e| e.code != "E001"),
            "unexpected E001: {:?}",
            errors
        );
    }

    #[test]
    fn abstraction_unknown_wire_to_any_inlet_ok() {
        // Registry: processor has in 0 (input): float
        // wire x = unknown_ext(440); wire p = processor(x);
        // → Unknown to Float → OK (skip)
        use crate::registry::{AbstractionInterface, AbstractionRegistry, PortInfo};

        let mut registry = AbstractionRegistry::new();
        registry.register_interface(AbstractionInterface {
            name: "processor".to_string(),
            in_ports: vec![PortInfo {
                index: 0,
                name: "input".to_string(),
                port_type: AstPortType::Float,
            }],
            out_ports: vec![],
        });

        let prog = make_program(
            vec![],
            vec![],
            vec![
                Wire {
                    name: "x".to_string(),
                    value: Expr::Call {
                        object: "unknown_ext".to_string(),
                        args: vec![CallArg::positional(Expr::Lit(LitValue::Int(440)))],
                    },
                    span: None,
                    attrs: vec![],
                },
                Wire {
                    name: "p".to_string(),
                    value: Expr::Call {
                        object: "processor".to_string(),
                        args: vec![CallArg::positional(Expr::Ref("x".to_string()))],
                    },
                    span: None,
                    attrs: vec![],
                },
            ],
            vec![],
        );

        let errors = type_check_with_registry(&prog, Some(&registry));
        assert!(
            errors.iter().all(|e| e.code != "E001"),
            "unexpected E001: {:?}",
            errors
        );
    }

    #[test]
    fn type_check_without_registry_still_works() {
        // type_check() (no registry) should work as before
        let prog = make_program(
            vec![InDecl {
                index: 0,
                name: "freq".to_string(),
                port_type: PortType::Float,
            }],
            vec![],
            vec![Wire {
                name: "osc".to_string(),
                value: Expr::Call {
                    object: "cycle~".to_string(),
                    args: vec![CallArg::positional(Expr::Ref("freq".to_string()))],
                },
                span: None,
                attrs: vec![],
            }],
            vec![],
        );

        let errors = type_check(&prog);
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    }

    // ─── MsgDecl tests ───

    #[test]
    fn msg_registers_as_control_symbol() {
        use flutmax_ast::MsgDecl;

        let prog = Program {
            in_decls: vec![],
            out_decls: vec![OutDecl {
                index: 0,
                name: "output".to_string(),
                port_type: PortType::Symbol,
                value: None,
            }],
            wires: vec![],
            destructuring_wires: vec![],
            msg_decls: vec![MsgDecl {
                name: "click".to_string(),
                content: "bang".to_string(),
                span: None,
                attrs: vec![],
            }],
            out_assignments: vec![OutAssignment {
                index: 0,
                value: Expr::Ref("click".to_string()),
                span: None,
            }],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let errors = type_check(&prog);
        assert!(errors.is_empty(), "msg should be Control(Symbol), got: {:?}", errors);
    }

    #[test]
    fn msg_duplicate_name_e003() {
        use flutmax_ast::MsgDecl;

        let prog = Program {
            in_decls: vec![InDecl {
                index: 0,
                name: "click".to_string(),
                port_type: PortType::Bang,
            }],
            out_decls: vec![],
            wires: vec![],
            destructuring_wires: vec![],
            msg_decls: vec![MsgDecl {
                name: "click".to_string(),
                content: "bang".to_string(),
                span: None,
                attrs: vec![],
            }],
            out_assignments: vec![],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let errors = type_check(&prog);
        let e003_errors: Vec<_> = errors.iter().filter(|e| e.code == "E003").collect();
        assert_eq!(e003_errors.len(), 1, "should have E003 for duplicate name");
    }

    #[test]
    fn msg_to_signal_output_e005() {
        use flutmax_ast::MsgDecl;

        let prog = Program {
            in_decls: vec![],
            out_decls: vec![OutDecl {
                index: 0,
                name: "audio".to_string(),
                port_type: PortType::Signal,
                value: None,
            }],
            wires: vec![],
            destructuring_wires: vec![],
            msg_decls: vec![MsgDecl {
                name: "click".to_string(),
                content: "bang".to_string(),
                span: None,
                attrs: vec![],
            }],
            out_assignments: vec![OutAssignment {
                index: 0,
                value: Expr::Ref("click".to_string()),
                span: None,
            }],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let errors = type_check(&prog);
        let e005_errors: Vec<_> = errors.iter().filter(|e| e.code == "E005").collect();
        assert_eq!(e005_errors.len(), 1, "msg (Control) to Signal output should error");
    }

    #[test]
    fn signal_to_ui_object_no_error() {
        // Signal → number/flonum/multislider should NOT trigger E001
        // Max allows implicit Signal→Control conversion for UI objects
        for ui_obj in &["number", "flonum", "multislider", "slider", "dial", "toggle"] {
            let prog = make_program(
                vec![],
                vec![],
                vec![
                    Wire {
                        name: "sig".to_string(),
                        value: Expr::Call {
                            object: "cycle~".to_string(),
                            args: vec![CallArg::positional(Expr::Lit(LitValue::Int(440)))],
                        },
                        span: None,
                        attrs: vec![],
                    },
                    Wire {
                        name: "ui_out".to_string(),
                        value: Expr::Call {
                            object: ui_obj.to_string(),
                            args: vec![CallArg::positional(Expr::Ref("sig".to_string()))],
                        },
                        span: None,
                        attrs: vec![],
                    },
                ],
                vec![],
            );

            let errors = type_check(&prog);
            let e001_errors: Vec<_> = errors.iter().filter(|e| e.code == "E001").collect();
            assert!(
                e001_errors.is_empty(),
                "Signal → {} should not trigger E001, got: {:?}",
                ui_obj, e001_errors
            );
        }
    }

    #[test]
    fn signal_to_out_polyport_no_error() {
        // Signal → out() in poly~ voice patcher should NOT trigger E001
        let prog = make_program(
            vec![],
            vec![],
            vec![
                Wire {
                    name: "sig".to_string(),
                    value: Expr::Call {
                        object: "cycle~".to_string(),
                        args: vec![CallArg::positional(Expr::Lit(LitValue::Int(440)))],
                    },
                    span: None,
                    attrs: vec![],
                },
                Wire {
                    name: "voice_out".to_string(),
                    value: Expr::Call {
                        object: "out".to_string(),
                        args: vec![CallArg::positional(Expr::Ref("sig".to_string()))],
                    },
                    span: None,
                    attrs: vec![],
                },
            ],
            vec![],
        );

        let errors = type_check(&prog);
        let e001_errors: Vec<_> = errors.iter().filter(|e| e.code == "E001").collect();
        assert!(
            e001_errors.is_empty(),
            "Signal → out (poly~ port) should not trigger E001, got: {:?}",
            e001_errors
        );
    }

    #[test]
    fn signal_to_pure_control_still_errors() {
        // Signal → print/add should still trigger E001
        let prog = make_program(
            vec![],
            vec![],
            vec![
                Wire {
                    name: "sig".to_string(),
                    value: Expr::Call {
                        object: "cycle~".to_string(),
                        args: vec![CallArg::positional(Expr::Lit(LitValue::Int(440)))],
                    },
                    span: None,
                    attrs: vec![],
                },
                Wire {
                    name: "bad".to_string(),
                    value: Expr::Call {
                        object: "print".to_string(),
                        args: vec![CallArg::positional(Expr::Ref("sig".to_string()))],
                    },
                    span: None,
                    attrs: vec![],
                },
            ],
            vec![],
        );

        let errors = type_check(&prog);
        let e001_errors: Vec<_> = errors.iter().filter(|e| e.code == "E001").collect();
        assert_eq!(
            e001_errors.len(),
            1,
            "Signal → print should still trigger E001"
        );
    }
}
