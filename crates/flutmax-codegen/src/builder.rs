/// AST -> PatchGraph conversion
///
/// Converts `Program` (AST) to `PatchGraph`.
/// Each InDecl/OutDecl/Wire/OutAssignment is converted to corresponding nodes and edges,
/// then `insert_triggers()` auto-inserts triggers for fanouts.
use std::collections::{HashMap, HashSet};

#[allow(unused_imports)] // CallArg used in tests
use flutmax_ast::{
    CallArg, DestructuringWire, DirectConnection, Expr, FeedbackAssignment, FeedbackDecl, InDecl,
    LitValue, MsgDecl, OutAssignment, OutDecl, PortType, Program, StateAssignment, StateDecl, Wire,
};
use flutmax_objdb::{InletSpec, ObjectDb, OutletSpec};
use flutmax_sema::graph::{NodePurity, PatchEdge, PatchGraph, PatchNode};
use flutmax_sema::registry::AbstractionRegistry;
use flutmax_sema::trigger::insert_triggers;

/// Code file mapping. Filename -> code content.
/// Used when referencing external code files in `v8.codebox` and `codebox` (gen~).
pub type CodeFiles = HashMap<String, String>;

/// Build error
#[derive(Debug)]
pub enum BuildError {
    /// Referenced an undefined variable
    UndefinedRef(String),
    /// Output port index out of range
    OutletIndexOutOfRange(u32),
    /// E004: no out declaration corresponding to out[N]
    NoOutDeclaration(u32),
    /// E006: destructuring LHS count does not match RHS outlet count
    DestructuringCountMismatch { expected: usize, got: usize },
    /// E009: Abstraction argument count does not match in_ports count
    AbstractionArgCountMismatch {
        name: String,
        expected: usize,
        got: usize,
    },
    /// E013: multiple assignments to the same feedback variable
    DuplicateFeedbackAssignment(String),
    /// E007: port index out of range
    InvalidPortIndex {
        node: String,
        port: String,
        index: u32,
        max: u32,
    },
    /// E020: bare reference to multi-outlet node (.out[N] required)
    BareMultiOutletRef { name: String, num_outlets: u32 },
    /// E019: multiple assignments to the same state
    DuplicateStateAssignment(String),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::UndefinedRef(name) => write!(f, "undefined reference: {}", name),
            BuildError::OutletIndexOutOfRange(idx) => {
                write!(f, "outlet index out of range: {}", idx)
            }
            BuildError::NoOutDeclaration(idx) => {
                write!(f, "E004: out[{}] has no corresponding out declaration", idx)
            }
            BuildError::DestructuringCountMismatch { expected, got } => {
                write!(
                    f,
                    "E006: destructuring count mismatch: expected {} names, got {}",
                    expected, got
                )
            }
            BuildError::AbstractionArgCountMismatch {
                name,
                expected,
                got,
            } => {
                write!(
                    f,
                    "E009: abstraction '{}' expects {} arguments, got {}",
                    name, expected, got
                )
            }
            BuildError::DuplicateFeedbackAssignment(name) => {
                write!(f, "E013: duplicate feedback assignment to '{}'", name)
            }
            BuildError::InvalidPortIndex {
                node,
                port,
                index,
                max,
            } => {
                write!(
                    f,
                    "E007: port index out of range: {}.{}[{}] (max: {})",
                    node, port, index, max
                )
            }
            BuildError::BareMultiOutletRef { name, num_outlets } => {
                write!(
                    f,
                    "E020: bare reference to multi-outlet node '{}' ({} outlets); use .out[N] to specify which outlet",
                    name, num_outlets
                )
            }
            BuildError::DuplicateStateAssignment(name) => {
                write!(f, "E019: duplicate state assignment to '{}'", name)
            }
        }
    }
}

impl std::error::Error for BuildError {}

/// Build warning
#[derive(Debug, Clone)]
pub enum BuildWarning {
    /// W001: duplicate connection to the same inlet
    DuplicateInletConnection {
        node_id: String,
        inlet: u32,
        count: usize,
    },
}

impl std::fmt::Display for BuildWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildWarning::DuplicateInletConnection {
                node_id,
                inlet,
                count,
            } => {
                write!(
                    f,
                    "W001: {} connections to {}.in[{}]",
                    count, node_id, inlet
                )
            }
        }
    }
}

/// Build result (graph + warnings)
pub struct BuildResult {
    pub graph: PatchGraph,
    pub warnings: Vec<BuildWarning>,
}

/// Builder that constructs a PatchGraph from AST.
struct GraphBuilder<'a> {
    graph: PatchGraph,
    /// Sequential node ID counter
    next_id: u32,
    /// Name -> (node_id, outlet_index) mapping
    /// Used to look up nodes from inlet names and wire names
    name_map: HashMap<String, (String, u32)>,
    /// out_decl index -> node_id
    outlet_nodes: HashMap<u32, String>,
    /// Abstraction registry (used during multi-file compilation)
    registry: Option<&'a AbstractionRegistry>,
    /// feedback name -> tapin~ node ID mapping
    feedback_map: HashMap<String, String>,
    /// Set of already-assigned feedback names (E013: duplicate detection)
    assigned_feedbacks: HashSet<String>,
    /// Set of names generated by destructuring assignments (excluded from E020)
    destructured_names: HashSet<String>,
    /// Set of already-assigned state names (E019: duplicate detection)
    assigned_states: HashSet<String>,
    /// Tuple wire name -> pack type arguments for each element ("i", "f", "s")
    /// Used for typed unpack generation during destructuring
    tuple_type_args: HashMap<String, Vec<String>>,
    /// Code file mapping (for codebox)
    code_files: Option<&'a CodeFiles>,
    /// Object definition database (used for inlet/outlet count inference)
    objdb: Option<&'a ObjectDb>,
}

impl<'a> GraphBuilder<'a> {
    fn new(
        registry: Option<&'a AbstractionRegistry>,
        code_files: Option<&'a CodeFiles>,
        objdb: Option<&'a ObjectDb>,
    ) -> Self {
        Self {
            graph: PatchGraph::new(),
            next_id: 1,
            name_map: HashMap::new(),
            outlet_nodes: HashMap::new(),
            registry,
            feedback_map: HashMap::new(),
            assigned_feedbacks: HashSet::new(),
            destructured_names: HashSet::new(),
            assigned_states: HashSet::new(),
            tuple_type_args: HashMap::new(),
            code_files,
            objdb,
        }
    }

    /// Generate a new node ID.
    fn gen_id(&mut self) -> String {
        let id = format!("obj-{}", self.next_id);
        self.next_id += 1;
        id
    }

    /// Convert an InDecl to a node.
    fn add_inlet(&mut self, decl: &InDecl) {
        let id = self.gen_id();
        let is_signal = decl.port_type.is_signal();
        let object_name = if is_signal { "inlet~" } else { "inlet" };
        let num_inlets = if is_signal { 1 } else { 0 };
        let node = PatchNode {
            id: id.clone(),
            object_name: object_name.to_string(),
            args: vec![],
            num_inlets,
            num_outlets: 1,
            is_signal,
            varname: None,
            hot_inlets: default_hot_inlets(object_name, num_inlets),
            purity: classify_purity(object_name),
            attrs: vec![],
            code: None,
        };
        self.graph.add_node(node);
        // Register inlet name for lookup
        self.name_map.insert(decl.name.clone(), (id, 0));
    }

    /// Convert an OutDecl to a node.
    fn add_outlet(&mut self, decl: &OutDecl) {
        let id = self.gen_id();
        let is_signal = decl.port_type.is_signal();
        let object_name = if is_signal { "outlet~" } else { "outlet" };
        let node = PatchNode {
            id: id.clone(),
            object_name: object_name.to_string(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 0,
            is_signal,
            varname: None,
            hot_inlets: default_hot_inlets(object_name, 1),
            purity: classify_purity(object_name),
            attrs: vec![],
            code: None,
        };
        self.graph.add_node(node);
        self.outlet_nodes.insert(decl.index, id);
    }

    /// Process a MsgDecl. Generate a Max message box node.
    fn add_msg(&mut self, decl: &MsgDecl) {
        let id = self.gen_id();
        let attrs = decl
            .attrs
            .iter()
            .map(|a| (a.key.clone(), format_attr_value(&a.value)))
            .collect();
        let node = PatchNode {
            id: id.clone(),
            object_name: "message".to_string(),
            args: vec![decl.content.clone()],
            num_inlets: 2, // inlet 0 = hot (bang/message), inlet 1 = cold (set)
            num_outlets: 1,
            is_signal: false,
            varname: Some(decl.name.clone()),
            hot_inlets: vec![true, false],
            purity: classify_purity("message"),
            attrs,
            code: None,
        };
        self.graph.add_node(node);
        self.name_map.insert(decl.name.clone(), (id, 0));
    }

    /// Process a Wire. Evaluate the expression and generate nodes/edges.
    fn add_wire(&mut self, wire: &Wire) -> Result<(), BuildError> {
        // For Tuples, record each element's type argument (for propagation during destructuring)
        if let Expr::Tuple(elements) = &wire.value {
            let type_args: Vec<String> = elements.iter().map(infer_pack_type_arg).collect();
            self.tuple_type_args.insert(wire.name.clone(), type_args);
        }

        let (node_id, outlet) = self.resolve_expr(&wire.value)?;
        // Set wire name as varname (for debugging in Max)
        if let Some(node) = self.graph.nodes.iter_mut().find(|n| n.id == node_id) {
            node.varname = Some(wire.name.clone());
        }
        // Transfer .attr() chain attributes to the node
        if !wire.attrs.is_empty() {
            if let Some(node) = self.graph.nodes.iter_mut().find(|n| n.id == node_id) {
                node.attrs = wire
                    .attrs
                    .iter()
                    .map(|a| (a.key.clone(), format_attr_value(&a.value)))
                    .collect();
            }
        }
        self.name_map.insert(wire.name.clone(), (node_id, outlet));
        Ok(())
    }

    /// Process an OutAssignment. Connect the wire's output to the outlet node.
    fn add_out_assignment(&mut self, assign: &OutAssignment) -> Result<(), BuildError> {
        let (source_id, source_outlet) = self.resolve_expr(&assign.value)?;
        let dest_id = self
            .outlet_nodes
            .get(&assign.index)
            .ok_or(BuildError::NoOutDeclaration(assign.index))?
            .clone();

        self.graph.add_edge(PatchEdge {
            source_id,
            source_outlet,
            dest_id,
            dest_inlet: 0,
            is_feedback: false,
            order: None,
        });
        Ok(())
    }

    /// Resolve an expression. Generate nodes if needed, and return (node_id, outlet_index).
    fn resolve_expr(&mut self, expr: &Expr) -> Result<(String, u32), BuildError> {
        match expr {
            Expr::Ref(name) => {
                let (node_id, outlet_index) = self
                    .name_map
                    .get(name)
                    .ok_or_else(|| BuildError::UndefinedRef(name.clone()))?
                    .clone();

                // Bare references are always interpreted as outlet 0 (E020 removed).
                // Bare references are OK even for multi-outlet nodes.
                // Use .out[N] to access outlet 1+.

                Ok((node_id, outlet_index))
            }
            Expr::Call { object, args } => {
                let id = self.gen_id();
                let max_name = resolve_max_object_name(object);
                let is_signal = max_name.ends_with('~');

                // Collect literal arguments as part of object text
                let mut lit_args: Vec<String> = Vec::new();
                // Collect Ref arguments to create edges later
                let mut ref_connections: Vec<(String, u32, u32)> = Vec::new(); // (source_node, source_outlet, dest_inlet)

                // gen~, mc.gen~ and rnbo~ take a literal first argument
                // (the subpatcher name) that becomes part of the object text
                // rather than consuming an inlet. For example,
                // `gen~("exciter_pluck", vel_sig, brightness)` produces:
                //   object text: "gen~ exciter_pluck"
                //   vel_sig → inlet 0, brightness → inlet 1
                let has_name_arg = matches!(max_name, "gen~" | "mc.gen~" | "rnbo~");
                let mut lit_count: u32 = 0;

                for (i, arg) in args.iter().enumerate() {
                    // Named argument → resolve inlet index from objdb or AbstractionRegistry;
                    // positional argument → use index directly.
                    let inlet_idx = if let Some(ref name) = arg.name {
                        resolve_inlet_name(max_name, name, self.objdb)
                            .or_else(|| resolve_abstraction_inlet_name(object, name, self.registry))
                            .unwrap_or(i as u32)
                    } else if has_name_arg {
                        // Literal name args don't consume inlets, so subtract their count.
                        (i as u32).saturating_sub(lit_count)
                    } else {
                        i as u32
                    };

                    match &arg.value {
                        Expr::Lit(lit) => {
                            lit_args.push(format_lit(lit));
                            if has_name_arg {
                                lit_count += 1;
                            }
                        }
                        Expr::Ref(name) => {
                            let (ref_node_id, ref_outlet) = self
                                .name_map
                                .get(name)
                                .ok_or_else(|| BuildError::UndefinedRef(name.clone()))?
                                .clone();
                            ref_connections.push((ref_node_id, ref_outlet, inlet_idx));
                        }
                        Expr::Call { .. } => {
                            // Recursively resolve nested calls
                            let (nested_id, nested_outlet) = self.resolve_expr(&arg.value)?;
                            ref_connections.push((nested_id, nested_outlet, inlet_idx));
                        }
                        Expr::OutputPortAccess(opa) => {
                            // output_port_access: resolve name.out[N]
                            let (ref_node_id, _) = self
                                .name_map
                                .get(&opa.object)
                                .ok_or_else(|| BuildError::UndefinedRef(opa.object.clone()))?
                                .clone();
                            ref_connections.push((ref_node_id, opa.index, inlet_idx));
                        }
                        Expr::Tuple(_) => {
                            // Recursively resolve nested tuple expressions
                            let (nested_id, nested_outlet) = self.resolve_expr(&arg.value)?;
                            ref_connections.push((nested_id, nested_outlet, inlet_idx));
                        }
                    }
                }

                // Check the Abstraction registry.
                // If object (name before alias resolution) is registered in the registry,
                // determine numinlets/numoutlets from its interface.
                // However, if the name changed through alias resolution (sub->-, add->+, etc.),
                // it is a built-in object and not treated as an Abstraction.
                let abstraction_info = if max_name == object {
                    self.registry.and_then(|reg| reg.lookup(object))
                } else {
                    // Alias-resolved name — this is a built-in Max object, not an abstraction
                    None
                };

                // E009: Abstraction argument count check
                if let Some(iface) = abstraction_info {
                    let expected = iface.in_ports.len();
                    let got = args.len();
                    if expected != got {
                        return Err(BuildError::AbstractionArgCountMismatch {
                            name: object.clone(),
                            expected,
                            got,
                        });
                    }
                }

                // For gen~/mc.gen~/rnbo~, literal name args are part of the object
                // text rather than inlets, so subtract them from the arg count.
                let effective_arg_count = if has_name_arg {
                    (args.len() as u32).saturating_sub(lit_count)
                } else {
                    args.len() as u32
                };

                // Estimate inlet/outlet count
                let (max_inlet, num_outlets, is_signal) = if let Some(iface) = abstraction_info {
                    // Abstraction: determined from interface
                    let num_in = iface.in_ports.len() as u32;
                    let num_out = iface.out_ports.len() as u32;
                    let sig = iface
                        .out_ports
                        .first()
                        .map(|p| p.port_type.is_signal())
                        .unwrap_or(false);
                    // Inlet count: use at least the number of interface in_ports
                    let max_from_refs = ref_connections
                        .iter()
                        .map(|(_, _, inlet)| *inlet + 1)
                        .max()
                        .unwrap_or(0);
                    let inlets =
                        std::cmp::max(std::cmp::max(max_from_refs, effective_arg_count), num_in);
                    (inlets, num_out, sig)
                } else {
                    // Normal Max object
                    let inlet_count = if ref_connections.is_empty() && lit_args.is_empty() {
                        infer_num_inlets(max_name, &lit_args, self.objdb)
                    } else {
                        let max_from_refs = ref_connections
                            .iter()
                            .map(|(_, _, inlet)| *inlet + 1)
                            .max()
                            .unwrap_or(0);
                        std::cmp::max(
                            std::cmp::max(max_from_refs, effective_arg_count),
                            infer_num_inlets(max_name, &lit_args, self.objdb),
                        )
                    };
                    let outlet_count = infer_num_outlets(max_name, &lit_args, self.objdb);
                    (inlet_count, outlet_count, is_signal)
                };

                // For Abstractions, use the name before alias resolution.
                // Max references the filename as the object name, like `[oscillator 440]`.
                let object_name = if abstraction_info.is_some() {
                    object.to_string()
                } else {
                    max_name.to_string()
                };

                let mut node = PatchNode {
                    id: id.clone(),
                    object_name: object_name.clone(),
                    args: lit_args.clone(),
                    num_inlets: max_inlet,
                    num_outlets,
                    is_signal,
                    varname: None,
                    hot_inlets: default_hot_inlets(&object_name, max_inlet),
                    purity: classify_purity(&object_name),
                    attrs: vec![],
                    code: None,
                };

                // Codebox: resolve code file reference and infer port counts
                if matches!(max_name, "v8.codebox" | "codebox") {
                    if let Some(code_files) = self.code_files {
                        if let Some(filename) = lit_args.first() {
                            if let Some(code_content) = code_files.get(filename.as_str()) {
                                node.code = Some(code_content.clone());
                                node.args.clear();
                                // gen~ codebox: infer inlet/outlet counts from in1..inN / out1..outN
                                if max_name == "codebox" {
                                    let (inlets, outlets) = infer_codebox_ports(code_content);
                                    node.num_inlets = inlets;
                                    node.num_outlets = outlets;
                                }
                            }
                        }
                    }
                }

                self.graph.add_node(node);

                // Create edges from Ref arguments
                for (source_id, source_outlet, dest_inlet) in ref_connections {
                    self.graph.add_edge(PatchEdge {
                        source_id,
                        source_outlet,
                        dest_id: id.clone(),
                        dest_inlet,
                        is_feedback: false,
                        order: None,
                    });
                }

                Ok((id, 0))
            }
            Expr::Lit(lit) => {
                // Generate literal expression as message/number box node
                let id = self.gen_id();
                let (object_name, arg_str, is_signal) = match lit {
                    LitValue::Int(v) => ("message".to_string(), v.to_string(), false),
                    LitValue::Float(_) => ("message".to_string(), format_lit(lit), false),
                    LitValue::Str(s) => ("message".to_string(), s.clone(), false),
                };
                let node = PatchNode {
                    id: id.clone(),
                    object_name,
                    args: vec![arg_str],
                    num_inlets: 1,
                    num_outlets: 1,
                    is_signal,
                    varname: None,
                    hot_inlets: default_hot_inlets("message", 1),
                    purity: classify_purity("message"),
                    attrs: vec![],
                    code: None,
                };
                self.graph.add_node(node);
                Ok((id, 0))
            }
            Expr::OutputPortAccess(opa) => {
                // output_port_access: resolve name.out[N]
                let (node_id, _) = self
                    .name_map
                    .get(&opa.object)
                    .ok_or_else(|| BuildError::UndefinedRef(opa.object.clone()))?
                    .clone();
                Ok((node_id, opa.index))
            }
            Expr::Tuple(elements) => {
                let id = self.gen_id();
                let num_elements = elements.len() as u32;

                // Resolve each element and create edges
                let mut ref_connections: Vec<(String, u32, u32)> = Vec::new();
                let mut type_args: Vec<String> = Vec::new();
                for (i, elem) in elements.iter().enumerate() {
                    let (elem_id, elem_outlet) = self.resolve_expr(elem)?;
                    ref_connections.push((elem_id, elem_outlet, i as u32));
                    // Infer type from expression to determine type argument
                    type_args.push(infer_pack_type_arg(elem));
                }

                let node = PatchNode {
                    id: id.clone(),
                    object_name: "pack".to_string(),
                    args: type_args,
                    num_inlets: num_elements,
                    num_outlets: 1,
                    is_signal: false,
                    varname: None,
                    hot_inlets: default_hot_inlets("pack", num_elements),
                    purity: classify_purity("pack"),
                    attrs: vec![],
                    code: None,
                };
                self.graph.add_node(node);

                for (source_id, source_outlet, dest_inlet) in ref_connections {
                    self.graph.add_edge(PatchEdge {
                        source_id,
                        source_outlet,
                        dest_id: id.clone(),
                        dest_inlet,
                        is_feedback: false,
                        order: None,
                    });
                }

                Ok((id, 0))
            }
        }
    }

    /// Process a DestructuringWire.
    /// Resolve the value expression and map each name to a different outlet of that node.
    ///
    /// For `wire (a, b) = unpack(coords);`:
    ///   - `unpack(coords)` generates an unpack node
    ///   - a → (unpack_id, 0), b → (unpack_id, 1)
    ///
    /// For `wire (a, b) = some_ref;`:
    ///   - After resolving some_ref, automatically insert an unpack node
    ///   - a → (unpack_id, 0), b → (unpack_id, 1)
    fn add_destructuring_wire(&mut self, dw: &DestructuringWire) -> Result<(), BuildError> {
        let (source_id, _source_outlet) = self.resolve_expr(&dw.value)?;
        let num_names = dw.names.len() as u32;

        // E006: Destructuring count check
        // If the RHS node num_outlets is known (not the default 1),
        // check if it matches the LHS name count.
        let resolved_node = self.graph.nodes.iter().find(|n| n.id == source_id);
        if let Some(node) = resolved_node {
            let outlet_count = node.num_outlets;
            // If default value is 1, treat as unknown and skip
            let is_known = outlet_count != 1
                || node.object_name == "unpack"
                || node.object_name == "inlet"
                || node.object_name == "inlet~";
            if is_known && outlet_count != num_names {
                return Err(BuildError::DestructuringCountMismatch {
                    expected: outlet_count as usize,
                    got: num_names as usize,
                });
            }
        }

        // Check if the resolved expression is already unpack (or has enough outlets)
        let source_has_enough_outlets = resolved_node
            .map(|n| n.num_outlets >= num_names)
            .unwrap_or(false);

        let target_id = if source_has_enough_outlets {
            // If the resolved node has enough outlets, map directly
            source_id.clone()
        } else {
            // Auto-insert unpack node
            let id = self.gen_id();
            // If source is from a tuple, use its type arguments
            let type_args = self.lookup_tuple_type_args(&dw.value, num_names);

            let node = PatchNode {
                id: id.clone(),
                object_name: "unpack".to_string(),
                args: type_args,
                num_inlets: 1,
                num_outlets: num_names,
                is_signal: false,
                varname: None,
                hot_inlets: default_hot_inlets("unpack", 1),
                purity: classify_purity("unpack"),
                attrs: vec![],
                code: None,
            };
            self.graph.add_node(node);

            self.graph.add_edge(PatchEdge {
                source_id,
                source_outlet: _source_outlet,
                dest_id: id.clone(),
                dest_inlet: 0,
                is_feedback: false,
                order: None,
            });

            id
        };

        // Map each name to its corresponding outlet
        for (i, name) in dw.names.iter().enumerate() {
            self.name_map
                .insert(name.clone(), (target_id.clone(), i as u32));
            self.destructured_names.insert(name.clone());
        }

        Ok(())
    }

    /// Look up tuple type arguments from the destructuring source.
    ///
    /// If source is `Expr::Ref(name)` and `name` is from a tuple,
    /// return the recorded type arguments.
    /// If source is `Expr::Call { object: "unpack", args: [Expr::Ref(name)] }` and
    /// `name` is from a tuple, return similarly.
    /// Falls back to all elements "f" if not found.
    fn lookup_tuple_type_args(&self, value: &Expr, num_names: u32) -> Vec<String> {
        let source_name = match value {
            Expr::Ref(name) => Some(name.as_str()),
            Expr::Call { object, args } if object == "unpack" => args.first().and_then(|arg| {
                if let Expr::Ref(name) = &arg.value {
                    Some(name.as_str())
                } else {
                    None
                }
            }),
            _ => None,
        };
        if let Some(name) = source_name {
            if let Some(type_args) = self.tuple_type_args.get(name) {
                return type_args.clone();
            }
        }
        (0..num_names).map(|_| "f".to_string()).collect()
    }

    /// Process a FeedbackDecl.
    ///
    /// Forward-declare the feedback name and register it in name_map.
    /// The actual tapin~ node is generated during feedback_assignment.
    /// Here we tentatively register the feedback name in name_map (so tapout~ can reference it later).
    fn add_feedback_decl(&mut self, decl: &FeedbackDecl) {
        // Register feedback name in feedback_map (tapin~ node ID is determined at assignment)
        // Insert dummy entry in name_map to make it referenceable
        // When fb is referenced in tapout~(fb, delay), a connection to the tapin~ node is needed
        // Generate the tapin~ node first and register it in name_map
        let tapin_id = self.gen_id();
        let node = PatchNode {
            id: tapin_id.clone(),
            object_name: "tapin~".to_string(),
            args: vec![],
            num_inlets: 1,
            num_outlets: 1,
            is_signal: true,
            varname: None,
            hot_inlets: default_hot_inlets("tapin~", 1),
            purity: classify_purity("tapin~"),
            attrs: vec![],
            code: None,
        };
        self.graph.add_node(node);
        self.feedback_map
            .insert(decl.name.clone(), tapin_id.clone());
        // Map tapin~ outlet 0 to the feedback name
        // When fb is referenced in tapout~(fb, 500), an edge from tapin~ outlet 0 -> tapout~ inlet 0 is created
        self.name_map.insert(decl.name.clone(), (tapin_id, 0));
    }

    /// Process a FeedbackAssignment.
    ///
    /// Evaluate the assignment value as in `feedback fb = tapin~(mixed, 1000);`
    /// and connect it to tapin~ node inlet 0.
    fn add_feedback_assignment(&mut self, assign: &FeedbackAssignment) -> Result<(), BuildError> {
        // E013: Duplicate assignment check
        if !self.assigned_feedbacks.insert(assign.target.clone()) {
            return Err(BuildError::DuplicateFeedbackAssignment(
                assign.target.clone(),
            ));
        }

        let (source_id, source_outlet) = self.resolve_expr(&assign.value)?;

        // Get tapin~ node ID
        if let Some(tapin_id) = self.feedback_map.get(&assign.target).cloned() {
            // Connection source -> tapin~ (feedback edge)
            self.graph.add_edge(PatchEdge {
                source_id,
                source_outlet,
                dest_id: tapin_id,
                dest_inlet: 0,
                is_feedback: true,
                order: None,
            });
        }

        Ok(())
    }

    /// Process a StateDecl.
    ///
    /// `state counter: int = 0;` -> Max `[int 0]` node
    /// `state volume: float = 0.5;` -> Max `[float 0.5]` node
    fn add_state_decl(&mut self, decl: &StateDecl) -> Result<(), BuildError> {
        let id = self.gen_id();

        let (object_name, init_arg) = match decl.port_type {
            PortType::Int => (
                "int".to_string(),
                match &decl.init_value {
                    Expr::Lit(LitValue::Int(v)) => v.to_string(),
                    Expr::Lit(LitValue::Float(v)) => format!("{}", *v as i64),
                    _ => "0".to_string(),
                },
            ),
            PortType::Float => (
                "float".to_string(),
                match &decl.init_value {
                    Expr::Lit(LitValue::Float(v)) => format_lit(&LitValue::Float(*v)),
                    Expr::Lit(LitValue::Int(v)) => format!("{}.", v),
                    _ => "0.".to_string(),
                },
            ),
            // Use int as fallback for Bang, List, Symbol
            _ => ("int".to_string(), "0".to_string()),
        };

        let node = PatchNode {
            id: id.clone(),
            object_name: object_name.clone(),
            args: vec![init_arg],
            num_inlets: 2, // inlet 0 = hot (bang/output), inlet 1 = cold (set value)
            num_outlets: 1,
            is_signal: false,
            varname: Some(decl.name.clone()),
            hot_inlets: vec![true, false], // inlet 0 hot, inlet 1 cold
            purity: classify_purity(&object_name),
            attrs: vec![],
            code: None,
        };
        self.graph.add_node(node);

        // Register state name in name_map
        self.name_map.insert(decl.name.clone(), (id, 0));

        Ok(())
    }

    /// Process a StateAssignment.
    ///
    /// `state counter = next;` -> connect `next` output to state node inlet 1 (cold)
    fn add_state_assignment(&mut self, assign: &StateAssignment) -> Result<(), BuildError> {
        // E019: Duplicate assignment check
        if !self.assigned_states.insert(assign.name.clone()) {
            return Err(BuildError::DuplicateStateAssignment(assign.name.clone()));
        }

        // Get node from state name
        let (state_node_id, _) = self
            .name_map
            .get(&assign.name)
            .ok_or_else(|| BuildError::UndefinedRef(assign.name.clone()))?
            .clone();

        // Resolve the expression
        let (source_id, source_outlet) = self.resolve_expr(&assign.value)?;

        // Connect source -> state node inlet 1 (cold inlet)
        self.graph.add_edge(PatchEdge {
            source_id,
            source_outlet,
            dest_id: state_node_id,
            dest_inlet: 1, // cold inlet for value update
            is_feedback: false,
            order: None,
        });

        Ok(())
    }

    /// Process a DirectConnection.
    ///
    /// Process direct connections like `node.in[N] = expr;`,
    /// connecting the expression output to the specified inlet of the target node.
    fn add_direct_connection(&mut self, conn: &DirectConnection) -> Result<(), BuildError> {
        let target_name = &conn.target.object;
        let index = conn.target.index;

        // Look up node from wire name
        let (node_id, _) = self
            .name_map
            .get(target_name)
            .ok_or_else(|| BuildError::UndefinedRef(target_name.clone()))?
            .clone();

        // Get node numinlets and validate port index
        // If index is out of range, expand the node numinlets
        // (can occur with back-edge direct_connections generated by the decompiler)
        if let Some(node) = self.graph.find_node_mut(&node_id) {
            if index >= node.num_inlets {
                node.num_inlets = index + 1;
            }
        }

        // Resolve the expression and create the edge
        let (source_id, source_outlet) = self.resolve_expr(&conn.value)?;

        self.graph.add_edge(PatchEdge {
            source_id,
            source_outlet,
            dest_id: node_id,
            dest_inlet: index,
            is_feedback: false,
            order: None,
        });

        Ok(())
    }
}

/// Infer pack type argument for a tuple element from an expression.
///
/// - `Expr::Lit(Int(_))` → `"i"`
/// - `Expr::Lit(Float(_))` → `"f"`
/// - `Expr::Lit(Str(_))` → `"s"`
/// - Others -> `"f"` fallback (cannot determine without type context)
fn infer_pack_type_arg(expr: &Expr) -> String {
    match expr {
        Expr::Lit(LitValue::Int(_)) => "i".to_string(),
        Expr::Lit(LitValue::Float(_)) => "f".to_string(),
        Expr::Lit(LitValue::Str(_)) => "s".to_string(),
        _ => "f".to_string(), // Ref, Call, OutputPortAccess, Tuple -> fallback
    }
}

/// Classify purity from object name.
fn classify_purity(object_name: &str) -> NodePurity {
    match object_name {
        // Signal objects are generally Pure (with exceptions)
        name if name.ends_with('~') => match name {
            "tapin~" | "tapout~" | "line~" | "delay~" | "phasor~" | "count~" | "index~"
            | "buffer~" | "groove~" | "play~" | "record~" | "sfplay~" | "sfrecord~" | "sig~" => {
                NodePurity::Stateful
            }
            _ => NodePurity::Pure,
        },
        // Known stateful Control objects
        "pack" | "unpack" | "int" | "float" | "toggle" | "gate" | "counter" | "message" | "zl"
        | "coll" | "dict" | "regexp" | "value" | "table" | "funbuff" | "bag" | "borax"
        | "bucket" | "histo" | "mousestate" | "spray" | "switch" | "if" | "expr" | "vexpr"
        | "button" | "number" | "flonum" | "slider" | "dial" | "umenu" | "preset" | "pattr"
        | "autopattr" | "pattrstorage" => NodePurity::Stateful,
        // Known pure Control objects
        "+" | "-" | "*" | "/" | "%" | "trigger" | "t" | "route" | "select" | "prepend"
        | "append" | "stripnote" | "makenote" | "scale" | "split" | "swap" | "clip" | "minimum"
        | "maximum" | "inlet" | "inlet~" | "outlet" | "outlet~" | "loadbang" | "print" | "send"
        | "receive" | "forward" | "ezdac~" | "dac~" | "adc~" => NodePurity::Pure,
        _ => NodePurity::Unknown,
    }
}

/// Generate default hot/cold inlets from object name and inlet count.
/// Max default rule: inlet 0 is hot, others are cold.
fn default_hot_inlets(_object_name: &str, num_inlets: u32) -> Vec<bool> {
    if num_inlets == 0 {
        return vec![];
    }
    // trigger has only inlet 0 as hot (no other inlets)
    // Most objects have inlet 0 as hot, the rest as cold
    (0..num_inlets).map(|i| i == 0).collect()
}

/// Assign order to fanout edges.
/// Assign 0, 1, 2... when multiple edges share the same (source_id, source_outlet).
/// Remain None for single edges.
fn assign_edge_orders(graph: &mut PatchGraph) {
    use std::collections::HashMap;

    // Group edge indices by (source_id, source_outlet)
    let mut groups: HashMap<(String, u32), Vec<usize>> = HashMap::new();
    for (i, edge) in graph.edges.iter().enumerate() {
        let key = (edge.source_id.clone(), edge.source_outlet);
        groups.entry(key).or_default().push(i);
    }

    // Only assign order to groups with 2+ edges
    for indices in groups.values() {
        if indices.len() >= 2 {
            for (order, &edge_idx) in indices.iter().enumerate() {
                graph.edges[edge_idx].order = Some(order as u32);
            }
        }
    }
}

/// Convert LitValue to string.
fn format_lit(lit: &LitValue) -> String {
    match lit {
        LitValue::Int(v) => v.to_string(),
        LitValue::Float(v) => {
            // Float always preserves the decimal point. In Max, 1. (float) and 1 (int) have different meanings.
            // e.g., [* 1.] is float multiplication, [* 1] is int multiplication.
            if v.fract() == 0.0 {
                format!("{}.", *v as i64)
            } else {
                format!("{}", v)
            }
        }
        LitValue::Str(s) => s.clone(),
    }
}

/// Convert AttrValue to string.
/// Used for Max `@key value` format and box JSON fields.
fn format_attr_value(val: &flutmax_ast::AttrValue) -> String {
    match val {
        flutmax_ast::AttrValue::Int(v) => v.to_string(),
        flutmax_ast::AttrValue::Float(v) => {
            // Max accepts trailing dot (e.g., "100.").
            // Format integer-like floats with trailing dot.
            if v.fract() == 0.0 {
                format!("{}.", *v as i64)
            } else {
                format!("{}", v)
            }
        }
        flutmax_ast::AttrValue::Str(s) => s.clone(),
        flutmax_ast::AttrValue::Ident(s) => s.clone(),
    }
}

/// Convert flutmax aliases to Max object names.
/// Only converts arithmetic operators. Returns others as-is.
fn resolve_max_object_name(flutmax_name: &str) -> &str {
    match flutmax_name {
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
        // Reversed arithmetic
        "rsub" => "!-",
        "rdvd" => "!/",
        "rmod" => "!%",
        "rsub~" => "!-~",
        "rdvd~" => "!/~",
        "rmod~" => "!%~",
        // Comparison
        "gt" => ">",
        "lt" => "<",
        "gte" => ">=",
        "lte" => "<=",
        "eq" => "==",
        "neq" => "!=",
        "gt~" => ">~",
        "lt~" => "<~",
        "gte~" => ">=~",
        "lte~" => "<=~",
        "eq~" => "==~",
        "neq~" => "!=~",
        // Logical/bitwise
        "and" => "&&",
        "or" => "||",
        "lshift" => "<<",
        "rshift" => ">>",
        other => other,
    }
}

/// Match named argument parameter name against objdb inlet definitions and return inlet index.
///
/// Returns `None` if not registered in objdb or if the name does not match.
/// Name matching is case-insensitive with spaces normalized to underscores.
fn resolve_inlet_name(object_name: &str, arg_name: &str, objdb: Option<&ObjectDb>) -> Option<u32> {
    let db = objdb?;
    let def = db.lookup(object_name)?;
    let inlets = match &def.inlets {
        InletSpec::Fixed(ports) => ports.as_slice(),
        InletSpec::Variable { defaults, .. } => defaults.as_slice(),
    };
    let arg_lower = arg_name.to_lowercase();
    for port in inlets {
        let normalized = normalize_port_description(&port.description);
        if let Some(ref n) = normalized {
            if *n == arg_lower {
                return Some(port.id);
            }
        }
    }
    None
}

/// Normalize an objdb port description to a valid flutmax identifier.
///
/// This must match the normalization in `flutmax-decompile`'s `normalize_inlet_name`
/// to ensure roundtrip consistency (decompile → named args → compile → resolve).
fn normalize_port_description(description: &str) -> Option<String> {
    let trimmed = description.trim();
    // Strip leading type prefix like "(signal)", "(signal/float)", "(float)"
    let stripped = if trimmed.starts_with('(') {
        if let Some(end) = trimmed.find(')') {
            trimmed[end + 1..].trim()
        } else {
            trimmed
        }
    } else {
        trimmed
    };
    let s: String = stripped
        .to_lowercase()
        .chars()
        .map(|c| if c == ' ' { '_' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    let parts: Vec<&str> = s.split('_').filter(|p| !p.is_empty()).collect();
    let result = parts.join("_");
    let result = result
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .to_string();
    if result.is_empty() || result.len() > 20 {
        None
    } else {
        Some(result)
    }
}

/// Resolve a named argument against the AbstractionRegistry.
///
/// Abstractions define their inlets via `in freq: float;` declarations.
/// This function matches the argument name against those declarations.
fn resolve_abstraction_inlet_name(
    object_name: &str,
    arg_name: &str,
    registry: Option<&AbstractionRegistry>,
) -> Option<u32> {
    let reg = registry?;
    let iface = reg.lookup(object_name)?;
    let arg_lower = arg_name.to_lowercase();
    for port in &iface.in_ports {
        if port.name.to_lowercase() == arg_lower {
            return Some(port.index);
        }
    }
    None
}

/// Estimate inlet count from object name and arguments.
/// Prioritizes objdb when provided; falls back to hardcoded table for unregistered objects.
/// Variable-inlet objects are inferred from argument count.
fn infer_num_inlets(object_name: &str, args: &[String], objdb: Option<&ObjectDb>) -> u32 {
    // Prioritize objdb lookup
    if let Some(db) = objdb {
        if let Some(def) = db.lookup(object_name) {
            return match &def.inlets {
                InletSpec::Fixed(ports) => ports.len() as u32,
                InletSpec::Variable {
                    defaults,
                    min_inlets,
                } => {
                    if args.is_empty() {
                        defaults.len().max(*min_inlets as usize) as u32
                    } else {
                        args.len() as u32
                    }
                }
            };
        }
    }
    // Hardcoded fallback
    match object_name {
        // Signal arithmetic
        "cycle~" => 2,
        "*~" | "+~" | "-~" | "/~" | "%~" | "!-~" | "!/~" | "!%~" => 2,
        ">~" | "<~" | ">=~" | "<=~" | "==~" | "!=~" => 2,
        // Control arithmetic
        "*" | "+" | "-" | "/" | "%" | "!-" | "!/" | "!%" => 2,
        ">" | "<" | ">=" | "<=" | "==" | "!=" => 2,
        "&&" | "||" | "<<" | ">>" => 2,
        // Audio I/O
        "ezdac~" => 2,
        "dac~" => 2,
        "adc~" => 0,
        // Triggers / UI
        "loadbang" => 1,
        "button" => 1,
        "print" => 1,
        // Signal processing
        "biquad~" => 6,
        "line~" => 2,
        "tapin~" => 1,
        "tapout~" => 2,
        "noise~" | "phasor~" => 1,
        "snapshot~" | "peakamp~" | "meter~" => 1,
        "edge~" => 1,
        "dspstate~" => 1,
        "fftinfo~" => 1,
        "fftin~" => 1,
        "fftout~" => 1,
        "cartopol~" | "poltocar~" => 2,
        "freqshift~" => 2,
        "curve~" => 2,
        "adsr~" => 5,
        "filtercoeff~" => 4,
        "filtergraph~" => 8,
        // Data
        "int" | "float" => 2,
        "inlet" | "inlet~" => 0,
        "outlet" | "outlet~" => 1,
        // Variable inlets (arg-dependent)
        "trigger" | "t" => 1,
        "select" | "sel" => {
            if args.is_empty() {
                2
            } else {
                1
            }
        }
        "route" => 1,
        "gate" => 2,
        "pack" | "pak" => {
            if args.is_empty() {
                2
            } else {
                args.len() as u32
            }
        }
        "unpack" => 1,
        "buddy" => {
            if args.is_empty() {
                2
            } else {
                args.first()
                    .and_then(|a| a.parse::<u32>().ok())
                    .unwrap_or(2)
            }
        }
        // MIDI
        "makenote" => 3,
        "notein" => 1,
        "noteout" => 3,
        "ctlin" => 1,
        "ctlout" => 3,
        "midiin" => 1,
        "midiout" => 1,
        "borax" => 1,
        // RNBO / gen~ I/O ports
        "param" => 2,
        "in~" => 1,
        "out~" => 1,
        "inport" => 1,
        "outport" => 1,
        // Timing / control
        "line" => 2,
        "function" => 2,
        "counter" => 3,
        "metro" => 2,
        "delay" => 2,
        "pipe" => {
            if args.is_empty() {
                2
            } else {
                args.len() as u32 + 1
            }
        }
        "speedlim" => 2,
        "thresh" => 2,
        // Data structures
        "coll" => 1,
        "urn" => 2,
        "drunk" => 2,
        "random" => 2,
        // List / string
        "match" => 1,
        "zl" => 2,
        "regexp" => 1,
        "sprintf" => {
            if args.is_empty() {
                1
            } else {
                args.len() as u32
            }
        }
        "fromsymbol" => 1,
        "tosymbol" => 1,
        "iter" => 1,
        // Codebox
        "v8.codebox" => 1,
        "codebox" => 1,
        // gen~ ternary conditional operator
        "?" => 3,
        _ => 1,
    }
}

/// Estimate outlet count from object name and arguments.
/// Prioritizes objdb when provided; falls back to hardcoded table for unregistered objects.
/// Variable-outlet objects are dynamically inferred from argument count.
fn infer_num_outlets(object_name: &str, args: &[String], objdb: Option<&ObjectDb>) -> u32 {
    // Prioritize objdb lookup
    if let Some(db) = objdb {
        if let Some(def) = db.lookup(object_name) {
            return match &def.outlets {
                OutletSpec::Fixed(ports) => ports.len() as u32,
                OutletSpec::Variable {
                    defaults,
                    min_outlets,
                } => {
                    if args.is_empty() {
                        defaults.len().max(*min_outlets as usize) as u32
                    } else {
                        args.len() as u32
                    }
                }
            };
        }
    }
    // Hardcoded fallback
    match object_name {
        // Signal processing
        "cycle~" => 1,
        "*~" | "+~" | "-~" | "/~" => 1,
        "biquad~" => 1,
        "line~" => 2,
        "tapin~" => 1,
        "tapout~" => 1,
        "noise~" | "phasor~" => 1,
        "snapshot~" | "peakamp~" | "meter~" => 1,
        "edge~" => 2,
        "dspstate~" => 4,
        "fftinfo~" => 4,
        "fftin~" => 3,
        "fftout~" => 1,
        "cartopol~" | "poltocar~" => 2,
        "freqshift~" => 2,
        "curve~" => 2,
        "adsr~" => 4,
        "filtercoeff~" => 5,
        "filtergraph~" => 7,
        // Control arithmetic
        "*" | "+" | "-" | "/" | "%" => 1,
        // Audio I/O
        "ezdac~" | "dac~" => 0,
        "adc~" => 1,
        // Triggers / UI
        "loadbang" => 1,
        "button" => 1,
        "print" => 0,
        // Data
        "int" | "float" => 1,
        "inlet" | "inlet~" => 1,
        "outlet" | "outlet~" => 0,
        // Variable outlets (arg-dependent)
        "select" | "sel" => {
            if args.is_empty() {
                2
            } else {
                args.len() as u32 + 1
            }
        }
        "route" => {
            if args.is_empty() {
                2
            } else {
                args.len() as u32 + 1
            }
        }
        "gate" => args
            .first()
            .and_then(|a| a.parse::<u32>().ok())
            .unwrap_or(2),
        "trigger" | "t" => {
            if args.is_empty() {
                1
            } else {
                args.len() as u32
            }
        }
        "unpack" => {
            if args.is_empty() {
                2
            } else {
                args.len() as u32
            }
        }
        "pack" | "pak" => 1,
        "buddy" => {
            if args.is_empty() {
                2
            } else {
                args.first()
                    .and_then(|a| a.parse::<u32>().ok())
                    .unwrap_or(2)
            }
        }
        // Timing / control
        "function" => 2,
        "line" => 2,
        "counter" => 4,
        "metro" => 1,
        "delay" => 1,
        "pipe" => {
            if args.is_empty() {
                1
            } else {
                args.len() as u32
            }
        }
        "speedlim" => 1,
        "thresh" => 2,
        // MIDI
        "makenote" => 2,
        "borax" => 8,
        "notein" => 3,
        "noteout" => 0,
        "ctlin" => 3,
        "ctlout" => 0,
        "midiin" => 1,
        "midiout" => 0,
        // RNBO / gen~ I/O ports
        "param" => 2,
        "in~" => 1,
        "out~" => 0,
        "inport" => 1,
        "outport" => 0,
        // Data structures
        "coll" => 4,
        "urn" => 2,
        "drunk" => 1,
        "random" => 1,
        // List / string / pattern
        "match" => 2,
        "zl" => 2,
        "regexp" => 5,
        "sprintf" => 1,
        "fromsymbol" => 1,
        "tosymbol" => 1,
        "iter" => 1,
        // UI objects
        "textbutton" => 3,
        "live.text" => 2,
        "live.dial" => 2,
        "live.toggle" => 1,
        "live.menu" => 3,
        "live.numbox" => 2,
        "live.tab" => 3,
        "live.comment" => 0,
        "umenu" => 3,
        "flonum" => 2,
        "number" => 2,
        "slider" | "dial" | "rslider" => 1,
        "multislider" | "kslider" => 2,
        "tab" => 3,
        "toggle" => 1,
        // Codebox
        "v8.codebox" => 1,
        "codebox" => 1,
        _ => 1,
    }
}

/// Infer inlet/outlet count from gen~ codebox code.
///
/// GenExpr code references inputs with `in1`, `in2`, ..., `inN`,
/// and defines outputs with `out1`, `out2`, ..., `outN`.
/// Detects the maximum N and returns inlet/outlet counts.
fn infer_codebox_ports(code: &str) -> (u32, u32) {
    let mut max_in: u32 = 0;
    let mut max_out: u32 = 0;

    // Scan for in1..inN and out1..outN patterns
    // Use simple byte scanning to avoid regex dependency
    let bytes = code.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        // Check for "in" or "out" at word boundary
        let at_word_start = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        if at_word_start {
            if i + 2 < len && bytes[i] == b'o' && bytes[i + 1] == b'u' && bytes[i + 2] == b't' {
                // Parse "outN"
                let mut j = i + 3;
                let mut num: u32 = 0;
                let mut has_digit = false;
                while j < len && bytes[j].is_ascii_digit() {
                    num = num * 10 + (bytes[j] - b'0') as u32;
                    has_digit = true;
                    j += 1;
                }
                // Must have digits and NOT be followed by alphanumeric (word boundary)
                if has_digit && (j >= len || !bytes[j].is_ascii_alphanumeric()) && num > max_out {
                    max_out = num;
                }
                i = j;
                continue;
            } else if i + 1 < len && bytes[i] == b'i' && bytes[i + 1] == b'n' {
                // Parse "inN" — but not "int", "into", etc.
                let mut j = i + 2;
                let mut num: u32 = 0;
                let mut has_digit = false;
                while j < len && bytes[j].is_ascii_digit() {
                    num = num * 10 + (bytes[j] - b'0') as u32;
                    has_digit = true;
                    j += 1;
                }
                if has_digit && (j >= len || !bytes[j].is_ascii_alphanumeric()) && num > max_in {
                    max_in = num;
                }
                if has_digit {
                    i = j;
                    continue;
                }
            }
        }
        i += 1;
    }

    // gen~ uses 1-based indexing: in1..inN means N inlets
    (max_in.max(1), max_out.max(1))
}

/// Convert Program (AST) to PatchGraph.
///
/// After conversion, calls `insert_triggers()` to auto-insert triggers for fanouts.
pub fn build_graph(program: &Program) -> Result<PatchGraph, BuildError> {
    build_graph_with_registry(program, None)
}

/// Convert Program (AST) to PatchGraph (with Abstraction registry).
///
/// When `registry` is `Some` and the `Expr::Call` object name is
/// registered in the registry, `numinlets`/`numoutlets` are determined from its interface.
pub fn build_graph_with_registry(
    program: &Program,
    registry: Option<&AbstractionRegistry>,
) -> Result<PatchGraph, BuildError> {
    build_graph_with_code_files(program, registry, None)
}

/// Convert Program (AST) to PatchGraph (with Abstraction registry + code files).
///
/// When `code_files` is `Some`, resolves `v8.codebox` and `codebox` filename arguments
/// to code content and stores it in `PatchNode.code`.
pub fn build_graph_with_code_files(
    program: &Program,
    registry: Option<&AbstractionRegistry>,
    code_files: Option<&CodeFiles>,
) -> Result<PatchGraph, BuildError> {
    build_graph_with_objdb(program, registry, code_files, None)
}

/// Convert Program (AST) to PatchGraph (with all parameters).
///
/// When `objdb` is `Some`, `infer_num_inlets`/`infer_num_outlets`
/// prioritize the object definition database, falling back to hardcoded tables for unregistered objects.
pub fn build_graph_with_objdb(
    program: &Program,
    registry: Option<&AbstractionRegistry>,
    code_files: Option<&CodeFiles>,
    objdb: Option<&ObjectDb>,
) -> Result<PatchGraph, BuildError> {
    let mut builder = GraphBuilder::new(registry, code_files, objdb);

    // 1. InDecl -> inlet nodes
    for decl in &program.in_decls {
        builder.add_inlet(decl);
    }

    // 2. OutDecl -> outlet nodes
    for decl in &program.out_decls {
        builder.add_outlet(decl);
    }

    // 2b. FeedbackDecl -> tapin~ nodes (forward declaration)
    for decl in &program.feedback_decls {
        builder.add_feedback_decl(decl);
    }

    // 2c. StateDecl -> int/float nodes (forward declaration)
    for decl in &program.state_decls {
        builder.add_state_decl(decl)?;
    }

    // 2d. MsgDecl -> message box nodes
    for decl in &program.msg_decls {
        builder.add_msg(decl);
    }

    // 3. Wire -> object nodes + edges
    for wire in &program.wires {
        builder.add_wire(wire)?;
    }

    // 3b. DestructuringWire -> unpack nodes + edges
    for dw in &program.destructuring_wires {
        builder.add_destructuring_wire(dw)?;
    }

    // 3c. FeedbackAssignment -> connections to tapin~
    for assign in &program.feedback_assignments {
        builder.add_feedback_assignment(assign)?;
    }

    // 3d. StateAssignment -> connections to state node cold inlets
    for assign in &program.state_assignments {
        builder.add_state_assignment(assign)?;
    }

    // 4. OutAssignment -> edges
    for assign in &program.out_assignments {
        builder.add_out_assignment(assign)?;
    }

    // 4a. OutDecl with inline value → implicit OutAssignment
    for decl in &program.out_decls {
        if let Some(ref value) = decl.value {
            let implicit_assign = OutAssignment {
                index: decl.index,
                value: value.clone(),
                span: None,
            };
            builder.add_out_assignment(&implicit_assign)?;
        }
    }

    // 4b. DirectConnection -> edges
    for conn in &program.direct_connections {
        builder.add_direct_connection(conn)?;
    }

    // 5. Auto-insert triggers
    insert_triggers(&mut builder.graph);

    // 6. Assign order to fanout edges
    assign_edge_orders(&mut builder.graph);

    Ok(builder.graph)
}

/// Convert Program (AST) to PatchGraph without auto-inserting triggers.
///
/// gen~ executes synchronously per-sample, so triggers are unnecessary.
/// The trigger object does not exist in the gen~ domain — inserting one
/// would produce an invalid patch.
pub fn build_graph_without_triggers(program: &Program) -> Result<PatchGraph, BuildError> {
    let mut builder = GraphBuilder::new(None, None, None);

    for decl in &program.in_decls {
        builder.add_inlet(decl);
    }
    for decl in &program.out_decls {
        builder.add_outlet(decl);
    }
    for decl in &program.feedback_decls {
        builder.add_feedback_decl(decl);
    }
    for decl in &program.state_decls {
        builder.add_state_decl(decl)?;
    }
    for decl in &program.msg_decls {
        builder.add_msg(decl);
    }
    for wire in &program.wires {
        builder.add_wire(wire)?;
    }
    for dw in &program.destructuring_wires {
        builder.add_destructuring_wire(dw)?;
    }
    for assign in &program.feedback_assignments {
        builder.add_feedback_assignment(assign)?;
    }
    for assign in &program.state_assignments {
        builder.add_state_assignment(assign)?;
    }
    for assign in &program.out_assignments {
        builder.add_out_assignment(assign)?;
    }
    for decl in &program.out_decls {
        if let Some(ref value) = decl.value {
            let implicit_assign = OutAssignment {
                index: decl.index,
                value: value.clone(),
                span: None,
            };
            builder.add_out_assignment(&implicit_assign)?;
        }
    }
    for conn in &program.direct_connections {
        builder.add_direct_connection(conn)?;
    }

    // Skip trigger insertion (gen~ does not need it).
    assign_edge_orders(&mut builder.graph);

    Ok(builder.graph)
}

/// Convert Program (AST) to PatchGraph + warnings.
pub fn build_graph_with_warnings(program: &Program) -> Result<BuildResult, BuildError> {
    build_graph_with_registry_and_warnings(program, None)
}

/// Convert Program (AST) to PatchGraph + warnings (with Abstraction registry).
pub fn build_graph_with_registry_and_warnings(
    program: &Program,
    registry: Option<&AbstractionRegistry>,
) -> Result<BuildResult, BuildError> {
    let graph = build_graph_with_registry(program, registry)?;
    let warnings = detect_duplicate_inlets(&graph);
    Ok(BuildResult { graph, warnings })
}

/// Detect duplicate connections to the same inlet.
fn detect_duplicate_inlets(graph: &PatchGraph) -> Vec<BuildWarning> {
    let mut inlet_counts: HashMap<(String, u32), usize> = HashMap::new();
    for edge in &graph.edges {
        if !edge.is_feedback {
            *inlet_counts
                .entry((edge.dest_id.clone(), edge.dest_inlet))
                .or_insert(0) += 1;
        }
    }
    let mut warnings: Vec<BuildWarning> = inlet_counts
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(
            |((node_id, inlet), count)| BuildWarning::DuplicateInletConnection {
                node_id,
                inlet,
                count,
            },
        )
        .collect();
    // Sort to make output order deterministic
    warnings.sort_by(|a, b| {
        let (a_id, a_inlet) = match a {
            BuildWarning::DuplicateInletConnection { node_id, inlet, .. } => (node_id, inlet),
        };
        let (b_id, b_inlet) = match b {
            BuildWarning::DuplicateInletConnection { node_id, inlet, .. } => (node_id, inlet),
        };
        a_id.cmp(b_id).then(a_inlet.cmp(b_inlet))
    });
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use flutmax_ast::*;

    /// L1: `cycle~ 440` -> `ezdac~` (minimal patch)
    fn make_l1_program() -> Program {
        Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![Wire {
                name: "osc".to_string(),
                value: Expr::Call {
                    object: "cycle~".to_string(),
                    args: vec![CallArg::positional(Expr::Lit(LitValue::Int(440)))],
                },
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
            state_assignments: vec![],
        }
    }

    /// L2: `in freq: float → cycle~(freq) → *~(osc, 0.5) → out audio: signal`
    fn make_l2_program() -> Program {
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
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![OutAssignment {
                index: 0,
                value: Expr::Ref("amp".to_string()),
                span: None,
            }],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        }
    }

    #[test]
    fn test_build_l1_nodes() {
        let prog = make_l1_program();
        let graph = build_graph(&prog).unwrap();

        // One cycle~ node
        assert_eq!(graph.nodes.len(), 1);
        let node = &graph.nodes[0];
        assert_eq!(node.object_name, "cycle~");
        assert_eq!(node.args, vec!["440"]);
        assert!(node.is_signal);
        assert_eq!(node.num_inlets, 2);
        assert_eq!(node.num_outlets, 1);
    }

    #[test]
    fn test_build_l1_no_edges() {
        let prog = make_l1_program();
        let graph = build_graph(&prog).unwrap();

        // No edges since cycle~ is standalone
        assert_eq!(graph.edges.len(), 0);
    }

    #[test]
    fn test_build_l2_nodes() {
        let prog = make_l2_program();
        let graph = build_graph(&prog).unwrap();

        // 4 nodes: inlet, outlet~, cycle~, *~
        assert_eq!(graph.nodes.len(), 4);

        let names: Vec<&str> = graph.nodes.iter().map(|n| n.object_name.as_str()).collect();
        assert!(names.contains(&"inlet"));
        assert!(names.contains(&"outlet~"));
        assert!(names.contains(&"cycle~"));
        assert!(names.contains(&"*~"));
    }

    #[test]
    fn test_build_l2_edges() {
        let prog = make_l2_program();
        let graph = build_graph(&prog).unwrap();

        // Edges: inlet->cycle~, cycle~->*~, *~->outlet~
        assert_eq!(graph.edges.len(), 3);

        // inlet → cycle~ (inlet 0)
        let inlet_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "inlet")
            .unwrap();
        let cycle_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "cycle~")
            .unwrap();
        let inlet_to_cycle = graph
            .edges
            .iter()
            .find(|e| e.source_id == inlet_node.id && e.dest_id == cycle_node.id)
            .expect("edge from inlet to cycle~ should exist");
        assert_eq!(inlet_to_cycle.source_outlet, 0);
        assert_eq!(inlet_to_cycle.dest_inlet, 0);

        // cycle~ → *~ (inlet 0)
        let mul_node = graph.nodes.iter().find(|n| n.object_name == "*~").unwrap();
        let cycle_to_mul = graph
            .edges
            .iter()
            .find(|e| e.source_id == cycle_node.id && e.dest_id == mul_node.id)
            .expect("edge from cycle~ to *~ should exist");
        assert_eq!(cycle_to_mul.dest_inlet, 0);

        // *~ → outlet~
        let outlet_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "outlet~")
            .unwrap();
        let mul_to_outlet = graph
            .edges
            .iter()
            .find(|e| e.source_id == mul_node.id && e.dest_id == outlet_node.id)
            .expect("edge from *~ to outlet~ should exist");
        assert_eq!(mul_to_outlet.dest_inlet, 0);
    }

    #[test]
    fn test_build_l2_mul_args() {
        let prog = make_l2_program();
        let graph = build_graph(&prog).unwrap();

        let mul_node = graph.nodes.iter().find(|n| n.object_name == "*~").unwrap();
        // *~(osc, 0.5) -> args contains "0.5"
        assert_eq!(mul_node.args, vec!["0.5"]);
    }

    #[test]
    fn test_undefined_ref_error() {
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![Wire {
                name: "x".to_string(),
                value: Expr::Call {
                    object: "cycle~".to_string(),
                    args: vec![CallArg::positional(Expr::Ref("nonexistent".to_string()))],
                },
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
            state_assignments: vec![],
        };

        let result = build_graph(&prog);
        assert!(result.is_err());
        match result.unwrap_err() {
            BuildError::UndefinedRef(name) => assert_eq!(name, "nonexistent"),
            _ => panic!("expected UndefinedRef error"),
        }
    }

    #[test]
    fn test_outlet_index_out_of_range() {
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![OutDecl {
                index: 0,
                name: "out".to_string(),
                port_type: PortType::Float,
                value: None,
            }],
            wires: vec![Wire {
                name: "x".to_string(),
                value: Expr::Call {
                    object: "button".to_string(),
                    args: vec![],
                },
                span: None,
                attrs: vec![],
            }],
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![OutAssignment {
                index: 5, // out_decls only has index 0
                value: Expr::Ref("x".to_string()),
                span: None,
            }],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let result = build_graph(&prog);
        assert!(result.is_err());
        match result.unwrap_err() {
            BuildError::NoOutDeclaration(idx) => assert_eq!(idx, 5),
            _ => panic!("expected NoOutDeclaration error"),
        }
    }

    #[test]
    fn test_format_lit_int() {
        assert_eq!(format_lit(&LitValue::Int(440)), "440");
        assert_eq!(format_lit(&LitValue::Int(-1)), "-1");
        assert_eq!(format_lit(&LitValue::Int(0)), "0");
    }

    #[test]
    fn test_format_lit_float() {
        assert_eq!(format_lit(&LitValue::Float(0.5)), "0.5");
        assert_eq!(format_lit(&LitValue::Float(440.0)), "440.");
        assert_eq!(format_lit(&LitValue::Float(3.14)), "3.14");
    }

    #[test]
    fn test_format_lit_str() {
        assert_eq!(format_lit(&LitValue::Str("hello".to_string())), "hello");
    }

    #[test]
    fn test_signal_inlet_is_signal() {
        let prog = Program {
            in_decls: vec![InDecl {
                index: 0,
                name: "sig_in".to_string(),
                port_type: PortType::Signal,
            }],
            out_decls: vec![],
            wires: vec![],
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();
        let inlet_node = &graph.nodes[0];
        assert_eq!(inlet_node.object_name, "inlet~");
        assert!(inlet_node.is_signal);
        assert_eq!(inlet_node.num_inlets, 1);
        assert_eq!(inlet_node.num_outlets, 1);
    }

    #[test]
    fn test_control_inlet_not_signal() {
        let prog = Program {
            in_decls: vec![InDecl {
                index: 0,
                name: "ctrl_in".to_string(),
                port_type: PortType::Float,
            }],
            out_decls: vec![],
            wires: vec![],
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();
        let inlet_node = &graph.nodes[0];
        assert_eq!(inlet_node.object_name, "inlet");
        assert!(!inlet_node.is_signal);
        assert_eq!(inlet_node.num_inlets, 0);
        assert_eq!(inlet_node.num_outlets, 1);
    }

    #[test]
    fn test_signal_outlet() {
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
            msg_decls: vec![],
            out_assignments: vec![],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();
        let outlet_node = &graph.nodes[0];
        assert_eq!(outlet_node.object_name, "outlet~");
        assert!(outlet_node.is_signal);
    }

    #[test]
    fn test_control_outlet() {
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![OutDecl {
                index: 0,
                name: "ctrl_out".to_string(),
                port_type: PortType::Float,
                value: None,
            }],
            wires: vec![],
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();
        let outlet_node = &graph.nodes[0];
        assert_eq!(outlet_node.object_name, "outlet");
        assert!(!outlet_node.is_signal);
    }

    #[test]
    fn test_nested_call() {
        // wire x = *~(cycle~(440), 0.5);
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![Wire {
                name: "x".to_string(),
                value: Expr::Call {
                    object: "*~".to_string(),
                    args: vec![
                        CallArg::positional(Expr::Call {
                            object: "cycle~".to_string(),
                            args: vec![CallArg::positional(Expr::Lit(LitValue::Int(440)))],
                        }),
                        CallArg::positional(Expr::Lit(LitValue::Float(0.5))),
                    ],
                },
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
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();
        // 2 nodes: cycle~ and *~
        assert_eq!(graph.nodes.len(), 2);

        let cycle_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "cycle~")
            .unwrap();
        let mul_node = graph.nodes.iter().find(|n| n.object_name == "*~").unwrap();

        // Edge: cycle~ -> *~
        let edge = graph
            .edges
            .iter()
            .find(|e| e.source_id == cycle_node.id && e.dest_id == mul_node.id)
            .expect("edge from cycle~ to *~ should exist");
        assert_eq!(edge.dest_inlet, 0);
    }

    #[test]
    fn test_multiple_outlets() {
        // Patch with 2 output ports
        let prog = Program {
            in_decls: vec![],
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
                    args: vec![CallArg::positional(Expr::Lit(LitValue::Int(440)))],
                },
                span: None,
                attrs: vec![],
            }],
            destructuring_wires: vec![],
            msg_decls: vec![],
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
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();

        // 2 outlet~ nodes, 1 cycle~ node
        let outlet_nodes: Vec<&PatchNode> = graph
            .nodes
            .iter()
            .filter(|n| n.object_name == "outlet~")
            .collect();
        assert_eq!(outlet_nodes.len(), 2);

        // 2 edges from cycle~ -> outlet~ (Signal, so no trigger needed)
        let cycle_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "cycle~")
            .unwrap();
        let edges_from_cycle: Vec<&PatchEdge> = graph
            .edges
            .iter()
            .filter(|e| e.source_id == cycle_node.id)
            .collect();
        assert_eq!(edges_from_cycle.len(), 2);
    }

    // ─── Abstraction registry tests ───

    /// Build AST for oscillator
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

    /// fm_synth AST: oscillator(base_freq) -> *~(carrier, 0.5) -> out[0]
    fn make_fm_synth_program() -> Program {
        Program {
            in_decls: vec![InDecl {
                index: 0,
                name: "base_freq".to_string(),
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
                    name: "carrier".to_string(),
                    value: Expr::Call {
                        object: "oscillator".to_string(),
                        args: vec![CallArg::positional(Expr::Ref("base_freq".to_string()))],
                    },
                    span: None,
                    attrs: vec![],
                },
                Wire {
                    name: "amp".to_string(),
                    value: Expr::Call {
                        object: "mul~".to_string(),
                        args: vec![
                            CallArg::positional(Expr::Ref("carrier".to_string())),
                            CallArg::positional(Expr::Lit(LitValue::Float(0.5))),
                        ],
                    },
                    span: None,
                    attrs: vec![],
                },
            ],
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![OutAssignment {
                index: 0,
                value: Expr::Ref("amp".to_string()),
                span: None,
            }],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        }
    }

    #[test]
    fn test_build_graph_with_registry_abstraction_inlets_outlets() {
        let mut registry = AbstractionRegistry::new();
        registry.register("oscillator", &make_oscillator_program());

        let prog = make_fm_synth_program();
        let graph = build_graph_with_registry(&prog, Some(&registry)).unwrap();

        // Find the oscillator node
        let osc_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "oscillator")
            .expect("oscillator node should exist");

        // oscillator has in_ports=1 (freq), out_ports=1 (audio)
        assert_eq!(osc_node.num_inlets, 1);
        assert_eq!(osc_node.num_outlets, 1);
        // First out_port is Signal, so is_signal = true
        assert!(osc_node.is_signal);
    }

    #[test]
    fn test_build_graph_with_registry_abstraction_name_preserved() {
        let mut registry = AbstractionRegistry::new();
        registry.register("oscillator", &make_oscillator_program());

        let prog = make_fm_synth_program();
        let graph = build_graph_with_registry(&prog, Some(&registry)).unwrap();

        // object_name remains "oscillator" without alias conversion
        let osc_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "oscillator")
            .expect("oscillator node should exist with original name");
        assert_eq!(osc_node.object_name, "oscillator");
    }

    #[test]
    fn test_build_graph_with_registry_full_graph() {
        let mut registry = AbstractionRegistry::new();
        registry.register("oscillator", &make_oscillator_program());

        let prog = make_fm_synth_program();
        let graph = build_graph_with_registry(&prog, Some(&registry)).unwrap();

        // Nodes: inlet, outlet~, oscillator, *~
        assert_eq!(graph.nodes.len(), 4);

        let names: Vec<&str> = graph.nodes.iter().map(|n| n.object_name.as_str()).collect();
        assert!(names.contains(&"inlet"));
        assert!(names.contains(&"outlet~"));
        assert!(names.contains(&"oscillator"));
        assert!(names.contains(&"*~"));

        // Edges: inlet->oscillator, oscillator->*~, *~->outlet~
        assert_eq!(graph.edges.len(), 3);
    }

    #[test]
    fn test_build_graph_without_registry_unknown_object() {
        // Calling oscillator without registry,
        // falls back to infer_num_inlets/outlets (no error)
        let prog = make_fm_synth_program();
        let graph = build_graph(&prog).unwrap();

        let osc_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "oscillator")
            .expect("oscillator node should exist");

        // Without registry: infer_num_inlets = 1, infer_num_outlets = 1
        // But with 1 argument, num_inlets = max(1, 1) = 1
        assert_eq!(osc_node.num_inlets, 1);
        assert_eq!(osc_node.num_outlets, 1);
    }

    #[test]
    fn test_build_graph_with_registry_multi_port_abstraction() {
        // filter abstraction with 3 inlets, 2 outlets
        let filter_prog = Program {
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
        };

        let mut registry = AbstractionRegistry::new();
        registry.register("filter", &filter_prog);

        // Program that calls filter(osc, 1000, 0.7)
        let caller = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![Wire {
                name: "result".to_string(),
                value: Expr::Call {
                    object: "filter".to_string(),
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
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let graph = build_graph_with_registry(&caller, Some(&registry)).unwrap();

        let filter_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "filter")
            .expect("filter node should exist");

        assert_eq!(filter_node.num_inlets, 3);
        assert_eq!(filter_node.num_outlets, 2);
        assert!(filter_node.is_signal);
    }

    #[test]
    fn test_build_graph_with_none_registry() {
        // registry=None behaves the same as build_graph
        let prog = make_l2_program();
        let graph = build_graph_with_registry(&prog, None).unwrap();

        assert_eq!(graph.nodes.len(), 4);
    }

    // ─── Tuple / Destructuring tests ───

    #[test]
    fn test_tuple_generates_pack_node() {
        // wire t = (x, y, z); -> pack f f f node
        let prog = Program {
            in_decls: vec![
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
                InDecl {
                    index: 2,
                    name: "z".to_string(),
                    port_type: PortType::Float,
                },
            ],
            out_decls: vec![OutDecl {
                index: 0,
                name: "coords".to_string(),
                port_type: PortType::List,
                value: None,
            }],
            wires: vec![Wire {
                name: "packed".to_string(),
                value: Expr::Tuple(vec![
                    Expr::Ref("x".to_string()),
                    Expr::Ref("y".to_string()),
                    Expr::Ref("z".to_string()),
                ]),
                span: None,
                attrs: vec![],
            }],
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![OutAssignment {
                index: 0,
                value: Expr::Ref("packed".to_string()),
                span: None,
            }],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();

        // pack node exists
        let pack_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "pack")
            .expect("pack node should exist");
        assert_eq!(pack_node.num_inlets, 3);
        assert_eq!(pack_node.num_outlets, 1);
        assert_eq!(pack_node.args, vec!["f", "f", "f"]);
        assert!(!pack_node.is_signal);

        // 3 edges from inlet -> pack
        let edges_to_pack: Vec<_> = graph
            .edges
            .iter()
            .filter(|e| e.dest_id == pack_node.id)
            .collect();
        assert_eq!(edges_to_pack.len(), 3);

        // Each inlet connects to a different dest_inlet
        let mut dest_inlets: Vec<u32> = edges_to_pack.iter().map(|e| e.dest_inlet).collect();
        dest_inlets.sort();
        assert_eq!(dest_inlets, vec![0, 1, 2]);
    }

    #[test]
    fn test_destructuring_with_unpack_call() {
        // wire (a, b) = unpack(data); -> Expr::Call generates an unpack node,
        // DestructuringWire maps a, b to its outlets
        use flutmax_ast::DestructuringWire;

        let prog = Program {
            in_decls: vec![InDecl {
                index: 0,
                name: "data".to_string(),
                port_type: PortType::Float,
            }],
            out_decls: vec![
                OutDecl {
                    index: 0,
                    name: "x".to_string(),
                    port_type: PortType::Float,
                    value: None,
                },
                OutDecl {
                    index: 1,
                    name: "y".to_string(),
                    port_type: PortType::Float,
                    value: None,
                },
            ],
            wires: vec![],
            destructuring_wires: vec![DestructuringWire {
                names: vec!["a".to_string(), "b".to_string()],
                value: Expr::Call {
                    object: "unpack".to_string(),
                    args: vec![CallArg::positional(Expr::Ref("data".to_string()))],
                },
                span: None,
            }],
            msg_decls: vec![],
            out_assignments: vec![
                OutAssignment {
                    index: 0,
                    value: Expr::Ref("a".to_string()),
                    span: None,
                },
                OutAssignment {
                    index: 1,
                    value: Expr::Ref("b".to_string()),
                    span: None,
                },
            ],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();

        // Expr::Call("unpack") generates one unpack node
        // DestructuringWire reuses existing unpack node outlets
        let unpack_nodes: Vec<_> = graph
            .nodes
            .iter()
            .filter(|n| n.object_name == "unpack")
            .collect();
        assert_eq!(unpack_nodes.len(), 1);

        let unpack_node = unpack_nodes[0];
        assert_eq!(unpack_node.num_outlets, 2);
        assert!(!unpack_node.is_signal);

        // Edge: inlet -> unpack
        let edges_to_unpack: Vec<_> = graph
            .edges
            .iter()
            .filter(|e| e.dest_id == unpack_node.id)
            .collect();
        assert_eq!(edges_to_unpack.len(), 1);

        // a and b are each connected to outlet nodes
        let outlet_nodes: Vec<_> = graph
            .nodes
            .iter()
            .filter(|n| n.object_name == "outlet")
            .collect();
        assert_eq!(outlet_nodes.len(), 2);

        // 2 edges from unpack -> outlet
        let edges_from_unpack: Vec<_> = graph
            .edges
            .iter()
            .filter(|e| e.source_id == unpack_node.id)
            .collect();
        assert_eq!(edges_from_unpack.len(), 2);

        // From outlet 0 and outlet 1 respectively
        let mut source_outlets: Vec<u32> =
            edges_from_unpack.iter().map(|e| e.source_outlet).collect();
        source_outlets.sort();
        assert_eq!(source_outlets, vec![0, 1]);
    }

    #[test]
    fn test_destructuring_with_ref_auto_unpack() {
        // wire (a, b) = packed; -> packed node has insufficient outlets, so auto-insert unpack
        use flutmax_ast::DestructuringWire;

        let prog = Program {
            in_decls: vec![
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
            out_decls: vec![],
            wires: vec![Wire {
                name: "packed".to_string(),
                value: Expr::Tuple(vec![Expr::Ref("x".to_string()), Expr::Ref("y".to_string())]),
                span: None,
                attrs: vec![],
            }],
            destructuring_wires: vec![DestructuringWire {
                names: vec!["a".to_string(), "b".to_string()],
                value: Expr::Ref("packed".to_string()),
                span: None,
            }],
            msg_decls: vec![],
            out_assignments: vec![],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();

        // One pack node (tuple)
        let pack_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "pack")
            .expect("pack node should exist");
        assert_eq!(pack_node.num_outlets, 1);

        // pack has 1 outlet, so DestructuringWire auto-inserts unpack
        let unpack_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "unpack")
            .expect("unpack node should be auto-inserted");
        assert_eq!(unpack_node.num_outlets, 2);
        assert_eq!(unpack_node.args, vec!["f", "f"]);

        // Edge: pack -> unpack
        let pack_to_unpack = graph
            .edges
            .iter()
            .find(|e| e.source_id == pack_node.id && e.dest_id == unpack_node.id)
            .expect("edge from pack to unpack should exist");
        assert_eq!(pack_to_unpack.dest_inlet, 0);
    }

    #[test]
    fn test_tuple_two_elements_pack() {
        // wire t = (a, b); → pack f f
        let prog = Program {
            in_decls: vec![
                InDecl {
                    index: 0,
                    name: "a".to_string(),
                    port_type: PortType::Float,
                },
                InDecl {
                    index: 1,
                    name: "b".to_string(),
                    port_type: PortType::Float,
                },
            ],
            out_decls: vec![],
            wires: vec![Wire {
                name: "t".to_string(),
                value: Expr::Tuple(vec![Expr::Ref("a".to_string()), Expr::Ref("b".to_string())]),
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
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();

        let pack_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "pack")
            .expect("pack node should exist");
        assert_eq!(pack_node.num_inlets, 2);
        assert_eq!(pack_node.args, vec!["f", "f"]);
    }

    // ─── Feedback tests ───

    #[test]
    fn test_feedback_generates_tapin_node() {
        // feedback fb: signal; -> tapin~ node is generated
        use flutmax_ast::FeedbackDecl;

        let prog = Program {
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
            wires: vec![
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
                            CallArg::positional(Expr::Call {
                                object: "mul~".to_string(),
                                args: vec![
                                    CallArg::positional(Expr::Ref("delayed".to_string())),
                                    CallArg::positional(Expr::Lit(LitValue::Float(0.3))),
                                ],
                            }),
                        ],
                    },
                    span: None,
                    attrs: vec![],
                },
            ],
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![OutAssignment {
                index: 0,
                value: Expr::Ref("mixed".to_string()),
                span: None,
            }],
            direct_connections: vec![],
            feedback_decls: vec![FeedbackDecl {
                name: "fb".to_string(),
                port_type: PortType::Signal,
                span: None,
            }],
            feedback_assignments: vec![FeedbackAssignment {
                target: "fb".to_string(),
                value: Expr::Call {
                    object: "tapin~".to_string(),
                    args: vec![
                        CallArg::positional(Expr::Ref("mixed".to_string())),
                        CallArg::positional(Expr::Lit(LitValue::Int(1000))),
                    ],
                },
                span: None,
            }],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();

        // tapin~ node exists
        let tapin_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "tapin~")
            .expect("tapin~ node should exist");
        assert!(tapin_node.is_signal);
        assert_eq!(tapin_node.num_inlets, 1);
        assert_eq!(tapin_node.num_outlets, 1);

        // tapout~ node exists
        let tapout_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "tapout~")
            .expect("tapout~ node should exist");
        assert!(tapout_node.is_signal);

        // Edge tapin~ -> tapout~ exists
        let tapin_to_tapout = graph
            .edges
            .iter()
            .find(|e| e.source_id == tapin_node.id && e.dest_id == tapout_node.id)
            .expect("edge from tapin~ to tapout~ should exist");
        assert_eq!(tapin_to_tapout.source_outlet, 0);
        assert_eq!(tapin_to_tapout.dest_inlet, 0);
        // tapin~ -> tapout~ is a normal edge (is_feedback is on the assignment edge)
        assert!(!tapin_to_tapout.is_feedback);

        // feedback assignment edge has is_feedback=true
        let feedback_edges: Vec<_> = graph.edges.iter().filter(|e| e.is_feedback).collect();
        assert_eq!(
            feedback_edges.len(),
            1,
            "should have exactly one feedback edge"
        );
    }

    #[test]
    fn test_feedback_no_trigger_on_feedback_edge() {
        // Verify trigger is not inserted for feedback edges
        use flutmax_ast::FeedbackDecl;

        let prog = Program {
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
            wires: vec![
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
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![OutAssignment {
                index: 0,
                value: Expr::Ref("mixed".to_string()),
                span: None,
            }],
            direct_connections: vec![],
            feedback_decls: vec![FeedbackDecl {
                name: "fb".to_string(),
                port_type: PortType::Signal,
                span: None,
            }],
            feedback_assignments: vec![FeedbackAssignment {
                target: "fb".to_string(),
                value: Expr::Call {
                    object: "tapin~".to_string(),
                    args: vec![
                        CallArg::positional(Expr::Ref("mixed".to_string())),
                        CallArg::positional(Expr::Lit(LitValue::Int(1000))),
                    ],
                },
                span: None,
            }],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();

        // Confirm no trigger node was inserted
        // (all Signal, so no trigger needed)
        let trigger_nodes: Vec<_> = graph
            .nodes
            .iter()
            .filter(|n| n.object_name == "trigger")
            .collect();
        assert_eq!(
            trigger_nodes.len(),
            0,
            "no trigger nodes should be inserted for signal-only feedback"
        );
    }

    // ─── E004: NoOutDeclaration tests ───

    #[test]
    fn test_e004_no_out_declaration_detected() {
        // Assign to out[0] without out declaration -> E004
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![Wire {
                name: "x".to_string(),
                value: Expr::Call {
                    object: "button".to_string(),
                    args: vec![],
                },
                span: None,
                attrs: vec![],
            }],
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![OutAssignment {
                index: 0,
                value: Expr::Ref("x".to_string()),
                span: None,
            }],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let result = build_graph(&prog);
        assert!(result.is_err());
        match result.unwrap_err() {
            BuildError::NoOutDeclaration(idx) => assert_eq!(idx, 0),
            other => panic!("expected NoOutDeclaration, got {:?}", other),
        }
    }

    #[test]
    fn test_e004_valid_out_declaration_no_error() {
        // Assign to out[0] with out declaration -> no error
        let prog = Program {
            in_decls: vec![],
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
                    args: vec![CallArg::positional(Expr::Lit(LitValue::Int(440)))],
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
        };

        let result = build_graph(&prog);
        assert!(result.is_ok());
    }

    // ─── E006: DestructuringCountMismatch tests ───

    #[test]
    fn test_e006_destructuring_count_mismatch_detected() {
        // unpack has 2 outlets but 3 names -> E006
        use flutmax_ast::DestructuringWire;

        let prog = Program {
            in_decls: vec![InDecl {
                index: 0,
                name: "data".to_string(),
                port_type: PortType::Float,
            }],
            out_decls: vec![],
            wires: vec![],
            destructuring_wires: vec![DestructuringWire {
                names: vec!["a".to_string(), "b".to_string(), "c".to_string()],
                value: Expr::Call {
                    object: "unpack".to_string(),
                    args: vec![CallArg::positional(Expr::Ref("data".to_string()))],
                },
                span: None,
            }],
            msg_decls: vec![],
            out_assignments: vec![],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let result = build_graph(&prog);
        assert!(result.is_err());
        match result.unwrap_err() {
            BuildError::DestructuringCountMismatch { expected, got } => {
                assert_eq!(expected, 2);
                assert_eq!(got, 3);
            }
            other => panic!("expected DestructuringCountMismatch, got {:?}", other),
        }
    }

    #[test]
    fn test_e006_destructuring_count_match_no_error() {
        // unpack has 2 outlets and 2 names -> no error
        use flutmax_ast::DestructuringWire;

        let prog = Program {
            in_decls: vec![InDecl {
                index: 0,
                name: "data".to_string(),
                port_type: PortType::Float,
            }],
            out_decls: vec![],
            wires: vec![],
            destructuring_wires: vec![DestructuringWire {
                names: vec!["a".to_string(), "b".to_string()],
                value: Expr::Call {
                    object: "unpack".to_string(),
                    args: vec![CallArg::positional(Expr::Ref("data".to_string()))],
                },
                span: None,
            }],
            msg_decls: vec![],
            out_assignments: vec![],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let result = build_graph(&prog);
        assert!(result.is_ok());
    }

    // ─── E009: AbstractionArgCountMismatch tests ───

    #[test]
    fn test_e009_abstraction_arg_count_mismatch_detected() {
        // oscillator has 1 in_port but called with 2 args -> E009
        let mut registry = AbstractionRegistry::new();
        registry.register("oscillator", &make_oscillator_program());

        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![Wire {
                name: "osc".to_string(),
                value: Expr::Call {
                    object: "oscillator".to_string(),
                    args: vec![
                        CallArg::positional(Expr::Lit(LitValue::Int(440))),
                        CallArg::positional(Expr::Lit(LitValue::Float(0.5))),
                    ],
                },
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
            state_assignments: vec![],
        };

        let result = build_graph_with_registry(&prog, Some(&registry));
        assert!(result.is_err());
        match result.unwrap_err() {
            BuildError::AbstractionArgCountMismatch {
                name,
                expected,
                got,
            } => {
                assert_eq!(name, "oscillator");
                assert_eq!(expected, 1);
                assert_eq!(got, 2);
            }
            other => panic!("expected AbstractionArgCountMismatch, got {:?}", other),
        }
    }

    #[test]
    fn test_e009_abstraction_arg_count_match_no_error() {
        // oscillator has 1 in_port with 1 arg -> no error
        let mut registry = AbstractionRegistry::new();
        registry.register("oscillator", &make_oscillator_program());

        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![Wire {
                name: "osc".to_string(),
                value: Expr::Call {
                    object: "oscillator".to_string(),
                    args: vec![CallArg::positional(Expr::Lit(LitValue::Int(440)))],
                },
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
            state_assignments: vec![],
        };

        let result = build_graph_with_registry(&prog, Some(&registry));
        assert!(result.is_ok());
    }

    // ─── E013: DuplicateFeedbackAssignment tests ───

    #[test]
    fn test_e013_duplicate_feedback_assignment_detected() {
        // 2 assignments to same feedback variable -> E013
        use flutmax_ast::{FeedbackAssignment, FeedbackDecl};

        let prog = Program {
            in_decls: vec![InDecl {
                index: 0,
                name: "input".to_string(),
                port_type: PortType::Signal,
            }],
            out_decls: vec![],
            wires: vec![Wire {
                name: "sig".to_string(),
                value: Expr::Call {
                    object: "cycle~".to_string(),
                    args: vec![CallArg::positional(Expr::Ref("input".to_string()))],
                },
                span: None,
                attrs: vec![],
            }],
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![],
            direct_connections: vec![],
            feedback_decls: vec![FeedbackDecl {
                name: "fb".to_string(),
                port_type: PortType::Signal,
                span: None,
            }],
            feedback_assignments: vec![
                FeedbackAssignment {
                    target: "fb".to_string(),
                    value: Expr::Ref("sig".to_string()),
                    span: None,
                },
                FeedbackAssignment {
                    target: "fb".to_string(),
                    value: Expr::Ref("sig".to_string()),
                    span: None,
                },
            ],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let result = build_graph(&prog);
        assert!(result.is_err());
        match result.unwrap_err() {
            BuildError::DuplicateFeedbackAssignment(name) => assert_eq!(name, "fb"),
            other => panic!("expected DuplicateFeedbackAssignment, got {:?}", other),
        }
    }

    #[test]
    fn test_e013_single_feedback_assignment_no_error() {
        // 1 feedback assignment -> no error
        use flutmax_ast::{FeedbackAssignment, FeedbackDecl};

        let prog = Program {
            in_decls: vec![InDecl {
                index: 0,
                name: "input".to_string(),
                port_type: PortType::Signal,
            }],
            out_decls: vec![],
            wires: vec![Wire {
                name: "sig".to_string(),
                value: Expr::Call {
                    object: "cycle~".to_string(),
                    args: vec![CallArg::positional(Expr::Ref("input".to_string()))],
                },
                span: None,
                attrs: vec![],
            }],
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![],
            direct_connections: vec![],
            feedback_decls: vec![FeedbackDecl {
                name: "fb".to_string(),
                port_type: PortType::Signal,
                span: None,
            }],
            feedback_assignments: vec![FeedbackAssignment {
                target: "fb".to_string(),
                value: Expr::Ref("sig".to_string()),
                span: None,
            }],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let result = build_graph(&prog);
        assert!(result.is_ok());
    }

    // ─── E17: Edge Order tests ───

    #[test]
    fn test_fanout_edges_get_order() {
        // cycle~ -> outlet~ x2 (Signal fanout) -> order is assigned
        let prog = Program {
            in_decls: vec![],
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
                    args: vec![CallArg::positional(Expr::Lit(LitValue::Int(440)))],
                },
                span: None,
                attrs: vec![],
            }],
            destructuring_wires: vec![],
            msg_decls: vec![],
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
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();

        // 2 edges from cycle~, with order assigned
        let cycle_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "cycle~")
            .unwrap();
        let edges_from_cycle: Vec<_> = graph
            .edges
            .iter()
            .filter(|e| e.source_id == cycle_node.id && e.source_outlet == 0)
            .collect();
        assert_eq!(edges_from_cycle.len(), 2);

        // Both have Some order
        assert!(edges_from_cycle[0].order.is_some());
        assert!(edges_from_cycle[1].order.is_some());

        // order is 0 and 1
        let mut orders: Vec<u32> = edges_from_cycle.iter().map(|e| e.order.unwrap()).collect();
        orders.sort();
        assert_eq!(orders, vec![0, 1]);
    }

    #[test]
    fn test_single_edge_no_order() {
        // Single-connection edges are not assigned order
        let prog = make_l2_program();
        let graph = build_graph(&prog).unwrap();

        // All edges have order: None
        for edge in &graph.edges {
            assert_eq!(
                edge.order, None,
                "single edge from {} outlet {} should have no order",
                edge.source_id, edge.source_outlet
            );
        }
    }

    // ─── E17: Purity Classification tests ───

    #[test]
    fn test_classify_purity_signal_pure() {
        assert_eq!(classify_purity("cycle~"), NodePurity::Pure);
        assert_eq!(classify_purity("*~"), NodePurity::Pure);
        assert_eq!(classify_purity("+~"), NodePurity::Pure);
        assert_eq!(classify_purity("biquad~"), NodePurity::Pure);
    }

    #[test]
    fn test_classify_purity_signal_stateful() {
        assert_eq!(classify_purity("tapin~"), NodePurity::Stateful);
        assert_eq!(classify_purity("tapout~"), NodePurity::Stateful);
        assert_eq!(classify_purity("line~"), NodePurity::Stateful);
        assert_eq!(classify_purity("delay~"), NodePurity::Stateful);
    }

    #[test]
    fn test_classify_purity_control_stateful() {
        assert_eq!(classify_purity("pack"), NodePurity::Stateful);
        assert_eq!(classify_purity("unpack"), NodePurity::Stateful);
        assert_eq!(classify_purity("int"), NodePurity::Stateful);
        assert_eq!(classify_purity("float"), NodePurity::Stateful);
        assert_eq!(classify_purity("toggle"), NodePurity::Stateful);
        assert_eq!(classify_purity("gate"), NodePurity::Stateful);
        assert_eq!(classify_purity("counter"), NodePurity::Stateful);
        assert_eq!(classify_purity("coll"), NodePurity::Stateful);
        assert_eq!(classify_purity("dict"), NodePurity::Stateful);
    }

    #[test]
    fn test_classify_purity_control_pure() {
        assert_eq!(classify_purity("+"), NodePurity::Pure);
        assert_eq!(classify_purity("-"), NodePurity::Pure);
        assert_eq!(classify_purity("*"), NodePurity::Pure);
        assert_eq!(classify_purity("/"), NodePurity::Pure);
        assert_eq!(classify_purity("trigger"), NodePurity::Pure);
        assert_eq!(classify_purity("t"), NodePurity::Pure);
        assert_eq!(classify_purity("route"), NodePurity::Pure);
        assert_eq!(classify_purity("select"), NodePurity::Pure);
        assert_eq!(classify_purity("prepend"), NodePurity::Pure);
    }

    #[test]
    fn test_classify_purity_unknown() {
        assert_eq!(classify_purity("my_custom_object"), NodePurity::Unknown);
        assert_eq!(classify_purity("some_abstraction"), NodePurity::Unknown);
    }

    // ─── E17: Hot/Cold Inlets tests ───

    #[test]
    fn test_default_hot_inlets_standard() {
        // inlet 0 = hot, others = cold
        let hot = default_hot_inlets("cycle~", 2);
        assert_eq!(hot, vec![true, false]);
    }

    #[test]
    fn test_default_hot_inlets_single() {
        let hot = default_hot_inlets("print", 1);
        assert_eq!(hot, vec![true]);
    }

    #[test]
    fn test_default_hot_inlets_none() {
        let hot = default_hot_inlets("inlet", 0);
        assert!(hot.is_empty());
    }

    #[test]
    fn test_default_hot_inlets_many() {
        let hot = default_hot_inlets("biquad~", 6);
        assert_eq!(hot, vec![true, false, false, false, false, false]);
    }

    // ─── E17: Graph Node Attributes tests ───

    #[test]
    fn test_built_node_has_purity() {
        let prog = make_l2_program();
        let graph = build_graph(&prog).unwrap();

        let cycle_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "cycle~")
            .unwrap();
        assert_eq!(cycle_node.purity, NodePurity::Pure);

        let mul_node = graph.nodes.iter().find(|n| n.object_name == "*~").unwrap();
        assert_eq!(mul_node.purity, NodePurity::Pure);
    }

    #[test]
    fn test_built_node_has_hot_inlets() {
        let prog = make_l2_program();
        let graph = build_graph(&prog).unwrap();

        let cycle_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "cycle~")
            .unwrap();
        assert_eq!(cycle_node.hot_inlets, vec![true, false]);

        let mul_node = graph.nodes.iter().find(|n| n.object_name == "*~").unwrap();
        assert_eq!(mul_node.hot_inlets, vec![true, false]);
    }

    // ─── E007: InvalidPortIndex tests ───

    #[test]
    fn test_direct_connection_valid_port() {
        // node.in[0] = expr; — valid port index
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![
                Wire {
                    name: "src".to_string(),
                    value: Expr::Call {
                        object: "button".to_string(),
                        args: vec![],
                    },
                    span: None,
                    attrs: vec![],
                },
                Wire {
                    name: "target".to_string(),
                    value: Expr::Call {
                        object: "+".to_string(),
                        args: vec![
                            CallArg::positional(Expr::Lit(LitValue::Int(0))),
                            CallArg::positional(Expr::Lit(LitValue::Int(0))),
                        ],
                    },
                    span: None,
                    attrs: vec![],
                },
            ],
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![],
            direct_connections: vec![DirectConnection {
                target: flutmax_ast::InputPortAccess {
                    object: "target".to_string(),
                    index: 0,
                },
                value: Expr::Ref("src".to_string()),
            }],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let result = build_graph(&prog);
        assert!(result.is_ok(), "valid port index should succeed");
    }

    #[test]
    fn test_direct_connection_invalid_port_index() {
        // node.in[99] = expr; — out-of-range port index -> E007
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![
                Wire {
                    name: "src".to_string(),
                    value: Expr::Call {
                        object: "button".to_string(),
                        args: vec![],
                    },
                    span: None,
                    attrs: vec![],
                },
                Wire {
                    name: "target".to_string(),
                    value: Expr::Call {
                        object: "+".to_string(),
                        args: vec![
                            CallArg::positional(Expr::Lit(LitValue::Int(0))),
                            CallArg::positional(Expr::Lit(LitValue::Int(0))),
                        ],
                    },
                    span: None,
                    attrs: vec![],
                },
            ],
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![],
            direct_connections: vec![DirectConnection {
                target: flutmax_ast::InputPortAccess {
                    object: "target".to_string(),
                    index: 99,
                },
                value: Expr::Ref("src".to_string()),
            }],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        // Port index exceeding initial num_inlets auto-extends the node
        // (needed for decompiler back-edge direct_connections).
        let result = build_graph(&prog);
        assert!(result.is_ok());
        let graph = result.unwrap();
        let target_node = graph
            .find_node("target_id_0")
            .or_else(|| graph.nodes.iter().find(|n| n.object_name == "+"));
        assert!(target_node.is_some());
        // The node should now have at least 100 inlets (index 99 + 1)
        assert!(target_node.unwrap().num_inlets >= 100);
    }

    #[test]
    fn test_direct_connection_undefined_node() {
        // nonexistent.in[0] = expr; — undefined node
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![Wire {
                name: "src".to_string(),
                value: Expr::Call {
                    object: "button".to_string(),
                    args: vec![],
                },
                span: None,
                attrs: vec![],
            }],
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![],
            direct_connections: vec![DirectConnection {
                target: flutmax_ast::InputPortAccess {
                    object: "nonexistent".to_string(),
                    index: 0,
                },
                value: Expr::Ref("src".to_string()),
            }],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let result = build_graph(&prog);
        assert!(result.is_err());
        match result.unwrap_err() {
            BuildError::UndefinedRef(name) => assert_eq!(name, "nonexistent"),
            other => panic!("expected UndefinedRef, got: {:?}", other),
        }
    }

    // ─── Typed Pack tests ───

    #[test]
    fn test_typed_pack_int_literals() {
        // wire t = (1, 2, 3); → pack i i i
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![Wire {
                name: "t".to_string(),
                value: Expr::Tuple(vec![
                    Expr::Lit(LitValue::Int(1)),
                    Expr::Lit(LitValue::Int(2)),
                    Expr::Lit(LitValue::Int(3)),
                ]),
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
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();
        let pack_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "pack")
            .expect("pack node should exist");
        assert_eq!(pack_node.args, vec!["i", "i", "i"]);
    }

    #[test]
    fn test_typed_pack_mixed_literals() {
        // wire t = (1, 0.5, "x"); → pack i f s
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![Wire {
                name: "t".to_string(),
                value: Expr::Tuple(vec![
                    Expr::Lit(LitValue::Int(1)),
                    Expr::Lit(LitValue::Float(0.5)),
                    Expr::Lit(LitValue::Str("x".to_string())),
                ]),
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
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();
        let pack_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "pack")
            .expect("pack node should exist");
        assert_eq!(pack_node.args, vec!["i", "f", "s"]);
    }

    #[test]
    fn test_typed_pack_ref_fallback() {
        // wire t = (x, y); -> pack f f (Ref falls back)
        let prog = Program {
            in_decls: vec![
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
            out_decls: vec![],
            wires: vec![Wire {
                name: "t".to_string(),
                value: Expr::Tuple(vec![Expr::Ref("x".to_string()), Expr::Ref("y".to_string())]),
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
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();
        let pack_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "pack")
            .expect("pack node should exist");
        assert_eq!(pack_node.args, vec!["f", "f"]);
    }

    // ─── E020: bare multi-outlet ref tests ───

    #[test]
    fn test_bare_multi_outlet_ref_ok() {
        // wire x = line~(arg0); out[0] = x; -> OK (bare = outlet 0, E020 removed)
        let prog = Program {
            in_decls: vec![InDecl {
                index: 0,
                name: "arg0".to_string(),
                port_type: PortType::Signal,
            }],
            out_decls: vec![OutDecl {
                index: 0,
                name: "out".to_string(),
                port_type: PortType::Signal,
                value: None,
            }],
            wires: vec![Wire {
                name: "result".to_string(),
                value: Expr::Call {
                    object: "line~".to_string(),
                    args: vec![CallArg::positional(Expr::Ref("arg0".to_string()))],
                },
                span: None,
                attrs: vec![],
            }],
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![OutAssignment {
                index: 0,
                value: Expr::Ref("result".to_string()),
                span: None,
            }],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let result = build_graph(&prog);
        assert!(
            result.is_ok(),
            "bare reference to multi-outlet node should be OK"
        );
    }

    #[test]
    fn test_e020_output_port_access_ok() {
        // wire x = line~(arg0); out[0] = x.out[0]; → OK
        use flutmax_ast::OutputPortAccess;

        let prog = Program {
            in_decls: vec![InDecl {
                index: 0,
                name: "arg0".to_string(),
                port_type: PortType::Signal,
            }],
            out_decls: vec![OutDecl {
                index: 0,
                name: "out".to_string(),
                port_type: PortType::Signal,
                value: None,
            }],
            wires: vec![Wire {
                name: "result".to_string(),
                value: Expr::Call {
                    object: "line~".to_string(),
                    args: vec![CallArg::positional(Expr::Ref("arg0".to_string()))],
                },
                span: None,
                attrs: vec![],
            }],
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![OutAssignment {
                index: 0,
                value: Expr::OutputPortAccess(OutputPortAccess {
                    object: "result".to_string(),
                    index: 0,
                }),
                span: None,
            }],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let result = build_graph(&prog);
        assert!(
            result.is_ok(),
            "OutputPortAccess should bypass E020: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_e020_destructured_names_exempt() {
        // wire (a, b) = unpack(data); out[0] = a; → OK (destructured names exempt from E020)
        use flutmax_ast::DestructuringWire;

        let prog = Program {
            in_decls: vec![InDecl {
                index: 0,
                name: "data".to_string(),
                port_type: PortType::Float,
            }],
            out_decls: vec![OutDecl {
                index: 0,
                name: "x".to_string(),
                port_type: PortType::Float,
                value: None,
            }],
            wires: vec![],
            destructuring_wires: vec![DestructuringWire {
                names: vec!["a".to_string(), "b".to_string()],
                value: Expr::Call {
                    object: "unpack".to_string(),
                    args: vec![CallArg::positional(Expr::Ref("data".to_string()))],
                },
                span: None,
            }],
            msg_decls: vec![],
            out_assignments: vec![OutAssignment {
                index: 0,
                value: Expr::Ref("a".to_string()),
                span: None,
            }],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let result = build_graph(&prog);
        assert!(
            result.is_ok(),
            "destructured name should not trigger E020: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_single_outlet_bare_ref_ok() {
        // wire x = cycle~(440); out[0] = x; → OK (single outlet)
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![OutDecl {
                index: 0,
                name: "out".to_string(),
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
        };

        let result = build_graph(&prog);
        assert!(
            result.is_ok(),
            "single outlet bare ref should be OK: {:?}",
            result.err()
        );
    }

    // ─── State tests ───

    #[test]
    fn test_state_decl_creates_int_node() {
        // state counter: int = 0; -> [int 0] node is generated
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
                name: "counter".to_string(),
                port_type: PortType::Int,
                init_value: Expr::Lit(LitValue::Int(0)),
                span: None,
            }],
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();

        assert_eq!(graph.nodes.len(), 1);
        let node = &graph.nodes[0];
        assert_eq!(node.object_name, "int");
        assert_eq!(node.args, vec!["0"]);
        assert_eq!(node.num_inlets, 2);
        assert_eq!(node.num_outlets, 1);
        assert!(!node.is_signal);
        assert_eq!(node.varname, Some("counter".to_string()));
        // inlet 0 hot, inlet 1 cold
        assert_eq!(node.hot_inlets, vec![true, false]);
    }

    #[test]
    fn test_state_decl_creates_float_node() {
        // state volume: float = 0.5; -> [float 0.5] node is generated
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
                name: "volume".to_string(),
                port_type: PortType::Float,
                init_value: Expr::Lit(LitValue::Float(0.5)),
                span: None,
            }],
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();

        assert_eq!(graph.nodes.len(), 1);
        let node = &graph.nodes[0];
        assert_eq!(node.object_name, "float");
        assert_eq!(node.args, vec!["0.5"]);
        assert_eq!(node.varname, Some("volume".to_string()));
    }

    #[test]
    fn test_state_assignment_connects_to_cold_inlet() {
        // state counter: int = 0;
        // wire next = add(counter, 1);
        // state counter = next;
        // -> next -> int inlet 1 (cold)
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
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
            state_assignments: vec![StateAssignment {
                name: "counter".to_string(),
                value: Expr::Ref("next".to_string()),
                span: None,
            }],
        };

        let graph = build_graph(&prog).unwrap();

        // int node (state)
        let int_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "int")
            .expect("int node should exist");

        // add node (next)
        let add_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "+")
            .expect("add node should exist");

        // Edge add -> int inlet 1 exists
        let edge = graph
            .edges
            .iter()
            .find(|e| e.source_id == add_node.id && e.dest_id == int_node.id)
            .expect("edge from add to int should exist");
        assert_eq!(
            edge.dest_inlet, 1,
            "state assignment should connect to cold inlet (1)"
        );
    }

    #[test]
    fn test_state_ref_in_wire_expression() {
        // state counter: int = 0;
        // wire next = add(counter, 1);
        // -> counter reference creates edge from int node outlet 0 -> add inlet 0
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
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
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();

        let int_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "int")
            .expect("int node should exist");
        let add_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "+")
            .expect("add node should exist");

        // Edge int -> add inlet 0 exists
        let edge = graph
            .edges
            .iter()
            .find(|e| e.source_id == int_node.id && e.dest_id == add_node.id)
            .expect("edge from int to add should exist");
        assert_eq!(edge.source_outlet, 0);
        assert_eq!(edge.dest_inlet, 0);
    }

    #[test]
    fn test_e019_duplicate_state_assignment() {
        // state counter: int = 0;
        // state counter = a;
        // state counter = b;  → E019
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![
                Wire {
                    name: "a".to_string(),
                    value: Expr::Call {
                        object: "button".to_string(),
                        args: vec![],
                    },
                    span: None,
                    attrs: vec![],
                },
                Wire {
                    name: "b".to_string(),
                    value: Expr::Call {
                        object: "button".to_string(),
                        args: vec![],
                    },
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
                    span: None,
                },
            ],
        };

        let result = build_graph(&prog);
        assert!(result.is_err());
        match result.unwrap_err() {
            BuildError::DuplicateStateAssignment(name) => assert_eq!(name, "counter"),
            other => panic!("expected DuplicateStateAssignment, got {:?}", other),
        }
    }

    #[test]
    fn test_state_single_assignment_no_error() {
        // Single state assignment does not error
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![Wire {
                name: "val".to_string(),
                value: Expr::Call {
                    object: "button".to_string(),
                    args: vec![],
                },
                span: None,
                attrs: vec![],
            }],
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
            state_assignments: vec![StateAssignment {
                name: "counter".to_string(),
                value: Expr::Ref("val".to_string()),
                span: None,
            }],
        };

        let result = build_graph(&prog);
        assert!(result.is_ok());
    }

    // ─── E20: Typed Destructuring (unpack subtype propagation) tests ───

    #[test]
    fn test_typed_unpack_from_int_tuple() {
        // wire t = (1, 2, 3); wire (a, b, c) = t;
        // → auto-inserted unpack should have args ["i", "i", "i"]
        use flutmax_ast::DestructuringWire;

        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![Wire {
                name: "t".to_string(),
                value: Expr::Tuple(vec![
                    Expr::Lit(LitValue::Int(1)),
                    Expr::Lit(LitValue::Int(2)),
                    Expr::Lit(LitValue::Int(3)),
                ]),
                span: None,
                attrs: vec![],
            }],
            destructuring_wires: vec![DestructuringWire {
                names: vec!["a".to_string(), "b".to_string(), "c".to_string()],
                value: Expr::Ref("t".to_string()),
                span: None,
            }],
            msg_decls: vec![],
            out_assignments: vec![],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();
        let unpack_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "unpack")
            .expect("unpack node should be auto-inserted");
        assert_eq!(unpack_node.args, vec!["i", "i", "i"]);
    }

    #[test]
    fn test_typed_unpack_from_mixed_tuple() {
        // wire t = (1, 0.5, "x"); wire (a, b, c) = t;
        // → auto-inserted unpack should have args ["i", "f", "s"]
        use flutmax_ast::DestructuringWire;

        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![Wire {
                name: "t".to_string(),
                value: Expr::Tuple(vec![
                    Expr::Lit(LitValue::Int(1)),
                    Expr::Lit(LitValue::Float(0.5)),
                    Expr::Lit(LitValue::Str("x".to_string())),
                ]),
                span: None,
                attrs: vec![],
            }],
            destructuring_wires: vec![DestructuringWire {
                names: vec!["a".to_string(), "b".to_string(), "c".to_string()],
                value: Expr::Ref("t".to_string()),
                span: None,
            }],
            msg_decls: vec![],
            out_assignments: vec![],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();
        let unpack_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "unpack")
            .expect("unpack node should be auto-inserted");
        assert_eq!(unpack_node.args, vec!["i", "f", "s"]);
    }

    #[test]
    fn test_typed_unpack_unknown_source_fallback() {
        // wire (a, b) = unpack(data); — data is an inlet (not tuple)
        // → unpack should have default args ["f", "f"]
        use flutmax_ast::DestructuringWire;

        let prog = Program {
            in_decls: vec![InDecl {
                index: 0,
                name: "data".to_string(),
                port_type: PortType::Float,
            }],
            out_decls: vec![],
            wires: vec![],
            destructuring_wires: vec![DestructuringWire {
                names: vec!["a".to_string(), "b".to_string()],
                value: Expr::Call {
                    object: "unpack".to_string(),
                    args: vec![CallArg::positional(Expr::Ref("data".to_string()))],
                },
                span: None,
            }],
            msg_decls: vec![],
            out_assignments: vec![],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();
        let unpack_nodes: Vec<_> = graph
            .nodes
            .iter()
            .filter(|n| n.object_name == "unpack")
            .collect();
        assert_eq!(unpack_nodes.len(), 1);
        // Explicit unpack call: the resolve_expr creates it with default num_outlets=2
        // which matches names count, so no auto-inserted unpack needed.
        // The Call-generated unpack has its own arg handling.
    }

    #[test]
    fn test_typed_unpack_ref_to_tuple_with_refs() {
        // wire t = (x, y); wire (a, b) = t;
        // → Ref elements fall back to "f", so unpack args = ["f", "f"]
        use flutmax_ast::DestructuringWire;

        let prog = Program {
            in_decls: vec![
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
            out_decls: vec![],
            wires: vec![Wire {
                name: "t".to_string(),
                value: Expr::Tuple(vec![Expr::Ref("x".to_string()), Expr::Ref("y".to_string())]),
                span: None,
                attrs: vec![],
            }],
            destructuring_wires: vec![DestructuringWire {
                names: vec!["a".to_string(), "b".to_string()],
                value: Expr::Ref("t".to_string()),
                span: None,
            }],
            msg_decls: vec![],
            out_assignments: vec![],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();
        let unpack_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "unpack")
            .expect("unpack node should be auto-inserted");
        assert_eq!(unpack_node.args, vec!["f", "f"]);
    }

    // ========================================
    // W001: Duplicate connection to same inlet warning
    // ========================================

    #[test]
    fn test_w001_duplicate_inlet_detected() {
        // 2 connections to target.in[0] -> W001 warning
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![
                Wire {
                    name: "a".to_string(),
                    value: Expr::Call {
                        object: "button".to_string(),
                        args: vec![],
                    },
                    span: None,
                    attrs: vec![],
                },
                Wire {
                    name: "b".to_string(),
                    value: Expr::Call {
                        object: "button".to_string(),
                        args: vec![],
                    },
                    span: None,
                    attrs: vec![],
                },
                Wire {
                    name: "target".to_string(),
                    value: Expr::Call {
                        object: "+".to_string(),
                        args: vec![
                            CallArg::positional(Expr::Lit(LitValue::Int(0))),
                            CallArg::positional(Expr::Lit(LitValue::Int(0))),
                        ],
                    },
                    span: None,
                    attrs: vec![],
                },
            ],
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![],
            direct_connections: vec![
                DirectConnection {
                    target: flutmax_ast::InputPortAccess {
                        object: "target".to_string(),
                        index: 0,
                    },
                    value: Expr::Ref("a".to_string()),
                },
                DirectConnection {
                    target: flutmax_ast::InputPortAccess {
                        object: "target".to_string(),
                        index: 0,
                    },
                    value: Expr::Ref("b".to_string()),
                },
            ],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let result = build_graph_with_warnings(&prog).unwrap();
        assert_eq!(result.warnings.len(), 1);
        match &result.warnings[0] {
            BuildWarning::DuplicateInletConnection {
                node_id: _,
                inlet,
                count,
            } => {
                assert_eq!(*inlet, 0);
                assert_eq!(*count, 2);
            }
        }
    }

    #[test]
    fn test_w001_no_warning_single_connection() {
        // 1 connection per inlet -> no warning
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![
                Wire {
                    name: "a".to_string(),
                    value: Expr::Call {
                        object: "button".to_string(),
                        args: vec![],
                    },
                    span: None,
                    attrs: vec![],
                },
                Wire {
                    name: "target".to_string(),
                    value: Expr::Call {
                        object: "+".to_string(),
                        args: vec![
                            CallArg::positional(Expr::Lit(LitValue::Int(0))),
                            CallArg::positional(Expr::Lit(LitValue::Int(0))),
                        ],
                    },
                    span: None,
                    attrs: vec![],
                },
            ],
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![],
            direct_connections: vec![DirectConnection {
                target: flutmax_ast::InputPortAccess {
                    object: "target".to_string(),
                    index: 1,
                },
                value: Expr::Ref("a".to_string()),
            }],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let result = build_graph_with_warnings(&prog).unwrap();
        assert!(
            result.warnings.is_empty(),
            "single connections should not trigger W001"
        );
    }

    #[test]
    fn test_w001_display_format() {
        let warning = BuildWarning::DuplicateInletConnection {
            node_id: "obj-3".to_string(),
            inlet: 0,
            count: 2,
        };
        assert_eq!(format!("{}", warning), "W001: 2 connections to obj-3.in[0]");
    }

    // ─── msg declaration tests ───

    #[test]
    fn test_msg_creates_message_node() {
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![OutDecl {
                index: 0,
                name: "output".to_string(),
                port_type: PortType::Bang,
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

        let graph = build_graph(&prog).unwrap();

        // Find the message node
        let msg_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "message")
            .expect("should have a message node");

        assert_eq!(msg_node.args, vec!["bang"]);
        assert_eq!(msg_node.num_inlets, 2);
        assert_eq!(msg_node.num_outlets, 1);
        assert!(!msg_node.is_signal);
        assert_eq!(msg_node.varname, Some("click".to_string()));
    }

    #[test]
    fn test_msg_connectable_as_source() {
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![Wire {
                name: "printer".to_string(),
                value: Expr::Call {
                    object: "print".to_string(),
                    args: vec![CallArg::positional(Expr::Ref("click".to_string()))],
                },
                span: None,
                attrs: vec![],
            }],
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

        let graph = build_graph(&prog).unwrap();

        // Should have edge from message node to print node
        assert!(!graph.edges.is_empty(), "should have at least one edge");
        let msg_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "message")
            .expect("message node");
        let print_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "print")
            .expect("print node");

        let edge = graph
            .edges
            .iter()
            .find(|e| e.source_id == msg_node.id && e.dest_id == print_node.id)
            .expect("edge from message to print");
        assert_eq!(edge.source_outlet, 0);
        assert_eq!(edge.dest_inlet, 0);
    }

    // ─── Dotted identifier tests ───

    #[test]
    fn test_dotted_object_name_in_call() {
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![OutDecl {
                index: 0,
                name: "output".to_string(),
                port_type: PortType::Float,
                value: None,
            }],
            wires: vec![Wire {
                name: "dial".to_string(),
                value: Expr::Call {
                    object: "live.dial".to_string(),
                    args: vec![CallArg::positional(Expr::Lit(LitValue::Float(0.5)))],
                },
                span: None,
                attrs: vec![],
            }],
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![OutAssignment {
                index: 0,
                value: Expr::OutputPortAccess(OutputPortAccess {
                    object: "dial".to_string(),
                    index: 0,
                }),
                span: None,
            }],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();

        // Should have a live.dial node
        let dial_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "live.dial")
            .expect("should have a live.dial node");
        assert_eq!(dial_node.args, vec!["0.5"]);
    }

    // ================================================
    // .attr() chain builder tests
    // ================================================

    #[test]
    fn test_wire_attrs_propagated_to_node() {
        use flutmax_ast::AttrPair;

        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![Wire {
                name: "w".to_string(),
                value: Expr::Call {
                    object: "flonum".to_string(),
                    args: vec![],
                },
                span: None,
                attrs: vec![
                    AttrPair {
                        key: "minimum".to_string(),
                        value: flutmax_ast::AttrValue::Float(0.0),
                    },
                    AttrPair {
                        key: "maximum".to_string(),
                        value: flutmax_ast::AttrValue::Float(100.0),
                    },
                ],
            }],
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();

        let fnum = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "flonum")
            .expect("should have a flonum node");

        assert_eq!(fnum.attrs.len(), 2);
        assert_eq!(fnum.attrs[0], ("minimum".to_string(), "0.".to_string()));
        assert_eq!(fnum.attrs[1], ("maximum".to_string(), "100.".to_string()));
    }

    #[test]
    fn test_msg_attrs_propagated_to_node() {
        use flutmax_ast::AttrPair;

        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![],
            destructuring_wires: vec![],
            msg_decls: vec![MsgDecl {
                name: "click".to_string(),
                content: "bang".to_string(),
                span: None,
                attrs: vec![AttrPair {
                    key: "patching_rect".to_string(),
                    value: flutmax_ast::AttrValue::Float(100.0),
                }],
            }],
            out_assignments: vec![],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();

        let msg = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "message")
            .expect("should have a message node");

        assert_eq!(msg.attrs.len(), 1);
        assert_eq!(
            msg.attrs[0],
            ("patching_rect".to_string(), "100.".to_string())
        );
    }

    #[test]
    fn test_wire_no_attrs_empty() {
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![Wire {
                name: "osc".to_string(),
                value: Expr::Call {
                    object: "cycle~".to_string(),
                    args: vec![CallArg::positional(Expr::Lit(LitValue::Int(440)))],
                },
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
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();

        let osc = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "cycle~")
            .expect("should have a cycle~ node");

        assert!(osc.attrs.is_empty());
    }

    // ================================================
    // infer_num_outlets / infer_num_inlets unit tests
    // ================================================

    #[test]
    fn test_infer_outlets_select_single_arg() {
        // select 0 → 2 outlets (1 match + 1 unmatched)
        assert_eq!(infer_num_outlets("select", &["0".to_string()], None), 2);
    }

    #[test]
    fn test_infer_outlets_select_multiple_args() {
        // select 1 2 3 → 4 outlets (3 matches + 1 unmatched)
        assert_eq!(
            infer_num_outlets(
                "select",
                &["1".to_string(), "2".to_string(), "3".to_string()],
                None
            ),
            4
        );
    }

    #[test]
    fn test_infer_outlets_sel_alias() {
        // sel 0 → 2 outlets (same as select)
        assert_eq!(infer_num_outlets("sel", &["0".to_string()], None), 2);
    }

    #[test]
    fn test_infer_outlets_select_no_args() {
        // select with no args → default 2
        assert_eq!(infer_num_outlets("select", &[], None), 2);
    }

    #[test]
    fn test_infer_outlets_trigger_two_args() {
        // trigger b f → 2 outlets
        assert_eq!(
            infer_num_outlets("trigger", &["b".to_string(), "f".to_string()], None),
            2
        );
    }

    #[test]
    fn test_infer_outlets_trigger_alias() {
        // t b i f → 3 outlets
        assert_eq!(
            infer_num_outlets(
                "t",
                &["b".to_string(), "i".to_string(), "f".to_string()],
                None
            ),
            3
        );
    }

    #[test]
    fn test_infer_outlets_function() {
        // function → 2 outlets (list output + bang)
        assert_eq!(infer_num_outlets("function", &[], None), 2);
    }

    #[test]
    fn test_infer_outlets_route() {
        // route a b c → 4 outlets (3 matches + 1 unmatched)
        assert_eq!(
            infer_num_outlets(
                "route",
                &["a".to_string(), "b".to_string(), "c".to_string()],
                None
            ),
            4
        );
    }

    #[test]
    fn test_infer_outlets_gate() {
        // gate 3 → 3 outlets
        assert_eq!(infer_num_outlets("gate", &["3".to_string()], None), 3);
    }

    #[test]
    fn test_infer_outlets_gate_default() {
        // gate with no args → 2
        assert_eq!(infer_num_outlets("gate", &[], None), 2);
    }

    #[test]
    fn test_infer_outlets_unpack_with_args() {
        // unpack f f f → 3 outlets
        assert_eq!(
            infer_num_outlets(
                "unpack",
                &["f".to_string(), "f".to_string(), "f".to_string()],
                None
            ),
            3
        );
    }

    #[test]
    fn test_infer_outlets_unpack_no_args() {
        // unpack with no args → default 2
        assert_eq!(infer_num_outlets("unpack", &[], None), 2);
    }

    #[test]
    fn test_infer_outlets_pack() {
        // pack always → 1 outlet
        assert_eq!(
            infer_num_outlets("pack", &["0".to_string(), "0".to_string()], None),
            1
        );
    }

    #[test]
    fn test_infer_outlets_fixed_objects() {
        // Verify expanded fixed-outlet table
        assert_eq!(infer_num_outlets("line", &[], None), 2);
        assert_eq!(infer_num_outlets("makenote", &[], None), 2);
        assert_eq!(infer_num_outlets("borax", &[], None), 8);
        assert_eq!(infer_num_outlets("counter", &[], None), 4);
        assert_eq!(infer_num_outlets("notein", &[], None), 3);
        assert_eq!(infer_num_outlets("noteout", &[], None), 0);
        assert_eq!(infer_num_outlets("ctlin", &[], None), 3);
        assert_eq!(infer_num_outlets("ctlout", &[], None), 0);
        assert_eq!(infer_num_outlets("midiin", &[], None), 1);
        assert_eq!(infer_num_outlets("midiout", &[], None), 0);
        assert_eq!(infer_num_outlets("coll", &[], None), 4);
        assert_eq!(infer_num_outlets("urn", &[], None), 2);
        assert_eq!(infer_num_outlets("drunk", &[], None), 1);
        assert_eq!(infer_num_outlets("random", &[], None), 1);
        assert_eq!(infer_num_outlets("match", &[], None), 2);
        assert_eq!(infer_num_outlets("zl", &[], None), 2);
        assert_eq!(infer_num_outlets("regexp", &[], None), 5);
        assert_eq!(infer_num_outlets("sprintf", &[], None), 1);
        assert_eq!(infer_num_outlets("thresh", &[], None), 2);
        assert_eq!(infer_num_outlets("metro", &[], None), 1);
        assert_eq!(infer_num_outlets("delay", &[], None), 1);
        assert_eq!(infer_num_outlets("speedlim", &[], None), 1);
    }

    #[test]
    fn test_infer_outlets_signal_objects() {
        assert_eq!(infer_num_outlets("dspstate~", &[], None), 4);
        assert_eq!(infer_num_outlets("edge~", &[], None), 2);
        assert_eq!(infer_num_outlets("fftinfo~", &[], None), 4);
        assert_eq!(infer_num_outlets("fftin~", &[], None), 3);
        assert_eq!(infer_num_outlets("fftout~", &[], None), 1);
        assert_eq!(infer_num_outlets("cartopol~", &[], None), 2);
        assert_eq!(infer_num_outlets("poltocar~", &[], None), 2);
        assert_eq!(infer_num_outlets("freqshift~", &[], None), 2);
        assert_eq!(infer_num_outlets("curve~", &[], None), 2);
        assert_eq!(infer_num_outlets("adsr~", &[], None), 4);
        assert_eq!(infer_num_outlets("filtercoeff~", &[], None), 5);
        assert_eq!(infer_num_outlets("filtergraph~", &[], None), 7);
        assert_eq!(infer_num_outlets("noise~", &[], None), 1);
        assert_eq!(infer_num_outlets("phasor~", &[], None), 1);
        assert_eq!(infer_num_outlets("snapshot~", &[], None), 1);
        assert_eq!(infer_num_outlets("peakamp~", &[], None), 1);
        assert_eq!(infer_num_outlets("meter~", &[], None), 1);
    }

    #[test]
    fn test_infer_inlets_expanded() {
        // Verify expanded inlet table
        assert_eq!(infer_num_inlets("function", &[], None), 2);
        assert_eq!(infer_num_inlets("counter", &[], None), 3);
        assert_eq!(infer_num_inlets("makenote", &[], None), 3);
        assert_eq!(infer_num_inlets("line", &[], None), 2);
        assert_eq!(infer_num_inlets("metro", &[], None), 2);
        assert_eq!(infer_num_inlets("delay", &[], None), 2);
        assert_eq!(infer_num_inlets("coll", &[], None), 1);
        assert_eq!(infer_num_inlets("urn", &[], None), 2);
        assert_eq!(infer_num_inlets("drunk", &[], None), 2);
        assert_eq!(infer_num_inlets("random", &[], None), 2);
    }

    /// Integration test: select with literal arg produces correct outlet count in graph
    #[test]
    fn test_graph_select_outlet_count() {
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![Wire {
                name: "s".to_string(),
                value: Expr::Call {
                    object: "select".to_string(),
                    args: vec![CallArg::positional(Expr::Lit(LitValue::Int(0)))],
                },
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
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();
        let sel = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "select")
            .expect("should have a select node");

        // select 0 → 2 outlets
        assert_eq!(sel.num_outlets, 2);
    }

    /// Integration test: function produces correct outlet count in graph
    #[test]
    fn test_graph_function_outlet_count() {
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![Wire {
                name: "f".to_string(),
                value: Expr::Call {
                    object: "function".to_string(),
                    args: vec![],
                },
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
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();
        let func = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "function")
            .expect("should have a function node");

        assert_eq!(func.num_outlets, 2);
    }

    /// Integration test: trigger with multiple args
    #[test]
    fn test_graph_trigger_outlet_count() {
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![Wire {
                name: "tr".to_string(),
                value: Expr::Call {
                    object: "trigger".to_string(),
                    args: vec![
                        CallArg::positional(Expr::Lit(LitValue::Str("b".to_string()))),
                        CallArg::positional(Expr::Lit(LitValue::Str("f".to_string()))),
                    ],
                },
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
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();
        let t = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "trigger")
            .expect("should have a trigger node");

        // trigger b f → 2 outlets
        assert_eq!(t.num_outlets, 2);
    }

    /// Integration test: route with multiple args
    #[test]
    fn test_graph_route_outlet_count() {
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![Wire {
                name: "r".to_string(),
                value: Expr::Call {
                    object: "route".to_string(),
                    args: vec![
                        CallArg::positional(Expr::Lit(LitValue::Str("a".to_string()))),
                        CallArg::positional(Expr::Lit(LitValue::Str("b".to_string()))),
                        CallArg::positional(Expr::Lit(LitValue::Str("c".to_string()))),
                    ],
                },
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
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();
        let r = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "route")
            .expect("should have a route node");

        // route a b c → 4 outlets
        assert_eq!(r.num_outlets, 4);
    }

    #[test]
    fn test_codebox_with_code_files() {
        let mut code_files = CodeFiles::new();
        code_files.insert(
            "processor.js".to_string(),
            "function bang() { outlet(0, 42); }".to_string(),
        );

        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![Wire {
                name: "cb".to_string(),
                value: Expr::Call {
                    object: "v8.codebox".to_string(),
                    args: vec![CallArg::positional(Expr::Lit(LitValue::Str(
                        "processor.js".to_string(),
                    )))],
                },
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
            state_assignments: vec![],
        };

        let graph = build_graph_with_code_files(&prog, None, Some(&code_files)).unwrap();

        let cb_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "v8.codebox")
            .expect("should have a v8.codebox node");

        assert_eq!(
            cb_node.code,
            Some("function bang() { outlet(0, 42); }".to_string())
        );
        assert!(
            cb_node.args.is_empty(),
            "args should be cleared when code file is resolved"
        );
    }

    #[test]
    fn test_codebox_without_code_files() {
        // When no code_files provided, codebox still works but code is None
        let prog = Program {
            in_decls: vec![],
            out_decls: vec![],
            wires: vec![Wire {
                name: "cb".to_string(),
                value: Expr::Call {
                    object: "v8.codebox".to_string(),
                    args: vec![CallArg::positional(Expr::Lit(LitValue::Str(
                        "processor.js".to_string(),
                    )))],
                },
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
            state_assignments: vec![],
        };

        let graph = build_graph(&prog).unwrap();

        let cb_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "v8.codebox")
            .expect("should have a v8.codebox node");

        assert_eq!(cb_node.code, None);
        assert_eq!(cb_node.args, vec!["processor.js"]);
    }

    #[test]
    fn test_codebox_infer_inlets_outlets() {
        // v8.codebox and codebox should have default 1 inlet and 1 outlet
        assert_eq!(infer_num_inlets("v8.codebox", &[], None), 1);
        assert_eq!(infer_num_inlets("codebox", &[], None), 1);
        assert_eq!(infer_num_outlets("v8.codebox", &[], None), 1);
        assert_eq!(infer_num_outlets("codebox", &[], None), 1);
    }

    #[test]
    fn test_infer_codebox_ports_basic() {
        // Simple: out1 = in1 * in2
        assert_eq!(infer_codebox_ports("out1 = in1 * in2;"), (2, 1));
    }

    #[test]
    fn test_infer_codebox_ports_multiple_outputs() {
        let code = "out1 = in1 * in2;\nout2 = in1 + in2;\nout3 = in1 - in2;";
        assert_eq!(infer_codebox_ports(code), (2, 3));
    }

    #[test]
    fn test_infer_codebox_ports_history() {
        // Real gen~ code with History, multiple ins/outs
        let code = "History hold(0), gate(0);\nout1 = in1 * in2 * in3;\nout2 = in4;";
        assert_eq!(infer_codebox_ports(code), (4, 2));
    }

    #[test]
    fn test_infer_codebox_ports_no_refs() {
        // No in/out references → defaults to (1, 1)
        assert_eq!(infer_codebox_ports("x = 42;"), (1, 1));
    }

    #[test]
    fn test_infer_codebox_ports_word_boundary() {
        // "into" and "output" should NOT match
        let code = "into = 5;\noutput = into + 1;\nout1 = in1;";
        assert_eq!(infer_codebox_ports(code), (1, 1));
    }

    // ================================================
    // ObjectDb integration tests
    // ================================================

    /// Registered objects return inlet/outlet counts from objdb
    #[test]
    fn test_infer_with_objdb() {
        use flutmax_objdb::{
            InletSpec, Module, ObjectDb, ObjectDef, OutletSpec, PortDef, PortType as ObjPortType,
        };

        let mut db = ObjectDb::new();
        db.insert(ObjectDef {
            name: "myobj~".to_string(),
            module: Module::Msp,
            category: "test".to_string(),
            digest: "test object".to_string(),
            inlets: InletSpec::Fixed(vec![
                PortDef {
                    id: 0,
                    port_type: ObjPortType::Signal,
                    is_hot: true,
                    description: "in 0".to_string(),
                },
                PortDef {
                    id: 1,
                    port_type: ObjPortType::Signal,
                    is_hot: false,
                    description: "in 1".to_string(),
                },
                PortDef {
                    id: 2,
                    port_type: ObjPortType::Float,
                    is_hot: false,
                    description: "in 2".to_string(),
                },
            ]),
            outlets: OutletSpec::Fixed(vec![
                PortDef {
                    id: 0,
                    port_type: ObjPortType::Signal,
                    is_hot: false,
                    description: "out 0".to_string(),
                },
                PortDef {
                    id: 1,
                    port_type: ObjPortType::Signal,
                    is_hot: false,
                    description: "out 1".to_string(),
                },
            ]),
            args: vec![],
        });

        // 3 inlets, 2 outlets returned from objdb
        assert_eq!(infer_num_inlets("myobj~", &[], Some(&db)), 3);
        assert_eq!(infer_num_outlets("myobj~", &[], Some(&db)), 2);
    }

    /// Unregistered objects fall back to hardcoded table
    #[test]
    fn test_infer_objdb_fallback() {
        use flutmax_objdb::ObjectDb;

        let db = ObjectDb::new(); // empty db

        // "cycle~" is not in objdb -> 2 inlets, 1 outlet from hardcoded table
        assert_eq!(infer_num_inlets("cycle~", &[], Some(&db)), 2);
        assert_eq!(infer_num_outlets("cycle~", &[], Some(&db)), 1);

        // "counter" also uses hardcoded fallback
        assert_eq!(infer_num_inlets("counter", &[], Some(&db)), 3);
        assert_eq!(infer_num_outlets("counter", &[], Some(&db)), 4);
    }

    /// objdb Variable inlet/outlet works correctly with default arguments
    #[test]
    fn test_infer_objdb_variable_ports() {
        use flutmax_objdb::{
            InletSpec, Module, ObjectDb, ObjectDef, OutletSpec, PortDef, PortType as ObjPortType,
        };

        let mut db = ObjectDb::new();
        db.insert(ObjectDef {
            name: "varobj".to_string(),
            module: Module::Max,
            category: "test".to_string(),
            digest: "variable port object".to_string(),
            inlets: InletSpec::Variable {
                defaults: vec![
                    PortDef {
                        id: 0,
                        port_type: ObjPortType::Any,
                        is_hot: true,
                        description: "in 0".to_string(),
                    },
                    PortDef {
                        id: 1,
                        port_type: ObjPortType::Any,
                        is_hot: false,
                        description: "in 1".to_string(),
                    },
                ],
                min_inlets: 1,
            },
            outlets: OutletSpec::Variable {
                defaults: vec![
                    PortDef {
                        id: 0,
                        port_type: ObjPortType::Any,
                        is_hot: false,
                        description: "out 0".to_string(),
                    },
                    PortDef {
                        id: 1,
                        port_type: ObjPortType::Any,
                        is_hot: false,
                        description: "out 1".to_string(),
                    },
                    PortDef {
                        id: 2,
                        port_type: ObjPortType::Any,
                        is_hot: false,
                        description: "out 2".to_string(),
                    },
                ],
                min_outlets: 1,
            },
            args: vec![],
        });

        // No arguments -> returns defaults.len()
        assert_eq!(infer_num_inlets("varobj", &[], Some(&db)), 2);
        assert_eq!(infer_num_outlets("varobj", &[], Some(&db)), 3);

        // With arguments -> returns args.len()
        assert_eq!(
            infer_num_inlets(
                "varobj",
                &["a".to_string(), "b".to_string(), "c".to_string()],
                Some(&db)
            ),
            3
        );
        assert_eq!(
            infer_num_outlets("varobj", &["x".to_string(), "y".to_string()], Some(&db)),
            2
        );
    }

    // ── E52: OutDecl with inline value ──────────────────────

    #[test]
    fn test_out_decl_inline_value_produces_edge() {
        // out audio: signal = osc; should produce the same graph as
        // out audio: signal; + out[0] = osc;
        let inline_program = Program {
            in_decls: vec![],
            out_decls: vec![OutDecl {
                index: 0,
                name: "audio".to_string(),
                port_type: PortType::Signal,
                value: Some(Expr::Ref("osc".to_string())),
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
            destructuring_wires: vec![],
            msg_decls: vec![],
            out_assignments: vec![],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let separate_program = Program {
            in_decls: vec![],
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
                    args: vec![CallArg::positional(Expr::Lit(LitValue::Int(440)))],
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
        };

        let inline_graph = build_graph(&inline_program).expect("inline build failed");
        let separate_graph = build_graph(&separate_program).expect("separate build failed");

        // Both should have same number of nodes and edges
        assert_eq!(
            inline_graph.nodes.len(),
            separate_graph.nodes.len(),
            "node count mismatch: inline={} vs separate={}",
            inline_graph.nodes.len(),
            separate_graph.nodes.len()
        );
        assert_eq!(
            inline_graph.edges.len(),
            separate_graph.edges.len(),
            "edge count mismatch: inline={} vs separate={}",
            inline_graph.edges.len(),
            separate_graph.edges.len()
        );
    }

    // ── Named argument resolution tests ─────────────────────────

    #[test]
    fn test_resolve_inlet_name_found() {
        use flutmax_objdb::{
            InletSpec, Module, ObjectDb, ObjectDef, OutletSpec, PortDef, PortType as ObjPortType,
        };

        let mut db = ObjectDb::new();
        db.insert(ObjectDef {
            name: "cycle~".to_string(),
            module: Module::Msp,
            category: String::new(),
            digest: String::new(),
            inlets: InletSpec::Fixed(vec![
                PortDef {
                    id: 0,
                    port_type: ObjPortType::SignalFloat,
                    is_hot: true,
                    description: "Frequency".to_string(),
                },
                PortDef {
                    id: 1,
                    port_type: ObjPortType::SignalFloat,
                    is_hot: false,
                    description: "Phase offset".to_string(),
                },
            ]),
            outlets: OutletSpec::Fixed(vec![]),
            args: vec![],
        });

        assert_eq!(
            resolve_inlet_name("cycle~", "frequency", Some(&db)),
            Some(0)
        );
        assert_eq!(
            resolve_inlet_name("cycle~", "phase_offset", Some(&db)),
            Some(1)
        );
    }

    #[test]
    fn test_resolve_inlet_name_not_found() {
        use flutmax_objdb::{
            InletSpec, Module, ObjectDb, ObjectDef, OutletSpec, PortDef, PortType as ObjPortType,
        };

        let mut db = ObjectDb::new();
        db.insert(ObjectDef {
            name: "cycle~".to_string(),
            module: Module::Msp,
            category: String::new(),
            digest: String::new(),
            inlets: InletSpec::Fixed(vec![PortDef {
                id: 0,
                port_type: ObjPortType::SignalFloat,
                is_hot: true,
                description: "Frequency".to_string(),
            }]),
            outlets: OutletSpec::Fixed(vec![]),
            args: vec![],
        });

        assert_eq!(resolve_inlet_name("cycle~", "nonexistent", Some(&db)), None);
    }

    #[test]
    fn test_resolve_inlet_name_no_objdb() {
        assert_eq!(resolve_inlet_name("cycle~", "frequency", None), None);
    }

    #[test]
    fn test_resolve_abstraction_inlet_name() {
        use flutmax_ast::PortType;
        use flutmax_sema::registry::{AbstractionInterface, AbstractionRegistry, PortInfo};

        let mut reg = AbstractionRegistry::new();
        reg.register_interface(AbstractionInterface {
            name: "simpleFM".to_string(),
            in_ports: vec![
                PortInfo {
                    index: 0,
                    name: "carrier_freq".to_string(),
                    port_type: PortType::Float,
                },
                PortInfo {
                    index: 1,
                    name: "harmonicity".to_string(),
                    port_type: PortType::Float,
                },
                PortInfo {
                    index: 2,
                    name: "mod_index".to_string(),
                    port_type: PortType::Float,
                },
            ],
            out_ports: vec![PortInfo {
                index: 0,
                name: "output".to_string(),
                port_type: PortType::Signal,
            }],
        });

        assert_eq!(
            resolve_abstraction_inlet_name("simpleFM", "carrier_freq", Some(&reg)),
            Some(0)
        );
        assert_eq!(
            resolve_abstraction_inlet_name("simpleFM", "harmonicity", Some(&reg)),
            Some(1)
        );
        assert_eq!(
            resolve_abstraction_inlet_name("simpleFM", "mod_index", Some(&reg)),
            Some(2)
        );
        assert_eq!(
            resolve_abstraction_inlet_name("simpleFM", "nonexistent", Some(&reg)),
            None
        );
        assert_eq!(
            resolve_abstraction_inlet_name("unknown", "carrier_freq", Some(&reg)),
            None
        );
        assert_eq!(
            resolve_abstraction_inlet_name("simpleFM", "carrier_freq", None),
            None
        );
    }

    #[test]
    fn test_named_arg_codegen() {
        // Named args should resolve to correct inlet indices
        use flutmax_objdb::{
            InletSpec, Module, ObjectDb, ObjectDef, OutletSpec, PortDef, PortType as ObjPortType,
        };

        let mut db = ObjectDb::new();
        db.insert(ObjectDef {
            name: "biquad~".to_string(),
            module: Module::Msp,
            category: String::new(),
            digest: String::new(),
            inlets: InletSpec::Fixed(vec![
                PortDef {
                    id: 0,
                    port_type: ObjPortType::Signal,
                    is_hot: true,
                    description: "Input".to_string(),
                },
                PortDef {
                    id: 1,
                    port_type: ObjPortType::SignalFloat,
                    is_hot: false,
                    description: "Frequency".to_string(),
                },
                PortDef {
                    id: 2,
                    port_type: ObjPortType::SignalFloat,
                    is_hot: false,
                    description: "Q factor".to_string(),
                },
            ]),
            outlets: OutletSpec::Fixed(vec![PortDef {
                id: 0,
                port_type: ObjPortType::Signal,
                is_hot: false,
                description: "Output".to_string(),
            }]),
            args: vec![],
        });

        // Build a program with named args
        let program = Program {
            in_decls: vec![
                InDecl {
                    index: 0,
                    name: "sig".to_string(),
                    port_type: PortType::Signal,
                },
                InDecl {
                    index: 1,
                    name: "freq".to_string(),
                    port_type: PortType::Float,
                },
            ],
            out_decls: vec![OutDecl {
                index: 0,
                name: "out".to_string(),
                port_type: PortType::Signal,
                value: None,
            }],
            wires: vec![Wire {
                name: "filtered".to_string(),
                value: Expr::Call {
                    object: "biquad~".to_string(),
                    args: vec![
                        // Use named args: "frequency" maps to inlet 1
                        CallArg::named("frequency", Expr::Ref("freq".to_string())),
                        // "input" maps to inlet 0
                        CallArg::named("input", Expr::Ref("sig".to_string())),
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
            destructuring_wires: vec![],
            msg_decls: vec![],
            direct_connections: vec![],
            feedback_decls: vec![],
            feedback_assignments: vec![],
            state_decls: vec![],
            state_assignments: vec![],
        };

        let graph =
            build_graph_with_objdb(&program, None, None, Some(&db)).expect("should build graph");

        // Verify that the named args resolved to the correct inlet indices
        // "frequency" → inlet 1, "input" → inlet 0
        // Find the biquad~ node ID
        let biquad_node = graph
            .nodes
            .iter()
            .find(|n| n.object_name == "biquad~")
            .expect("should have biquad~ node");
        let biquad_id = &biquad_node.id;

        let biquad_edges: Vec<_> = graph
            .edges
            .iter()
            .filter(|e| &e.dest_id == biquad_id)
            .collect();

        // Should have 2 edges going into biquad~
        assert_eq!(
            biquad_edges.len(),
            2,
            "expected 2 edges to biquad~, got {}: {:?}",
            biquad_edges.len(),
            biquad_edges
        );

        // Check inlet assignments: freq→inlet 1, sig→inlet 0
        let freq_edge = biquad_edges.iter().find(|e| e.dest_inlet == 1);
        let sig_edge = biquad_edges.iter().find(|e| e.dest_inlet == 0);
        assert!(
            freq_edge.is_some(),
            "should have edge to inlet 1 (frequency)"
        );
        assert!(sig_edge.is_some(), "should have edge to inlet 0 (input)");
    }
}
