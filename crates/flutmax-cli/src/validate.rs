use serde_json::Value;
use std::net::UdpSocket;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Validation error found by Layer 1 or Layer 2.
#[derive(Debug)]
pub struct ValidationError {
    pub layer: &'static str,
    pub location: Option<String>,
    pub message: String,
}

/// Layer 1 static validation report.
#[derive(Debug)]
pub struct ValidationReport {
    pub json_ok: bool,
    pub structure_ok: bool,
    pub boxes_count: usize,
    pub lines_count: usize,
    pub errors: Vec<ValidationError>,
    pub warnings: Vec<String>,
}

/// Layer 2 Max runtime validation result.
#[derive(Debug)]
pub struct MaxValidationResult {
    pub status: String,
    pub errors: Vec<MaxValidationError>,
    pub warnings: Vec<String>,
    pub boxes_checked: usize,
    pub lines_checked: usize,
}

/// Individual error from Max runtime validation.
#[derive(Debug)]
pub struct MaxValidationError {
    pub error_type: String,
    pub box_id: Option<String>,
    pub message: String,
}

/// Options for the validate subcommand.
#[derive(Debug)]
pub struct ValidateOptions {
    pub ci_only: bool,
    pub max_only: bool,
    pub full: bool,
    pub port: u16,
    pub timeout: Duration,
    pub input_path: String,
}

impl Default for ValidateOptions {
    fn default() -> Self {
        Self {
            ci_only: false,
            max_only: false,
            full: false,
            port: 7401,
            timeout: Duration::from_secs(10),
            input_path: String::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------

pub fn parse_validate_args(args: &[String]) -> Result<ValidateOptions, String> {
    let mut opts = ValidateOptions::default();
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "--ci" => {
                opts.ci_only = true;
                i += 1;
            }
            "--max" => {
                opts.max_only = true;
                i += 1;
            }
            "--full" => {
                opts.full = true;
                i += 1;
            }
            "--port" => {
                if i + 1 >= args.len() {
                    return Err("--port requires a port number argument".to_string());
                }
                opts.port = args[i + 1]
                    .parse::<u16>()
                    .map_err(|e| format!("invalid port number '{}': {}", args[i + 1], e))?;
                i += 2;
            }
            "--timeout" => {
                if i + 1 >= args.len() {
                    return Err("--timeout requires a seconds argument".to_string());
                }
                let secs = args[i + 1]
                    .parse::<u64>()
                    .map_err(|e| format!("invalid timeout '{}': {}", args[i + 1], e))?;
                opts.timeout = Duration::from_secs(secs);
                i += 2;
            }
            "--help" | "-h" => {
                return Err(String::new()); // signals help request
            }
            arg if arg.starts_with('-') => {
                return Err(format!("unknown option '{}'", arg));
            }
            arg => {
                if !opts.input_path.is_empty() {
                    return Err(format!("unexpected argument '{}'", arg));
                }
                opts.input_path = arg.to_string();
                i += 1;
            }
        }
    }

    if opts.input_path.is_empty() {
        return Err("missing input file path".to_string());
    }

    // Validate mutually exclusive flags
    let flag_count = [opts.ci_only, opts.max_only, opts.full]
        .iter()
        .filter(|&&b| b)
        .count();
    if flag_count > 1 {
        return Err("--ci, --max, and --full are mutually exclusive".to_string());
    }

    Ok(opts)
}

// ---------------------------------------------------------------------------
// Layer 1: Static validation
// ---------------------------------------------------------------------------

/// Validate JSON syntax of a .maxpat file.
fn validate_json_syntax(content: &str) -> Result<Value, ValidationError> {
    serde_json::from_str::<Value>(content).map_err(|e| ValidationError {
        layer: "Layer 1",
        location: Some(format!("line {}, column {}", e.line(), e.column())),
        message: format!("JSON parse error: {}", e),
    })
}

/// Validate .maxpat structure (required fields, box wrappers, etc.).
fn validate_structure(json: &Value) -> (usize, usize, Vec<ValidationError>) {
    let mut errors = Vec::new();
    let mut boxes_count = 0;
    let mut lines_count = 0;

    let patcher = match json.get("patcher") {
        Some(p) => p,
        None => {
            errors.push(ValidationError {
                layer: "Layer 1",
                location: None,
                message: "'patcher' root object not found".to_string(),
            });
            return (0, 0, errors);
        }
    };

    // Required patcher fields
    for field in &["fileversion", "boxes", "lines"] {
        if patcher.get(field).is_none() {
            errors.push(ValidationError {
                layer: "Layer 1",
                location: Some(format!("patcher.{}", field)),
                message: format!("missing required field '{}'", field),
            });
        }
    }

    // Validate boxes array
    if let Some(boxes) = patcher.get("boxes").and_then(|b| b.as_array()) {
        boxes_count = boxes.len();
        for (i, item) in boxes.iter().enumerate() {
            match item.get("box") {
                None => {
                    errors.push(ValidationError {
                        layer: "Layer 1",
                        location: Some(format!("patcher.boxes[{}]", i)),
                        message: "missing 'box' wrapper".to_string(),
                    });
                }
                Some(b) => {
                    for field in &["id", "maxclass", "numinlets", "numoutlets", "patching_rect"] {
                        if b.get(field).is_none() {
                            errors.push(ValidationError {
                                layer: "Layer 1",
                                location: Some(format!("patcher.boxes[{}].box", i)),
                                message: format!("missing required field '{}'", field),
                            });
                        }
                    }
                }
            }
        }
    }

    // Validate lines array and cross-reference with boxes
    if let Some(lines) = patcher.get("lines").and_then(|l| l.as_array()) {
        lines_count = lines.len();

        // Collect all box IDs for reference checking
        let box_ids: std::collections::HashSet<String> = patcher
            .get("boxes")
            .and_then(|b| b.as_array())
            .map(|boxes| {
                boxes
                    .iter()
                    .filter_map(|item| {
                        item.get("box")
                            .and_then(|b| b.get("id"))
                            .and_then(|id| id.as_str())
                            .map(|s| s.to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();

        for (i, line) in lines.iter().enumerate() {
            match line.get("patchline") {
                None => {
                    errors.push(ValidationError {
                        layer: "Layer 1",
                        location: Some(format!("patcher.lines[{}]", i)),
                        message: "missing 'patchline' wrapper".to_string(),
                    });
                }
                Some(pl) => {
                    // Check source reference exists
                    if let Some(source) = pl.get("source").and_then(|s| s.as_array()) {
                        if let Some(src_id) = source.first().and_then(|s| s.as_str()) {
                            if !box_ids.contains(src_id) {
                                errors.push(ValidationError {
                                    layer: "Layer 1",
                                    location: Some(format!("patcher.lines[{}].patchline", i)),
                                    message: format!("source '{}' not found", src_id),
                                });
                            }
                        }
                    }

                    // Check destination reference exists
                    if let Some(dest) = pl.get("destination").and_then(|d| d.as_array()) {
                        if let Some(dst_id) = dest.first().and_then(|d| d.as_str()) {
                            if !box_ids.contains(dst_id) {
                                errors.push(ValidationError {
                                    layer: "Layer 1",
                                    location: Some(format!("patcher.lines[{}].patchline", i)),
                                    message: format!("destination '{}' not found", dst_id),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    (boxes_count, lines_count, errors)
}

/// Run Layer 1 static validation on a .maxpat file.
pub fn validate_layer1(path: &Path) -> ValidationReport {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            return ValidationReport {
                json_ok: false,
                structure_ok: false,
                boxes_count: 0,
                lines_count: 0,
                errors: vec![ValidationError {
                    layer: "Layer 1",
                    location: None,
                    message: format!("failed to read file: {}", e),
                }],
                warnings: vec![],
            };
        }
    };

    // JSON syntax check
    let json = match validate_json_syntax(&content) {
        Ok(v) => v,
        Err(e) => {
            return ValidationReport {
                json_ok: false,
                structure_ok: false,
                boxes_count: 0,
                lines_count: 0,
                errors: vec![e],
                warnings: vec![],
            };
        }
    };

    // Structure check
    let (boxes_count, lines_count, errors) = validate_structure(&json);
    let structure_ok = errors.is_empty();

    ValidationReport {
        json_ok: true,
        structure_ok,
        boxes_count,
        lines_count,
        errors,
        warnings: vec![],
    }
}

// ---------------------------------------------------------------------------
// Layer 2: Max runtime validation via UDP
// ---------------------------------------------------------------------------

/// Generate a simple request ID from the current timestamp.
fn generate_request_id() -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("req-{}", ts)
}

/// Send a validation request to Node for Max via UDP and wait for response.
pub fn validate_via_max(
    maxpat_path: &str,
    port: u16,
    timeout: Duration,
) -> Result<MaxValidationResult, String> {
    // Resolve to absolute path
    let abs_path = std::fs::canonicalize(maxpat_path)
        .map_err(|e| format!("failed to resolve path '{}': {}", maxpat_path, e))?;
    let abs_path_str = abs_path.display().to_string();

    // Bind a socket on port+1 to receive the response
    let response_port = port + 1;
    let recv_socket = UdpSocket::bind(format!("127.0.0.1:{}", response_port))
        .map_err(|e| format!("failed to bind UDP socket on port {}: {}", response_port, e))?;
    recv_socket
        .set_read_timeout(Some(timeout))
        .map_err(|e| format!("failed to set socket timeout: {}", e))?;

    // Build request JSON
    let request_id = generate_request_id();
    let request = serde_json::json!({
        "id": request_id,
        "cmd": "validate",
        "path": abs_path_str,
    });
    let request_bytes = request.to_string().into_bytes();

    // Send request to Node for Max
    let send_socket = UdpSocket::bind("127.0.0.1:0")
        .map_err(|e| format!("failed to create send socket: {}", e))?;
    send_socket
        .send_to(&request_bytes, format!("127.0.0.1:{}", port))
        .map_err(|e| format!("failed to send UDP packet to port {}: {}", port, e))?;

    // Wait for response
    let mut buf = [0u8; 65535];
    let (len, _addr) = recv_socket.recv_from(&mut buf).map_err(|e| {
        if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut {
            format!(
                "timeout waiting for Max validator response ({}s). Is flutmax-validator.maxpat open?",
                timeout.as_secs()
            )
        } else {
            format!("failed to receive UDP response: {}", e)
        }
    })?;

    let response_str = std::str::from_utf8(&buf[..len])
        .map_err(|e| format!("invalid UTF-8 in response: {}", e))?;

    // Parse response JSON
    let response: Value = serde_json::from_str(response_str)
        .map_err(|e| format!("invalid JSON in response: {}", e))?;

    // Check that response ID matches
    if let Some(resp_id) = response.get("id").and_then(|v| v.as_str()) {
        if resp_id != request_id {
            return Err(format!(
                "response ID mismatch: expected '{}', got '{}'",
                request_id, resp_id
            ));
        }
    }

    // Extract result fields
    let status = response
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let boxes_checked = response
        .get("boxes_checked")
        .or_else(|| response.get("objects_checked"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;

    let lines_checked = response
        .get("lines_checked")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;

    let mut errors = Vec::new();
    if let Some(err_arr) = response.get("errors").and_then(|v| v.as_array()) {
        for err in err_arr {
            errors.push(MaxValidationError {
                error_type: err
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                box_id: err.get("box_id").and_then(|v| v.as_str()).map(String::from),
                message: err
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            });
        }
    }

    let mut warnings = Vec::new();
    if let Some(warn_arr) = response.get("warnings").and_then(|v| v.as_array()) {
        for w in warn_arr {
            if let Some(s) = w.as_str() {
                warnings.push(s.to_string());
            }
        }
    }

    Ok(MaxValidationResult {
        status,
        errors,
        warnings,
        boxes_checked,
        lines_checked,
    })
}

// ---------------------------------------------------------------------------
// Output formatting
// ---------------------------------------------------------------------------

fn print_header(path: &str) {
    eprintln!();
    eprintln!("=== flutmax validate: {} ===", path);
}

fn print_layer1_report(report: &ValidationReport) {
    // JSON syntax
    if report.json_ok {
        eprintln!("[Layer 1] JSON syntax      : OK");
    } else {
        eprintln!("[Layer 1] JSON syntax      : ERROR");
        for e in &report.errors {
            if e.message.contains("JSON parse error") {
                if let Some(ref loc) = e.location {
                    eprintln!("  - {}: {}", loc, e.message);
                } else {
                    eprintln!("  - {}", e.message);
                }
            }
        }
        return; // No point checking structure if JSON is invalid
    }

    // Structure
    if report.structure_ok {
        eprintln!(
            "[Layer 1] Structure        : OK ({} boxes, {} lines)",
            report.boxes_count, report.lines_count
        );
    } else {
        eprintln!("[Layer 1] Structure        : ERROR");
        for e in &report.errors {
            if let Some(ref loc) = e.location {
                eprintln!("  - {}: {}", loc, e.message);
            } else {
                eprintln!("  - {}", e.message);
            }
        }
    }
}

fn print_layer2_result(result: &Result<MaxValidationResult, String>) {
    match result {
        Ok(r) => {
            if r.errors.is_empty() {
                eprintln!(
                    "[Layer 2] Max runtime      : OK ({} boxes checked)",
                    r.boxes_checked
                );
            } else {
                eprintln!("[Layer 2] Max runtime      : ERROR");
                for e in &r.errors {
                    if let Some(ref box_id) = e.box_id {
                        eprintln!("  - [{}] {}: {}", e.error_type, box_id, e.message);
                    } else {
                        eprintln!("  - [{}] {}", e.error_type, e.message);
                    }
                }
            }
            for w in &r.warnings {
                eprintln!("  warning: {}", w);
            }
        }
        Err(msg) => {
            eprintln!("[Layer 2] Max runtime      : SKIP ({})", msg);
        }
    }
}

fn print_layer2_skip(port: u16) {
    eprintln!(
        "[Layer 2] Max runtime      : SKIP (validator not running on port {})",
        port
    );
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

/// Run the validate subcommand. Returns the process exit code.
pub fn run(args: &[String]) -> i32 {
    let opts = match parse_validate_args(args) {
        Ok(o) => o,
        Err(msg) if msg.is_empty() => {
            print_validate_usage();
            return 0;
        }
        Err(msg) => {
            eprintln!("error: {}", msg);
            eprintln!();
            print_validate_usage();
            return 1;
        }
    };

    let input = &opts.input_path;

    // If input is .flutmax, compile to a temp file first
    let maxpat_path: String;

    if input.ends_with(".flutmax") {
        // Compile .flutmax to a temporary .maxpat
        let source = match std::fs::read_to_string(input) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: failed to read '{}': {}", input, e);
                return 1;
            }
        };

        let json = match crate::compile(&source) {
            Ok(j) => j,
            Err(e) => {
                eprintln!("error: compilation failed: {}", e);
                return 1;
            }
        };

        // Write to a temp file in the system temp directory
        let temp_path = std::env::temp_dir().join("flutmax_validate_temp.maxpat");
        if let Err(e) = std::fs::write(&temp_path, &json) {
            eprintln!(
                "error: failed to write temp file '{}': {}",
                temp_path.display(),
                e
            );
            return 1;
        }
        eprintln!("compiled {} -> {}", input, temp_path.display());
        maxpat_path = temp_path.display().to_string();
    } else if input.ends_with(".maxpat") {
        maxpat_path = input.clone();
    } else {
        eprintln!(
            "error: unsupported file extension. Expected .flutmax or .maxpat, got '{}'",
            input
        );
        return 1;
    }

    print_header(&opts.input_path);

    let mut total_errors = 0usize;
    let mut total_warnings = 0usize;

    // Determine which layers to run
    let run_layer1 = !opts.max_only;
    let run_layer2 = !opts.ci_only;

    // Layer 1: Static validation
    let layer1_report = if run_layer1 {
        let report = validate_layer1(Path::new(&maxpat_path));
        print_layer1_report(&report);
        total_errors += report.errors.len();
        total_warnings += report.warnings.len();
        Some(report)
    } else {
        None
    };

    // Don't run Layer 2 if Layer 1 JSON failed
    let layer1_json_ok = layer1_report.as_ref().map(|r| r.json_ok).unwrap_or(true);

    // Layer 2: Max runtime validation
    if run_layer2 {
        if !layer1_json_ok {
            eprintln!("[Layer 2] Max runtime      : SKIP (JSON invalid)");
        } else {
            // Try to connect; gracefully handle failure
            let result = validate_via_max(&maxpat_path, opts.port, opts.timeout);
            match &result {
                Ok(r) => {
                    total_errors += r.errors.len();
                    total_warnings += r.warnings.len();
                    print_layer2_result(&result);
                }
                Err(_) => {
                    print_layer2_skip(opts.port);
                }
            }
        }
    }

    // Summary
    eprintln!();
    if total_errors == 0 && total_warnings == 0 {
        eprintln!("Summary: 0 errors, 0 warnings");
    } else {
        eprintln!(
            "Summary: {} error{}, {} warning{}",
            total_errors,
            if total_errors == 1 { "" } else { "s" },
            total_warnings,
            if total_warnings == 1 { "" } else { "s" },
        );
    }

    // Clean up temp file if we created one
    if input.ends_with(".flutmax") {
        let temp_path = std::env::temp_dir().join("flutmax_validate_temp.maxpat");
        let _ = std::fs::remove_file(temp_path);
    }

    if total_errors > 0 {
        1
    } else {
        0
    }
}

fn print_validate_usage() {
    eprintln!("flutmax validate - validate .maxpat files");
    eprintln!();
    eprintln!("USAGE:");
    eprintln!("    flutmax validate [options] <file.maxpat>");
    eprintln!("    flutmax validate [options] <file.flutmax>   (compiles first, then validates)");
    eprintln!();
    eprintln!("OPTIONS:");
    eprintln!("    --ci          Layer 1 only (static checks, no Max required)");
    eprintln!("    --max         Layer 2 only (Node for Max runtime check)");
    eprintln!("    --full        Layer 1 + Layer 2 (default)");
    eprintln!("    --port <N>    UDP port for Node for Max (default: 7401)");
    eprintln!("    --timeout <S> Timeout in seconds for Max response (default: 10)");
    eprintln!("    -h, --help    Print help information");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_args_basic() {
        let args = vec!["test.maxpat".to_string()];
        let opts = parse_validate_args(&args).unwrap();
        assert_eq!(opts.input_path, "test.maxpat");
        assert!(!opts.ci_only);
        assert!(!opts.max_only);
        assert!(!opts.full);
        assert_eq!(opts.port, 7401);
        assert_eq!(opts.timeout, Duration::from_secs(10));
    }

    #[test]
    fn test_parse_args_ci() {
        let args = vec!["--ci".to_string(), "test.maxpat".to_string()];
        let opts = parse_validate_args(&args).unwrap();
        assert!(opts.ci_only);
        assert_eq!(opts.input_path, "test.maxpat");
    }

    #[test]
    fn test_parse_args_max_with_port() {
        let args = vec![
            "--max".to_string(),
            "--port".to_string(),
            "8000".to_string(),
            "--timeout".to_string(),
            "30".to_string(),
            "output.maxpat".to_string(),
        ];
        let opts = parse_validate_args(&args).unwrap();
        assert!(opts.max_only);
        assert_eq!(opts.port, 8000);
        assert_eq!(opts.timeout, Duration::from_secs(30));
        assert_eq!(opts.input_path, "output.maxpat");
    }

    #[test]
    fn test_parse_args_mutually_exclusive() {
        let args = vec![
            "--ci".to_string(),
            "--max".to_string(),
            "test.maxpat".to_string(),
        ];
        let result = parse_validate_args(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("mutually exclusive"));
    }

    #[test]
    fn test_parse_args_missing_input() {
        let args: Vec<String> = vec!["--ci".to_string()];
        let result = parse_validate_args(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing input"));
    }

    #[test]
    fn test_validate_json_syntax_valid() {
        let content = r#"{"patcher": {"boxes": [], "lines": [], "fileversion": 1}}"#;
        let result = validate_json_syntax(content);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_json_syntax_invalid() {
        let content = r#"{"patcher": {"boxes": [}}"#;
        let result = validate_json_syntax(content);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_structure_valid() {
        let json: Value = serde_json::from_str(
            r#"{
                "patcher": {
                    "fileversion": 1,
                    "boxes": [
                        {
                            "box": {
                                "id": "obj-1",
                                "maxclass": "newobj",
                                "numinlets": 1,
                                "numoutlets": 1,
                                "patching_rect": [100, 100, 50, 22]
                            }
                        }
                    ],
                    "lines": []
                }
            }"#,
        )
        .unwrap();
        let (boxes, lines, errors) = validate_structure(&json);
        assert_eq!(boxes, 1);
        assert_eq!(lines, 0);
        assert!(errors.is_empty());
    }

    #[test]
    fn test_validate_structure_missing_patcher() {
        let json: Value = serde_json::from_str(r#"{"not_patcher": {}}"#).unwrap();
        let (_, _, errors) = validate_structure(&json);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("patcher"));
    }

    #[test]
    fn test_validate_structure_missing_box_id() {
        let json: Value = serde_json::from_str(
            r#"{
                "patcher": {
                    "fileversion": 1,
                    "boxes": [
                        {
                            "box": {
                                "maxclass": "newobj",
                                "numinlets": 1,
                                "numoutlets": 1,
                                "patching_rect": [100, 100, 50, 22]
                            }
                        }
                    ],
                    "lines": []
                }
            }"#,
        )
        .unwrap();
        let (_, _, errors) = validate_structure(&json);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("id"));
    }

    #[test]
    fn test_validate_structure_dangling_source() {
        let json: Value = serde_json::from_str(
            r#"{
                "patcher": {
                    "fileversion": 1,
                    "boxes": [
                        {
                            "box": {
                                "id": "obj-1",
                                "maxclass": "newobj",
                                "numinlets": 1,
                                "numoutlets": 1,
                                "patching_rect": [100, 100, 50, 22]
                            }
                        }
                    ],
                    "lines": [
                        {
                            "patchline": {
                                "source": ["obj-99", 0],
                                "destination": ["obj-1", 0]
                            }
                        }
                    ]
                }
            }"#,
        )
        .unwrap();
        let (_, _, errors) = validate_structure(&json);
        assert!(!errors.is_empty());
        assert!(errors.iter().any(|e| e.message.contains("obj-99")));
    }

    #[test]
    fn test_generate_request_id() {
        let id1 = generate_request_id();
        assert!(id1.starts_with("req-"));
        assert!(id1.len() > 4);
    }
}
