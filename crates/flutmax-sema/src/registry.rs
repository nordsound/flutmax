/// Abstraction registry
///
/// A registry for registering and looking up `in`/`out` declarations
/// across multiple `.flutmax` files during cross-file compilation.
/// Referenced by `build_graph` when resolving object calls to determine
/// `numinlets`/`numoutlets` for Abstractions.

use std::collections::HashMap;

use flutmax_ast::{PortType, Program};

/// Port information
#[derive(Debug, Clone, PartialEq)]
pub struct PortInfo {
    pub index: u32,
    pub name: String,
    pub port_type: PortType,
}

/// Abstraction interface information
#[derive(Debug, Clone, PartialEq)]
pub struct AbstractionInterface {
    pub name: String,
    pub in_ports: Vec<PortInfo>,
    pub out_ports: Vec<PortInfo>,
}

/// Abstraction registry
///
/// Maps filename (without extension) to interface information.
#[derive(Debug)]
pub struct AbstractionRegistry {
    interfaces: HashMap<String, AbstractionInterface>,
}

impl AbstractionRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            interfaces: HashMap::new(),
        }
    }

    /// Extract interface from AST `in`/`out` declarations and register it.
    ///
    /// `name` is the filename (without extension). Referenced as `[name]` in Max.
    pub fn register(&mut self, name: &str, program: &Program) {
        let in_ports: Vec<PortInfo> = program
            .in_decls
            .iter()
            .map(|decl| PortInfo {
                index: decl.index,
                name: decl.name.clone(),
                port_type: decl.port_type,
            })
            .collect();

        let out_ports: Vec<PortInfo> = program
            .out_decls
            .iter()
            .map(|decl| PortInfo {
                index: decl.index,
                name: decl.name.clone(),
                port_type: decl.port_type,
            })
            .collect();

        let iface = AbstractionInterface {
            name: name.to_string(),
            in_ports,
            out_ports,
        };

        self.interfaces.insert(name.to_string(), iface);
    }

    /// Look up an Abstraction by name.
    pub fn lookup(&self, name: &str) -> Option<&AbstractionInterface> {
        self.interfaces.get(name)
    }

    /// Returns whether a name is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.interfaces.contains_key(name)
    }

    /// Register an interface directly.
    ///
    /// Used for tests and internal purposes when registering an interface without going through AST.
    pub fn register_interface(&mut self, iface: AbstractionInterface) {
        self.interfaces.insert(iface.name.clone(), iface);
    }
}

impl Default for AbstractionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flutmax_ast::*;

    /// Build the AST for oscillator.flutmax
    fn make_oscillator_program() -> Program {
        Program {
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
            wires: vec![Wire {
                name: "osc".to_string(),
                value: Expr::Call {
                    object: "cycle~".to_string(),
                    args: vec![CallArg::positional(Expr::Ref("freq".to_string()))],
                },
                span: None,
                attrs: vec![],
            }],
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![OutAssignment {
                index: 0,
                value: Expr::Ref("osc".to_string()),
                span: None,
            }],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        }
    }

    /// Build the AST for filter.flutmax with multiple ports
    fn make_filter_program() -> Program {
        Program {
            in_decls: vec![
                InDecl {
                    index: 0,
                    name: "input_sig".to_string(),
                    port_type: PortType::Signal,
                },
                InDecl {
                    index: 1,
                    name: "cutoff".to_string(),
                    port_type: PortType::Float,
                },
                InDecl {
                    index: 2,
                    name: "q_factor".to_string(),
                    port_type: PortType::Float,
                },
            ],
            out_decls: vec![
                OutDecl {
                    index: 0,
                    name: "lowpass".to_string(),
                    port_type: PortType::Signal,
                value: None,
                },
                OutDecl {
                    index: 1,
                    name: "highpass".to_string(),
                    port_type: PortType::Signal,
                value: None,
                },
            ],
            wires: vec![],
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        }
    }

    #[test]
    fn test_register_from_ast() {
        let mut registry = AbstractionRegistry::new();
        let prog = make_oscillator_program();
        registry.register("oscillator", &prog);

        let iface = registry.lookup("oscillator").unwrap();
        assert_eq!(iface.name, "oscillator");
        assert_eq!(iface.in_ports.len(), 1);
        assert_eq!(iface.out_ports.len(), 1);

        assert_eq!(iface.in_ports[0].index, 0);
        assert_eq!(iface.in_ports[0].name, "freq");
        assert_eq!(iface.in_ports[0].port_type, PortType::Float);

        assert_eq!(iface.out_ports[0].index, 0);
        assert_eq!(iface.out_ports[0].name, "audio");
        assert_eq!(iface.out_ports[0].port_type, PortType::Signal);
    }

    #[test]
    fn test_register_multiple_ports() {
        let mut registry = AbstractionRegistry::new();
        let prog = make_filter_program();
        registry.register("filter", &prog);

        let iface = registry.lookup("filter").unwrap();
        assert_eq!(iface.in_ports.len(), 3);
        assert_eq!(iface.out_ports.len(), 2);

        assert_eq!(iface.in_ports[0].port_type, PortType::Signal);
        assert_eq!(iface.in_ports[1].port_type, PortType::Float);
        assert_eq!(iface.in_ports[2].port_type, PortType::Float);

        assert_eq!(iface.out_ports[0].port_type, PortType::Signal);
        assert_eq!(iface.out_ports[1].port_type, PortType::Signal);
    }

    #[test]
    fn test_lookup_existing() {
        let mut registry = AbstractionRegistry::new();
        let prog = make_oscillator_program();
        registry.register("oscillator", &prog);

        assert!(registry.lookup("oscillator").is_some());
    }

    #[test]
    fn test_lookup_missing() {
        let registry = AbstractionRegistry::new();
        assert!(registry.lookup("nonexistent").is_none());
    }

    #[test]
    fn test_contains() {
        let mut registry = AbstractionRegistry::new();
        let prog = make_oscillator_program();
        registry.register("oscillator", &prog);

        assert!(registry.contains("oscillator"));
        assert!(!registry.contains("nonexistent"));
    }

    #[test]
    fn test_multiple_registrations() {
        let mut registry = AbstractionRegistry::new();
        registry.register("oscillator", &make_oscillator_program());
        registry.register("filter", &make_filter_program());

        assert!(registry.contains("oscillator"));
        assert!(registry.contains("filter"));
        assert!(!registry.contains("other"));
    }

    #[test]
    fn test_register_empty_program() {
        let mut registry = AbstractionRegistry::new();
        let prog = Program::new();
        registry.register("empty", &prog);

        let iface = registry.lookup("empty").unwrap();
        assert_eq!(iface.in_ports.len(), 0);
        assert_eq!(iface.out_ports.len(), 0);
    }

    #[test]
    fn test_default() {
        let registry = AbstractionRegistry::default();
        assert!(registry.lookup("anything").is_none());
    }
}
