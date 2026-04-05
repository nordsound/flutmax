use std::path::Path;

use quick_xml::de::from_str;
use serde::Deserialize;

use crate::{ArgDef, InletSpec, Module, ObjectDb, ObjectDef, OutletSpec, PortDef, PortType};

/// XML parse error
#[derive(Debug)]
pub enum ParseError {
    Xml(quick_xml::DeError),
    Io(std::io::Error),
    /// c74object element is missing the name attribute
    MissingName,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Xml(e) => write!(f, "XML parse error: {}", e),
            ParseError::Io(e) => write!(f, "IO error: {}", e),
            ParseError::MissingName => write!(f, "Missing 'name' attribute on c74object"),
        }
    }
}

impl std::error::Error for ParseError {}

impl From<quick_xml::DeError> for ParseError {
    fn from(e: quick_xml::DeError) -> Self {
        ParseError::Xml(e)
    }
}

impl From<std::io::Error> for ParseError {
    fn from(e: std::io::Error) -> Self {
        ParseError::Io(e)
    }
}

// ---- XML serde structs ----

/// c74object root element
#[derive(Debug, Deserialize)]
#[serde(rename = "c74object")]
struct XmlC74Object {
    #[serde(rename = "@name")]
    name: Option<String>,
    #[serde(rename = "@module")]
    module: Option<String>,
    #[serde(rename = "@category")]
    category: Option<String>,
    digest: Option<XmlDigest>,
    inletlist: Option<XmlInletList>,
    outletlist: Option<XmlOutletList>,
    objarglist: Option<XmlObjArgList>,
}

#[derive(Debug, Deserialize)]
struct XmlInletList {
    #[serde(rename = "inlet", default)]
    inlets: Vec<XmlInlet>,
}

#[derive(Debug, Deserialize)]
struct XmlInlet {
    #[serde(rename = "@id")]
    id: Option<u32>,
    #[serde(rename = "@type")]
    inlet_type: Option<String>,
    digest: Option<XmlDigest>,
}

#[derive(Debug, Deserialize)]
struct XmlOutletList {
    #[serde(rename = "outlet", default)]
    outlets: Vec<XmlOutlet>,
}

#[derive(Debug, Deserialize)]
struct XmlOutlet {
    #[serde(rename = "@id")]
    id: Option<u32>,
    #[serde(rename = "@type")]
    outlet_type: Option<String>,
    digest: Option<XmlDigest>,
}

#[derive(Debug, Deserialize)]
struct XmlObjArgList {
    #[serde(rename = "objarg", default)]
    args: Vec<XmlObjArg>,
}

#[derive(Debug, Deserialize)]
struct XmlObjArg {
    #[serde(rename = "@name")]
    name: Option<String>,
    #[serde(rename = "@optional")]
    optional: Option<String>,
    #[serde(rename = "@type")]
    arg_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct XmlDigest {
    #[serde(rename = "$text")]
    text: Option<String>,
}

// ---- Conversion logic ----

/// Heuristic to determine whether inlet/outlet count is variable (depends on argument count)
fn has_variable_ports(args: &[XmlObjArg], ports: &[XmlInlet]) -> bool {
    // If a port has INLET_TYPE, it's likely variable
    let has_dynamic_type = ports
        .iter()
        .any(|p| matches!(p.inlet_type.as_deref(), Some("INLET_TYPE")));

    // Check for descriptions like "number of inlets is determined" in objarg
    // Since description parsing with serde is omitted, only type-based heuristics are used
    if has_dynamic_type && !args.is_empty() {
        return true;
    }

    false
}

fn has_variable_outlet_ports(args: &[XmlObjArg], ports: &[XmlOutlet]) -> bool {
    let has_dynamic_type = ports
        .iter()
        .any(|p| matches!(p.outlet_type.as_deref(), Some("OUTLET_TYPE")));

    if has_dynamic_type && !args.is_empty() {
        return true;
    }

    false
}

fn convert_inlet(inlet: &XmlInlet, index: usize) -> PortDef {
    let type_str = inlet.inlet_type.as_deref().unwrap_or("");
    let digest_text = inlet
        .digest
        .as_ref()
        .and_then(|d| d.text.as_deref())
        .unwrap_or("")
        .trim()
        .to_string();

    PortDef {
        id: inlet.id.unwrap_or(index as u32),
        port_type: PortType::from_xml_type(type_str),
        is_hot: inlet.id.unwrap_or(index as u32) == 0,
        description: digest_text,
    }
}

fn convert_outlet(outlet: &XmlOutlet, index: usize) -> PortDef {
    let type_str = outlet.outlet_type.as_deref().unwrap_or("");
    let digest_text = outlet
        .digest
        .as_ref()
        .and_then(|d| d.text.as_deref())
        .unwrap_or("")
        .trim()
        .to_string();

    PortDef {
        id: outlet.id.unwrap_or(index as u32),
        port_type: PortType::from_xml_type(type_str),
        is_hot: false,
        description: digest_text,
    }
}

fn convert_arg(arg: &XmlObjArg) -> ArgDef {
    ArgDef {
        name: arg.name.clone().unwrap_or_default(),
        arg_type: arg.arg_type.clone().unwrap_or_default(),
        optional: arg.optional.as_deref() == Some("1"),
    }
}

/// Parse a .maxref.xml content string and return an ObjectDef
pub fn parse_maxref(xml_content: &str) -> Result<ObjectDef, ParseError> {
    let obj: XmlC74Object = from_str(xml_content)?;

    let name = obj.name.ok_or(ParseError::MissingName)?;
    let module = Module::parse(obj.module.as_deref().unwrap_or("max"));
    let category = obj.category.unwrap_or_default();
    let digest = obj
        .digest
        .and_then(|d| d.text)
        .unwrap_or_default()
        .trim()
        .to_string();

    let xml_inlets = obj.inletlist.map(|il| il.inlets).unwrap_or_default();
    let xml_outlets = obj.outletlist.map(|ol| ol.outlets).unwrap_or_default();
    let xml_args = obj.objarglist.map(|al| al.args).unwrap_or_default();

    let inlet_defs: Vec<PortDef> = xml_inlets
        .iter()
        .enumerate()
        .map(|(i, inlet)| convert_inlet(inlet, i))
        .collect();

    let outlet_defs: Vec<PortDef> = xml_outlets
        .iter()
        .enumerate()
        .map(|(i, outlet)| convert_outlet(outlet, i))
        .collect();

    let args: Vec<ArgDef> = xml_args.iter().map(convert_arg).collect();

    let inlets = if has_variable_ports(&xml_args, &xml_inlets) {
        InletSpec::Variable {
            min_inlets: if inlet_defs.is_empty() { 0 } else { 1 },
            defaults: inlet_defs,
        }
    } else {
        InletSpec::Fixed(inlet_defs)
    };

    let outlets = if has_variable_outlet_ports(&xml_args, &xml_outlets) {
        OutletSpec::Variable {
            min_outlets: if outlet_defs.is_empty() { 0 } else { 1 },
            defaults: outlet_defs,
        }
    } else {
        OutletSpec::Fixed(outlet_defs)
    };

    Ok(ObjectDef {
        name,
        module,
        category,
        digest,
        inlets,
        outlets,
        args,
    })
}

/// Parse all .maxref.xml files in a directory and return an ObjectDb.
/// Files that fail to parse are skipped (error count is returned).
pub fn load_directory(dir: &Path) -> Result<(ObjectDb, usize), ParseError> {
    let mut db = ObjectDb::new();
    let mut error_count = 0;

    if !dir.is_dir() {
        return Err(ParseError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Directory not found: {:?}", dir),
        )));
    }

    let entries = std::fs::read_dir(dir)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("xml") {
            continue;
        }

        let file_name = path.file_name().and_then(|f| f.to_str()).unwrap_or("");

        if !file_name.ends_with(".maxref.xml") {
            continue;
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => {
                error_count += 1;
                continue;
            }
        };

        match parse_maxref(&content) {
            Ok(def) => {
                db.insert(def);
            }
            Err(_) => {
                error_count += 1;
            }
        }
    }

    Ok((db, error_count))
}

/// Load all .maxref.xml files recursively from a directory tree.
///
/// Useful for Package refpages that have subdirectories
/// (e.g., `packages/Gen/docs/refpages1/common/`, `packages/RNBO/docs/refpages/rnbo/`).
pub fn load_directory_recursive(dir: &Path) -> Result<(ObjectDb, usize), ParseError> {
    let mut db = ObjectDb::new();
    let mut error_count = 0;
    load_recursive_inner(dir, &mut db, &mut error_count)?;
    Ok((db, error_count))
}

fn load_recursive_inner(
    dir: &Path,
    db: &mut ObjectDb,
    error_count: &mut usize,
) -> Result<(), ParseError> {
    if !dir.is_dir() {
        return Ok(());
    }

    let entries = std::fs::read_dir(dir).map_err(ParseError::Io)?;
    for entry in entries {
        let entry = entry.map_err(ParseError::Io)?;
        let path = entry.path();

        if path.is_dir() {
            load_recursive_inner(&path, db, error_count)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("xml") {
            let file_name = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
            if !file_name.ends_with(".maxref.xml") {
                continue;
            }

            match std::fs::read_to_string(&path) {
                Ok(content) => match parse_maxref(&content) {
                    Ok(def) => {
                        db.insert(def);
                    }
                    Err(_) => {
                        *error_count += 1;
                    }
                },
                Err(_) => {
                    *error_count += 1;
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CYCLE_XML: &str = r#"<?xml version="1.0" encoding="utf-8" standalone="yes"?>
<c74object name="cycle~" module="msp" category="MSP Synthesis">
    <digest>Sinusoidal oscillator</digest>
    <description>Use the cycle~ object to generate a periodic waveform.</description>
    <inletlist>
        <inlet id="0" type="signal/float">
            <digest>Frequency</digest>
            <description>TEXT_HERE</description>
        </inlet>
        <inlet id="1" type="signal/float">
            <digest>Phase (0-1)</digest>
            <description>TEXT_HERE</description>
        </inlet>
    </inletlist>
    <outletlist>
        <outlet id="0" type="signal">
            <digest>Output</digest>
            <description>TEXT_HERE</description>
        </outlet>
    </outletlist>
    <objarglist>
        <objarg name="frequency" optional="1" units="hz" type="number">
            <digest>Oscillator frequency (initial)</digest>
        </objarg>
        <objarg name="buffer-name" optional="1" type="symbol">
            <digest>Buffer name</digest>
        </objarg>
    </objarglist>
</c74object>"#;

    const BIQUAD_XML: &str = r#"<?xml version="1.0" encoding="utf-8" standalone="yes"?>
<c74object name="biquad~" module="msp" category="MSP Filters">
    <digest>Two-pole, two-zero filter</digest>
    <description>biquad~ implements a two-pole, two-zero filter.</description>
    <inletlist>
        <inlet id="0" type="signal">
            <digest>Input</digest>
            <description>TEXT_HERE</description>
        </inlet>
        <inlet id="1" type="signal/float">
            <digest>Input Gain (Filter coefficient a0)</digest>
            <description>TEXT_HERE</description>
        </inlet>
        <inlet id="2" type="signal/float">
            <digest>Filter coefficient a1</digest>
            <description>TEXT_HERE</description>
        </inlet>
        <inlet id="3" type="signal/float">
            <digest>Filter coefficient a2</digest>
            <description>TEXT_HERE</description>
        </inlet>
        <inlet id="4" type="signal/float">
            <digest>Filter coefficient b1</digest>
            <description>TEXT_HERE</description>
        </inlet>
        <inlet id="5" type="signal/float">
            <digest>Filter coefficient b2</digest>
            <description>TEXT_HERE</description>
        </inlet>
    </inletlist>
    <outletlist>
        <outlet id="0" type="signal">
            <digest>Output</digest>
            <description>TEXT_HERE</description>
        </outlet>
    </outletlist>
    <objarglist>
        <objarg name="a0" optional="0" type="float">
            <digest>a0 coefficient initial value</digest>
        </objarg>
        <objarg name="a1" optional="0" type="float">
            <digest>a1 coefficient initial value</digest>
        </objarg>
        <objarg name="a2" optional="0" type="float">
            <digest>a2 coefficient initial value</digest>
        </objarg>
        <objarg name="b1" optional="0" type="float">
            <digest>b1 coefficient initial value</digest>
        </objarg>
        <objarg name="b2" optional="0" type="float">
            <digest>b2 coefficient initial value</digest>
        </objarg>
    </objarglist>
</c74object>"#;

    const TRIGGER_XML: &str = r#"<?xml version="1.0" encoding="utf-8" standalone="yes"?>
<c74object name="trigger" module="max" category="Control, Right-to-Left">
    <digest>Send input to many places</digest>
    <description>Outputs any input received in order from right to left.</description>
    <inletlist>
        <inlet id="0" type="INLET_TYPE">
            <digest>Message to be Fanned to Multiple Outputs</digest>
            <description>TEXT_HERE</description>
        </inlet>
    </inletlist>
    <outletlist>
        <outlet id="0" type="OUTLET_TYPE">
            <digest>Output Order 2 (int)</digest>
            <description>TEXT_HERE</description>
        </outlet>
        <outlet id="1" type="OUTLET_TYPE">
            <digest>Output Order 1 (int)</digest>
            <description>TEXT_HERE</description>
        </outlet>
    </outletlist>
    <objarglist>
        <objarg name="formats" optional="1" type="symbol">
            <digest>Output types</digest>
            <description>The number of arguments determines the number of outlets.</description>
        </objarg>
    </objarglist>
</c74object>"#;

    const PACK_XML: &str = r#"<?xml version="1.0" encoding="utf-8" standalone="yes"?>
<c74object name="pack" module="max" category="Lists">
    <digest>Create a list</digest>
    <description>Combine items into an output list.</description>
    <inletlist>
        <inlet id="0" type="INLET_TYPE">
            <digest>value for the first list element, causes output</digest>
            <description></description>
        </inlet>
        <inlet id="1" type="INLET_TYPE">
            <digest>value for the second list element</digest>
            <description></description>
        </inlet>
    </inletlist>
    <outletlist>
        <outlet id="0" type="OUTLET_TYPE">
            <digest>Output list</digest>
            <description></description>
        </outlet>
    </outletlist>
    <objarglist>
        <objarg name="list-elements" optional="1" type="any">
            <digest>List elements</digest>
            <description>The number of inlets is determined by the number of arguments.</description>
        </objarg>
    </objarglist>
</c74object>"#;

    const SELECTOR_XML: &str = r#"<?xml version="1.0" encoding="utf-8" standalone="yes"?>
<c74object name="selector~" module="msp" category="MSP Routing">
    <digest>Assign one of several inputs to an outlet</digest>
    <description>Use the selector~ object to choose between one of several input signals.</description>
    <inletlist>
        <inlet id="0" type="int/signal">
            <digest>int/signal Turns Input Off or Routes to Output</digest>
            <description>TEXT_HERE</description>
        </inlet>
        <inlet id="1" type="signal">
            <digest>(signal) Input</digest>
            <description>TEXT_HERE</description>
        </inlet>
    </inletlist>
    <outletlist>
        <outlet id="0" type="signal">
            <digest>(signal) Output</digest>
            <description>TEXT_HERE</description>
        </outlet>
    </outletlist>
    <objarglist>
        <objarg name="number-of-inputs" optional="1" type="int">
            <digest>Number of inputs</digest>
        </objarg>
        <objarg name="initially-open-inlet" optional="1" type="int">
            <digest>Initial input selected</digest>
        </objarg>
    </objarglist>
</c74object>"#;

    // ---- cycle~ tests ----

    #[test]
    fn test_parse_cycle() {
        let def = parse_maxref(CYCLE_XML).unwrap();
        assert_eq!(def.name, "cycle~");
        assert_eq!(def.module, Module::Msp);
        assert_eq!(def.category, "MSP Synthesis");
        assert_eq!(def.digest, "Sinusoidal oscillator");

        // cycle~ has fixed inlets
        assert!(!def.has_variable_inlets());
        assert_eq!(def.default_inlet_count(), 2);

        if let InletSpec::Fixed(ref inlets) = def.inlets {
            assert_eq!(inlets[0].id, 0);
            assert_eq!(inlets[0].port_type, PortType::SignalFloat);
            assert!(inlets[0].is_hot);
            assert_eq!(inlets[0].description, "Frequency");

            assert_eq!(inlets[1].id, 1);
            assert_eq!(inlets[1].port_type, PortType::SignalFloat);
            assert!(!inlets[1].is_hot);
            assert_eq!(inlets[1].description, "Phase (0-1)");
        } else {
            panic!("Expected Fixed inlets for cycle~");
        }

        // outlet
        assert!(!def.has_variable_outlets());
        assert_eq!(def.default_outlet_count(), 1);

        if let OutletSpec::Fixed(ref outlets) = def.outlets {
            assert_eq!(outlets[0].port_type, PortType::Signal);
        } else {
            panic!("Expected Fixed outlets for cycle~");
        }

        // args
        assert_eq!(def.args.len(), 2);
        assert_eq!(def.args[0].name, "frequency");
        assert!(def.args[0].optional);
    }

    // ---- biquad~ tests ----

    #[test]
    fn test_parse_biquad() {
        let def = parse_maxref(BIQUAD_XML).unwrap();
        assert_eq!(def.name, "biquad~");
        assert_eq!(def.module, Module::Msp);
        assert!(!def.has_variable_inlets());
        assert_eq!(def.default_inlet_count(), 6);

        if let InletSpec::Fixed(ref inlets) = def.inlets {
            // inlet 0 is signal only
            assert_eq!(inlets[0].port_type, PortType::Signal);
            assert!(inlets[0].is_hot);
            // inlet 1-5 are signal/float
            for i in 1..6 {
                assert_eq!(inlets[i].port_type, PortType::SignalFloat);
                assert!(!inlets[i].is_hot);
            }
        } else {
            panic!("Expected Fixed inlets for biquad~");
        }

        assert_eq!(def.default_outlet_count(), 1);
        assert_eq!(def.args.len(), 5);
        assert!(!def.args[0].optional); // biquad~ args are required
    }

    // ---- trigger tests ----

    #[test]
    fn test_parse_trigger() {
        let def = parse_maxref(TRIGGER_XML).unwrap();
        assert_eq!(def.name, "trigger");
        assert_eq!(def.module, Module::Max);

        // trigger has variable outlets
        assert!(def.has_variable_outlets());
        assert_eq!(def.default_outlet_count(), 2);

        // inlet is INLET_TYPE -> Dynamic, Variable because there are arguments
        assert!(def.has_variable_inlets());
        if let InletSpec::Variable { ref defaults, .. } = def.inlets {
            assert_eq!(defaults.len(), 1);
            assert_eq!(defaults[0].port_type, PortType::Dynamic);
            assert!(defaults[0].is_hot);
        } else {
            panic!("Expected Variable inlets for trigger");
        }

        if let OutletSpec::Variable { ref defaults, .. } = def.outlets {
            assert_eq!(defaults.len(), 2);
            assert_eq!(defaults[0].port_type, PortType::Dynamic);
            assert_eq!(defaults[1].port_type, PortType::Dynamic);
        } else {
            panic!("Expected Variable outlets for trigger");
        }
    }

    // ---- pack tests ----

    #[test]
    fn test_parse_pack() {
        let def = parse_maxref(PACK_XML).unwrap();
        assert_eq!(def.name, "pack");
        assert_eq!(def.module, Module::Max);

        // pack has variable inlets
        assert!(def.has_variable_inlets());
        assert_eq!(def.default_inlet_count(), 2);

        if let InletSpec::Variable {
            ref defaults,
            min_inlets,
        } = def.inlets
        {
            assert_eq!(min_inlets, 1);
            assert_eq!(defaults.len(), 2);
            assert_eq!(defaults[0].port_type, PortType::Dynamic);
            assert!(defaults[0].is_hot);
            assert!(!defaults[1].is_hot);
        } else {
            panic!("Expected Variable inlets for pack");
        }

        // outlet is OUTLET_TYPE + has arguments -> Variable
        assert!(def.has_variable_outlets());
    }

    // ---- selector~ tests ----

    #[test]
    fn test_parse_selector() {
        let def = parse_maxref(SELECTOR_XML).unwrap();
        assert_eq!(def.name, "selector~");
        assert_eq!(def.module, Module::Msp);
        assert_eq!(def.category, "MSP Routing");

        // selector~ has fixed inlets (not INLET_TYPE)
        assert!(!def.has_variable_inlets());
        assert_eq!(def.default_inlet_count(), 2);

        if let InletSpec::Fixed(ref inlets) = def.inlets {
            assert_eq!(inlets[0].port_type, PortType::IntSignal);
            assert!(inlets[0].is_hot);
            assert_eq!(inlets[1].port_type, PortType::Signal);
            assert!(!inlets[1].is_hot);
        } else {
            panic!("Expected Fixed inlets for selector~");
        }

        assert!(!def.has_variable_outlets());
        assert_eq!(def.default_outlet_count(), 1);
    }

    // ---- Error handling tests ----

    #[test]
    fn test_parse_missing_name() {
        let xml = r#"<?xml version="1.0"?>
<c74object module="msp">
    <digest>Test</digest>
</c74object>"#;
        let result = parse_maxref(xml);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_minimal() {
        let xml = r#"<?xml version="1.0"?>
<c74object name="test" module="max">
    <digest>Minimal test</digest>
</c74object>"#;
        let def = parse_maxref(xml).unwrap();
        assert_eq!(def.name, "test");
        assert_eq!(def.default_inlet_count(), 0);
        assert_eq!(def.default_outlet_count(), 0);
    }

    // ---- Tests using real XML files (when Max.app is installed) ----

    #[test]
    fn test_parse_real_cycle_xml() {
        let path = Path::new(
            "/Applications/Max.app/Contents/Resources/C74/docs/refpages/msp-ref/cycle~.maxref.xml",
        );
        if !path.exists() {
            eprintln!("Skipping test: Max.app not found");
            return;
        }

        let content = std::fs::read_to_string(path).unwrap();
        let def = parse_maxref(&content).unwrap();

        assert_eq!(def.name, "cycle~");
        assert_eq!(def.module, Module::Msp);
        assert_eq!(def.default_inlet_count(), 2);
        assert_eq!(def.default_outlet_count(), 1);
    }

    #[test]
    fn test_parse_real_biquad_xml() {
        let path = Path::new(
            "/Applications/Max.app/Contents/Resources/C74/docs/refpages/msp-ref/biquad~.maxref.xml",
        );
        if !path.exists() {
            eprintln!("Skipping test: Max.app not found");
            return;
        }

        let content = std::fs::read_to_string(path).unwrap();
        let def = parse_maxref(&content).unwrap();

        assert_eq!(def.name, "biquad~");
        assert_eq!(def.default_inlet_count(), 6);
    }

    #[test]
    fn test_parse_real_trigger_xml() {
        let path = Path::new(
            "/Applications/Max.app/Contents/Resources/C74/docs/refpages/max-ref/trigger.maxref.xml",
        );
        if !path.exists() {
            eprintln!("Skipping test: Max.app not found");
            return;
        }

        let content = std::fs::read_to_string(path).unwrap();
        let def = parse_maxref(&content).unwrap();

        assert_eq!(def.name, "trigger");
        assert!(def.has_variable_outlets());
    }

    #[test]
    fn test_load_msp_ref_directory() {
        let dir = Path::new("/Applications/Max.app/Contents/Resources/C74/docs/refpages/msp-ref");
        if !dir.exists() {
            eprintln!("Skipping test: Max.app not found");
            return;
        }

        let (db, errors) = load_directory(dir).unwrap();

        // msp-ref has ~455 files. Most should succeed even with some parse errors
        assert!(db.len() > 400, "Expected > 400 objects, got {}", db.len());
        assert!(errors < 60, "Too many parse errors: {}", errors);

        // Verify representative objects are included
        assert!(db.lookup("cycle~").is_some());
        assert!(db.lookup("biquad~").is_some());
        assert!(db.lookup("selector~").is_some());
    }

    #[test]
    fn test_load_max_ref_directory() {
        let dir = Path::new("/Applications/Max.app/Contents/Resources/C74/docs/refpages/max-ref");
        if !dir.exists() {
            eprintln!("Skipping test: Max.app not found");
            return;
        }

        let (db, errors) = load_directory(dir).unwrap();

        assert!(db.len() > 400, "Expected > 400 objects, got {}", db.len());
        assert!(errors < 80, "Too many parse errors: {}", errors);

        assert!(db.lookup("trigger").is_some());
        assert!(db.lookup("pack").is_some());
    }

    // ---- load_directory_recursive tests ----

    #[test]
    fn test_load_directory_recursive_on_flat_dir() {
        // load_directory_recursive should also work on a flat directory (same as load_directory)
        let dir = Path::new("/Applications/Max.app/Contents/Resources/C74/docs/refpages/msp-ref");
        if !dir.exists() {
            eprintln!("Skipping test: Max.app not found");
            return;
        }

        let (db_flat, errors_flat) = load_directory(dir).unwrap();
        let (db_recursive, errors_recursive) = load_directory_recursive(dir).unwrap();

        // Recursive should find at least as many as flat on the same directory
        assert_eq!(
            db_flat.len(),
            db_recursive.len(),
            "Flat ({}) and recursive ({}) should match on a flat directory",
            db_flat.len(),
            db_recursive.len()
        );
        assert_eq!(errors_flat, errors_recursive);
    }

    #[test]
    fn test_load_directory_recursive_finds_subdirectories() {
        // Package directories have subdirectories with refpages
        let packages_dir = Path::new("/Applications/Max.app/Contents/Resources/C74/packages");
        if !packages_dir.exists() {
            eprintln!("Skipping test: Max.app packages not found");
            return;
        }

        // Try a known package with subdirectories (e.g., Gen)
        let gen_refpages = packages_dir.join("Gen").join("docs").join("refpages1");
        if !gen_refpages.exists() {
            eprintln!("Skipping test: Gen package refpages1 not found");
            return;
        }

        let (db, _errors) = load_directory_recursive(&gen_refpages).unwrap();
        eprintln!(
            "load_directory_recursive on Gen/docs/refpages1: {} objects",
            db.len()
        );
        assert!(
            db.len() > 0,
            "Expected at least 1 object from recursive scan of Gen refpages1"
        );
    }

    #[test]
    fn test_load_directory_recursive_nonexistent_dir() {
        let dir = Path::new("/nonexistent/path/that/does/not/exist");
        let (db, error_count) = load_directory_recursive(dir).unwrap();
        assert!(db.is_empty());
        assert_eq!(error_count, 0);
    }
}
