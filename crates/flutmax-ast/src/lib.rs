/// flutmax AST (Abstract Syntax Tree)
///
/// Type definitions representing the structure of `.flutmax` source code.
/// Converted from Tree-sitter CST to this AST, then passed to semantic analysis.
///
/// Top-level program structure
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub in_decls: Vec<InDecl>,
    pub out_decls: Vec<OutDecl>,
    pub wires: Vec<Wire>,
    pub destructuring_wires: Vec<DestructuringWire>,
    pub msg_decls: Vec<MsgDecl>,
    pub out_assignments: Vec<OutAssignment>,
    pub direct_connections: Vec<DirectConnection>,
    pub feedback_decls: Vec<FeedbackDecl>,
    pub feedback_assignments: Vec<FeedbackAssignment>,
    pub state_decls: Vec<StateDecl>,
    pub state_assignments: Vec<StateAssignment>,
}

impl Default for Program {
    fn default() -> Self {
        Self::new()
    }
}

impl Program {
    pub fn new() -> Self {
        Self {
            in_decls: Vec::new(),
            out_decls: Vec::new(),
            wires: Vec::new(),
            destructuring_wires: Vec::new(),
            msg_decls: Vec::new(),
            out_assignments: Vec::new(),
            direct_connections: Vec::new(),
            feedback_decls: Vec::new(),
            feedback_assignments: Vec::new(),
            state_decls: Vec::new(),
            state_assignments: Vec::new(),
        }
    }
}

/// Source code location information
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Span {
    pub start_line: usize,
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
}

/// Port type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortType {
    Signal,
    Float,
    Int,
    Bang,
    List,
    Symbol,
}

impl PortType {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "signal" => Some(Self::Signal),
            "float" => Some(Self::Float),
            "int" => Some(Self::Int),
            "bang" => Some(Self::Bang),
            "list" => Some(Self::List),
            "symbol" => Some(Self::Symbol),
            _ => None,
        }
    }

    pub fn is_signal(&self) -> bool {
        matches!(self, Self::Signal)
    }
}

/// Input port declaration: `in 0 (freq): float;`
#[derive(Debug, Clone, PartialEq)]
pub struct InDecl {
    pub index: u32,
    pub name: String,
    pub port_type: PortType,
}

/// Output port declaration: `out 0 (audio): signal;` or `out audio: signal = expr;`
#[derive(Debug, Clone, PartialEq)]
pub struct OutDecl {
    pub index: u32,
    pub name: String,
    pub port_type: PortType,
    pub value: Option<Expr>,
}

/// Wire declaration: `wire osc = cycle~(440);`
/// Optional `.attr()` chain: `wire w = flonum(x).attr(minimum: 0., maximum: 100.);`
#[derive(Debug, Clone, PartialEq)]
pub struct Wire {
    pub name: String,
    pub value: Expr,
    pub span: Option<Span>,
    pub attrs: Vec<AttrPair>,
}

/// Output assignment: `out[0] = osc;`
#[derive(Debug, Clone, PartialEq)]
pub struct OutAssignment {
    pub index: u32,
    pub value: Expr,
    pub span: Option<Span>,
}

/// Direct connection: `node_a.in[0] = trigger;`
#[derive(Debug, Clone, PartialEq)]
pub struct DirectConnection {
    pub target: InputPortAccess,
    pub value: Expr,
}

/// Input port access (lvalue): `node_a.in[0]`
#[derive(Debug, Clone, PartialEq)]
pub struct InputPortAccess {
    pub object: String,
    pub index: u32,
}

/// Output port access (rvalue): `node_a.out[0]`
#[derive(Debug, Clone, PartialEq)]
pub struct OutputPortAccess {
    pub object: String,
    pub index: u32,
}

/// Destructuring wire: `wire (a, b, c) = expr;`
#[derive(Debug, Clone, PartialEq)]
pub struct DestructuringWire {
    pub names: Vec<String>,
    pub value: Expr,
    pub span: Option<Span>,
}

/// Feedback declaration: `feedback fb: signal;`
#[derive(Debug, Clone, PartialEq)]
pub struct FeedbackDecl {
    pub name: String,
    pub port_type: PortType,
    pub span: Option<Span>,
}

/// Feedback assignment: `feedback fb = tapin~(mixed, 1000);`
#[derive(Debug, Clone, PartialEq)]
pub struct FeedbackAssignment {
    pub target: String,
    pub value: Expr,
    pub span: Option<Span>,
}

/// State declaration: `state counter: int = 0;`
#[derive(Debug, Clone, PartialEq)]
pub struct StateDecl {
    pub name: String,
    pub port_type: PortType,
    pub init_value: Expr,
    pub span: Option<Span>,
}

/// State assignment: `state counter = next;`
#[derive(Debug, Clone, PartialEq)]
pub struct StateAssignment {
    pub name: String,
    pub value: Expr,
    pub span: Option<Span>,
}

/// Message declaration: `msg click = "bang";`
/// Optional `.attr()` chain: `msg click = "bang".attr(patching_rect: 100.);`
#[derive(Debug, Clone, PartialEq)]
pub struct MsgDecl {
    pub name: String,
    pub content: String,
    pub span: Option<Span>,
    pub attrs: Vec<AttrPair>,
}

/// Call argument (positional or named)
#[derive(Debug, Clone, PartialEq)]
pub struct CallArg {
    /// None = positional argument, Some = named argument (e.g., `freq: 440`)
    pub name: Option<String>,
    /// Argument value
    pub value: Expr,
}

impl CallArg {
    /// Create a positional argument.
    pub fn positional(value: Expr) -> Self {
        CallArg { name: None, value }
    }

    /// Create a named argument.
    pub fn named(name: impl Into<String>, value: Expr) -> Self {
        CallArg {
            name: Some(name.into()),
            value,
        }
    }
}

/// Expression
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// Object call: `cycle~(440)`, `*~(osc, 0.5)`, `biquad~(input: osc, freq: cutoff)`
    Call { object: String, args: Vec<CallArg> },
    /// Variable reference: `osc`, `freq`
    Ref(String),
    /// Literal value
    Lit(LitValue),
    /// Output port access (rvalue): `node_a.out[0]`
    OutputPortAccess(OutputPortAccess),
    /// Tuple expression: `(x, y, z)` -- converted to `[pack]`
    Tuple(Vec<Expr>),
}

/// Literal value
#[derive(Debug, Clone, PartialEq)]
pub enum LitValue {
    Int(i64),
    Float(f64),
    Str(String),
}

/// Attribute key-value pair: `.attr(key: value, ...)`
#[derive(Debug, Clone, PartialEq)]
pub struct AttrPair {
    pub key: String,
    pub value: AttrValue,
}

/// Attribute value
#[derive(Debug, Clone, PartialEq)]
pub enum AttrValue {
    Int(i64),
    Float(f64),
    Str(String),
    Ident(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_port_type_from_str() {
        assert_eq!(PortType::parse("signal"), Some(PortType::Signal));
        assert_eq!(PortType::parse("float"), Some(PortType::Float));
        assert_eq!(PortType::parse("int"), Some(PortType::Int));
        assert_eq!(PortType::parse("bang"), Some(PortType::Bang));
        assert_eq!(PortType::parse("unknown"), None);
    }

    #[test]
    fn test_program_new() {
        let prog = Program::new();
        assert!(prog.in_decls.is_empty());
        assert!(prog.out_decls.is_empty());
        assert!(prog.wires.is_empty());
        assert!(prog.out_assignments.is_empty());
    }

    #[test]
    fn test_build_l2_ast() {
        // Manually construct the AST for L2_simple_synth.flutmax
        let prog = Program {
            in_decls: vec![InDecl {
                index: 0,
                name: "freq".to_string(),
                port_type: PortType::Float,
            }],
            out_decls: vec![OutDecl {
                index: 0,
                name: "audio".to_string(),
                port_type: PortType::Signal,
                value: None,
            }],
            wires: vec![
                Wire {
                    name: "osc".to_string(),
                    value: Expr::Call {
                        object: "cycle~".to_string(),
                        args: vec![CallArg::positional(Expr::Ref("freq".to_string()))],
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
            destructuring_wires: Vec::new(),
            msg_decls: Vec::new(),
            out_assignments: vec![OutAssignment {
                index: 0,
                value: Expr::Ref("amp".to_string()),
                span: None,
            }],
            direct_connections: Vec::new(),
            feedback_decls: Vec::new(),
            feedback_assignments: Vec::new(),
            state_decls: Vec::new(),
            state_assignments: Vec::new(),
        };

        assert_eq!(prog.in_decls.len(), 1);
        assert_eq!(prog.out_decls.len(), 1);
        assert_eq!(prog.wires.len(), 2);
        assert_eq!(prog.out_assignments.len(), 1);
    }

    #[test]
    fn test_call_arg_helpers() {
        let pos = CallArg::positional(Expr::Lit(LitValue::Int(440)));
        assert_eq!(pos.name, None);
        assert_eq!(pos.value, Expr::Lit(LitValue::Int(440)));

        let named = CallArg::named("freq", Expr::Lit(LitValue::Int(440)));
        assert_eq!(named.name, Some("freq".to_string()));
        assert_eq!(named.value, Expr::Lit(LitValue::Int(440)));
    }
}
