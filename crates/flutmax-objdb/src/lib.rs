pub mod parser;

use std::collections::HashMap;

/// Port (inlet/outlet) type for Max objects
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortType {
    /// Signal only (audio rate only)
    Signal,
    /// Signal + Float dual (controlled by float when no signal connection)
    SignalFloat,
    /// Int + Signal dual
    IntSignal,
    /// Float only
    Float,
    /// Int only
    Int,
    /// Bang only
    Bang,
    /// List only
    List,
    /// Symbol only
    Symbol,
    /// Any message (bang, int, float, list, symbol)
    Any,
    /// Multi-channel signal
    MultiChannelSignal,
    /// Multi-channel signal + float
    MultiChannelSignalFloat,
    /// Depends on argument type (placeholder for variable objects)
    Dynamic,
    /// Inactive inlet/outlet
    Inactive,
}

impl PortType {
    /// Convert XML type attribute string to PortType.
    /// Normalizes case differences and notation variants (/, " or ", ", ").
    pub fn from_xml_type(type_str: &str) -> Self {
        let normalized = type_str
            .to_lowercase()
            .replace(" / ", "/")
            .replace(", ", "/")
            .replace(" or ", "/");
        let normalized = normalized.trim();

        match normalized {
            "signal" => PortType::Signal,
            "signal/float" | "float/signal" | "signal/float/symbol" | "signal/float/timevalue" => {
                PortType::SignalFloat
            }
            "int/signal" | "signal/int" => PortType::IntSignal,
            "float" | "double" => PortType::Float,
            "int" | "long" | "int/voice" => PortType::Int,
            "bang" => PortType::Bang,
            "list" => PortType::List,
            "symbol" => PortType::Symbol,
            "anything" | "message" | "bang/int" | "bang/anything" | "int/float"
            | "int/float/list" | "int/list" | "float/list" | "int/float/sig" | "signal/msg"
            | "signal/message" | "signal/list" | "dictionary" | "dict" | "setvalue"
            | "midievent" | "matrix" => PortType::Any,
            "multi-channel signal" | "signal/multi-channel signal" => PortType::MultiChannelSignal,
            "multi-channel signal/float" | "multi-channel signal/message" => {
                PortType::MultiChannelSignalFloat
            }
            "inlet_type" | "outlet_type" | "objarg_type" => PortType::Dynamic,
            "inactive" => PortType::Inactive,
            "" => PortType::Any,
            _ => PortType::Any,
        }
    }

    /// Whether this port accepts Signal
    pub fn accepts_signal(&self) -> bool {
        matches!(
            self,
            PortType::Signal
                | PortType::SignalFloat
                | PortType::IntSignal
                | PortType::MultiChannelSignal
                | PortType::MultiChannelSignalFloat
        )
    }

    /// Whether this port accepts Control messages
    pub fn accepts_control(&self) -> bool {
        matches!(
            self,
            PortType::SignalFloat
                | PortType::IntSignal
                | PortType::Float
                | PortType::Int
                | PortType::Bang
                | PortType::List
                | PortType::Symbol
                | PortType::Any
                | PortType::Dynamic
                | PortType::MultiChannelSignalFloat
        )
    }
}

/// Module type
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Module {
    Max,
    Msp,
    Other(String),
}

impl Module {
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "max" => Module::Max,
            "msp" => Module::Msp,
            other => Module::Other(other.to_string()),
        }
    }
}

/// Inlet definition
#[derive(Debug, Clone)]
pub struct PortDef {
    /// Port ID (0-based)
    pub id: u32,
    /// Port type
    pub port_type: PortType,
    /// Whether this is a hot inlet (triggers output on message receipt). Always false for outlets.
    pub is_hot: bool,
    /// Description (digest text)
    pub description: String,
}

/// Inlet configuration (fixed or variable count)
#[derive(Debug, Clone)]
pub enum InletSpec {
    /// Fixed number of inlets (e.g., cycle~ = 2, biquad~ = 6)
    Fixed(Vec<PortDef>),
    /// Variable inlets depending on argument count (e.g., pack)
    Variable {
        /// Representative inlet definitions described in XML
        defaults: Vec<PortDef>,
        /// Minimum number of inlets
        min_inlets: u32,
    },
}

/// Outlet configuration (fixed or variable count)
#[derive(Debug, Clone)]
pub enum OutletSpec {
    /// Fixed number of outlets
    Fixed(Vec<PortDef>),
    /// Variable outlets depending on argument count (e.g., trigger)
    Variable {
        /// Representative outlet definitions described in XML
        defaults: Vec<PortDef>,
        /// Minimum number of outlets
        min_outlets: u32,
    },
}

/// Object argument definition
#[derive(Debug, Clone)]
pub struct ArgDef {
    pub name: String,
    pub arg_type: String,
    pub optional: bool,
}

/// Object definition
#[derive(Debug, Clone)]
pub struct ObjectDef {
    /// Object name (e.g., "cycle~", "pack", "trigger")
    pub name: String,
    /// Module (max, msp, etc.)
    pub module: Module,
    /// Category (e.g., "MSP Synthesis", "Lists")
    pub category: String,
    /// Short description
    pub digest: String,
    /// Inlet definitions
    pub inlets: InletSpec,
    /// Outlet definitions
    pub outlets: OutletSpec,
    /// Argument definitions
    pub args: Vec<ArgDef>,
}

impl ObjectDef {
    /// Whether this ObjectDef has variable inlets
    pub fn has_variable_inlets(&self) -> bool {
        matches!(self.inlets, InletSpec::Variable { .. })
    }

    /// Whether this ObjectDef has variable outlets
    pub fn has_variable_outlets(&self) -> bool {
        matches!(self.outlets, OutletSpec::Variable { .. })
    }

    /// Returns the inlet count in the default configuration
    pub fn default_inlet_count(&self) -> usize {
        match &self.inlets {
            InletSpec::Fixed(ports) => ports.len(),
            InletSpec::Variable { defaults, .. } => defaults.len(),
        }
    }

    /// Returns the outlet count in the default configuration
    pub fn default_outlet_count(&self) -> usize {
        match &self.outlets {
            OutletSpec::Fixed(ports) => ports.len(),
            OutletSpec::Variable { defaults, .. } => defaults.len(),
        }
    }
}

/// Object definition database
#[derive(Debug)]
pub struct ObjectDb {
    objects: HashMap<String, ObjectDef>,
}

impl ObjectDb {
    /// Create an empty database
    pub fn new() -> Self {
        ObjectDb {
            objects: HashMap::new(),
        }
    }

    /// Insert an ObjectDef
    pub fn insert(&mut self, def: ObjectDef) {
        self.objects.insert(def.name.clone(), def);
    }

    /// Look up by object name
    pub fn lookup(&self, name: &str) -> Option<&ObjectDef> {
        self.objects.get(name)
    }

    /// Number of registered objects
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    /// Whether the database is empty
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// Iterator over all object names
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.objects.keys().map(|s| s.as_str())
    }

    /// Iterator over ObjectDefs filtered by module
    pub fn by_module(&self, module: &Module) -> Vec<&ObjectDef> {
        self.objects
            .values()
            .filter(|def| &def.module == module)
            .collect()
    }
}

impl Default for ObjectDb {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_port_type_from_xml_signal() {
        assert_eq!(PortType::from_xml_type("signal"), PortType::Signal);
        assert_eq!(PortType::from_xml_type("Signal"), PortType::Signal);
    }

    #[test]
    fn test_port_type_from_xml_signal_float_variants() {
        assert_eq!(
            PortType::from_xml_type("signal/float"),
            PortType::SignalFloat
        );
        assert_eq!(
            PortType::from_xml_type("Signal/Float"),
            PortType::SignalFloat
        );
        assert_eq!(
            PortType::from_xml_type("signal, float"),
            PortType::SignalFloat
        );
        assert_eq!(
            PortType::from_xml_type("signal or float"),
            PortType::SignalFloat
        );
        assert_eq!(
            PortType::from_xml_type("float/signal"),
            PortType::SignalFloat
        );
        assert_eq!(
            PortType::from_xml_type("float / signal"),
            PortType::SignalFloat
        );
    }

    #[test]
    fn test_port_type_from_xml_int_signal_variants() {
        assert_eq!(PortType::from_xml_type("int/signal"), PortType::IntSignal);
        assert_eq!(PortType::from_xml_type("signal/int"), PortType::IntSignal);
        assert_eq!(PortType::from_xml_type("signal, int"), PortType::IntSignal);
        assert_eq!(PortType::from_xml_type("int / signal"), PortType::IntSignal);
    }

    #[test]
    fn test_port_type_from_xml_dynamic() {
        assert_eq!(PortType::from_xml_type("INLET_TYPE"), PortType::Dynamic);
        assert_eq!(PortType::from_xml_type("OUTLET_TYPE"), PortType::Dynamic);
    }

    #[test]
    fn test_port_type_from_xml_control_types() {
        assert_eq!(PortType::from_xml_type("float"), PortType::Float);
        assert_eq!(PortType::from_xml_type("int"), PortType::Int);
        assert_eq!(PortType::from_xml_type("bang"), PortType::Bang);
        assert_eq!(PortType::from_xml_type("list"), PortType::List);
        assert_eq!(PortType::from_xml_type("symbol"), PortType::Symbol);
        assert_eq!(PortType::from_xml_type("anything"), PortType::Any);
    }

    #[test]
    fn test_port_type_accepts_signal() {
        assert!(PortType::Signal.accepts_signal());
        assert!(PortType::SignalFloat.accepts_signal());
        assert!(PortType::IntSignal.accepts_signal());
        assert!(!PortType::Float.accepts_signal());
        assert!(!PortType::Any.accepts_signal());
        assert!(!PortType::Dynamic.accepts_signal());
    }

    #[test]
    fn test_port_type_accepts_control() {
        assert!(!PortType::Signal.accepts_control());
        assert!(PortType::SignalFloat.accepts_control());
        assert!(PortType::IntSignal.accepts_control());
        assert!(PortType::Float.accepts_control());
        assert!(PortType::Any.accepts_control());
        assert!(PortType::Dynamic.accepts_control());
    }

    #[test]
    fn test_module_from_str() {
        assert_eq!(Module::parse("max"), Module::Max);
        assert_eq!(Module::parse("msp"), Module::Msp);
        assert_eq!(Module::parse("jit"), Module::Other("jit".to_string()));
    }

    #[test]
    fn test_object_db_basic_operations() {
        let mut db = ObjectDb::new();
        assert!(db.is_empty());
        assert_eq!(db.len(), 0);

        let def = ObjectDef {
            name: "cycle~".to_string(),
            module: Module::Msp,
            category: "MSP Synthesis".to_string(),
            digest: "Sinusoidal oscillator".to_string(),
            inlets: InletSpec::Fixed(vec![
                PortDef {
                    id: 0,
                    port_type: PortType::SignalFloat,
                    is_hot: true,
                    description: "Frequency".to_string(),
                },
                PortDef {
                    id: 1,
                    port_type: PortType::SignalFloat,
                    is_hot: false,
                    description: "Phase (0-1)".to_string(),
                },
            ]),
            outlets: OutletSpec::Fixed(vec![PortDef {
                id: 0,
                port_type: PortType::Signal,
                is_hot: false,
                description: "Output".to_string(),
            }]),
            args: vec![],
        };

        db.insert(def);
        assert_eq!(db.len(), 1);
        assert!(!db.is_empty());

        let looked_up = db.lookup("cycle~").unwrap();
        assert_eq!(looked_up.name, "cycle~");
        assert_eq!(looked_up.module, Module::Msp);
        assert_eq!(looked_up.default_inlet_count(), 2);
        assert_eq!(looked_up.default_outlet_count(), 1);
        assert!(!looked_up.has_variable_inlets());

        assert!(db.lookup("nonexistent").is_none());
    }
}
