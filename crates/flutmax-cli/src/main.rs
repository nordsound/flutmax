use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        print_usage();
        process::exit(1);
    }

    match args[1].as_str() {
        "compile" => run_compile(&args[2..]),
        "decompile" => run_decompile(&args[2..]),
        "validate" => {
            let code = flutmax_cli::validate::run(&args[2..]);
            process::exit(code);
        }
        "--help" | "-h" => {
            print_usage();
            process::exit(0);
        }
        "--version" | "-V" => {
            eprintln!("flutmax {}", env!("CARGO_PKG_VERSION"));
            process::exit(0);
        }
        other => {
            eprintln!("error: unknown command '{}'", other);
            eprintln!();
            print_usage();
            process::exit(1);
        }
    }
}

fn print_usage() {
    eprintln!("flutmax - transpile .flutmax to .maxpat");
    eprintln!();
    eprintln!("USAGE:");
    eprintln!("    flutmax compile <input.flutmax> -o <output.maxpat>");
    eprintln!("    flutmax compile --gen <input.flutmax> -o <output.maxpat>");
    eprintln!("    flutmax compile --rnbo <input.flutmax> -o <output.maxpat>");
    eprintln!("    flutmax compile <input_dir/> -o <output_dir/>");
    eprintln!("    flutmax decompile <input.maxpat> -o <output.flutmax>");
    eprintln!("    flutmax decompile --multi <input.maxpat> -o <output.flutmax>");
    eprintln!("    flutmax validate [options] <file.maxpat|file.flutmax>");
    eprintln!("    flutmax --help");
    eprintln!("    flutmax --version");
    eprintln!();
    eprintln!("COMMANDS:");
    eprintln!("    compile    Transpile .flutmax file(s) to .maxpat file(s)");
    eprintln!("    decompile  Decompile .maxpat file to .flutmax source");
    eprintln!("    validate   Validate a .maxpat file (static + optional Max runtime)");
    eprintln!();
    eprintln!("OPTIONS:");
    eprintln!("    -o <path>      Output file or directory path (required for compile/decompile)");
    eprintln!("    --gen          Compile as gen~ patcher (classnamespace: dsp.gen)");
    eprintln!("    --rnbo         Compile as RNBO patcher (classnamespace: rnbo)");
    eprintln!("    --multi        Multi-file decompile: extract subpatchers as separate files");
    eprintln!("    -h, --help     Print help information");
    eprintln!("    -V, --version  Print version information");
}

fn run_compile(args: &[String]) {
    let mut input_path: Option<&str> = None;
    let mut output_path: Option<&str> = None;
    let mut gen_mode = false;
    let mut rnbo_mode = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                if i + 1 >= args.len() {
                    eprintln!("error: -o requires an output path argument");
                    process::exit(1);
                }
                output_path = Some(&args[i + 1]);
                i += 2;
            }
            "--gen" => {
                gen_mode = true;
                i += 1;
            }
            "--rnbo" => {
                rnbo_mode = true;
                i += 1;
            }
            "--help" | "-h" => {
                print_usage();
                process::exit(0);
            }
            arg if arg.starts_with('-') => {
                eprintln!("error: unknown option '{}'", arg);
                process::exit(1);
            }
            arg => {
                if input_path.is_some() {
                    eprintln!("error: unexpected argument '{}'", arg);
                    process::exit(1);
                }
                input_path = Some(arg);
                i += 1;
            }
        }
    }

    let input_path = match input_path {
        Some(p) => p,
        None => {
            eprintln!("error: missing input file path");
            eprintln!();
            print_usage();
            process::exit(1);
        }
    };

    let output_path = match output_path {
        Some(p) => p,
        None => {
            eprintln!("error: missing output file path (-o <path>)");
            eprintln!();
            print_usage();
            process::exit(1);
        }
    };

    // Load object database for accurate inlet/outlet inference
    let objdb = flutmax_validate::try_load_max_objdb();

    let input_meta = fs::metadata(input_path);
    if input_meta.map(|m| m.is_dir()).unwrap_or(false) {
        run_compile_directory(input_path, output_path, objdb.as_ref(), gen_mode, rnbo_mode);
    } else {
        run_compile_single(input_path, output_path, objdb.as_ref(), gen_mode, rnbo_mode);
    }
}

fn run_compile_single(
    input_path: &str,
    output_path: &str,
    objdb: Option<&flutmax_objdb::ObjectDb>,
    gen_mode: bool,
    rnbo_mode: bool,
) {
    // Read input file
    let source = match fs::read_to_string(input_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: failed to read '{}': {}", input_path, e);
            process::exit(1);
        }
    };

    if gen_mode {
        // gen~ mode: use compile_gen() with dsp.gen classnamespace
        let json = match flutmax_cli::compile_gen(&source) {
            Ok(j) => j,
            Err(e) => {
                eprintln!("error: gen~ compilation failed: {}", e);
                process::exit(1);
            }
        };
        write_output(output_path, &json);
        eprintln!("compiled {} -> {} (gen~)", input_path, output_path);
        return;
    }

    if rnbo_mode {
        // RNBO mode: use compile_rnbo() with rnbo classnamespace
        let json = match flutmax_cli::compile_rnbo(&source) {
            Ok(j) => j,
            Err(e) => {
                eprintln!("error: RNBO compilation failed: {}", e);
                process::exit(1);
            }
        };
        write_output(output_path, &json);
        eprintln!("compiled {} -> {} (RNBO)", input_path, output_path);
        return;
    }

    // Read code files (.js, .genexpr) from the same directory
    let code_files = load_code_files(input_path);
    let code_files_ref = if code_files.is_empty() {
        None
    } else {
        Some(&code_files)
    };

    // Load .uiflutmax sidecar file if present
    let ui_data = load_ui_data(input_path);

    // Compile
    let json = match flutmax_cli::compile_full_with_ui(
        &source,
        None,
        code_files_ref,
        objdb,
        ui_data.as_ref(),
    ) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("error: compilation failed: {}", e);
            process::exit(1);
        }
    };

    // Write output file
    write_output(output_path, &json);
    if ui_data.is_some() {
        eprintln!(
            "compiled {} -> {} (with .uiflutmax)",
            input_path, output_path
        );
    } else {
        eprintln!("compiled {} -> {}", input_path, output_path);
    }
}

fn run_decompile(args: &[String]) {
    let mut input_path: Option<&str> = None;
    let mut output_path: Option<&str> = None;
    let mut multi = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                if i + 1 >= args.len() {
                    eprintln!("error: -o requires an output path argument");
                    process::exit(1);
                }
                output_path = Some(&args[i + 1]);
                i += 2;
            }
            "--multi" => {
                multi = true;
                i += 1;
            }
            "--help" | "-h" => {
                print_usage();
                process::exit(0);
            }
            arg if arg.starts_with('-') => {
                eprintln!("error: unknown option '{}'", arg);
                process::exit(1);
            }
            arg => {
                if input_path.is_some() {
                    eprintln!("error: unexpected argument '{}'", arg);
                    process::exit(1);
                }
                input_path = Some(arg);
                i += 1;
            }
        }
    }

    let input_path = match input_path {
        Some(p) => p,
        None => {
            eprintln!("error: missing input .maxpat file path");
            eprintln!();
            print_usage();
            process::exit(1);
        }
    };

    let output_path = match output_path {
        Some(p) => p,
        None => {
            eprintln!("error: missing output file path (-o <path>)");
            eprintln!();
            print_usage();
            process::exit(1);
        }
    };

    // Read input .maxpat file
    let json_str = match fs::read_to_string(input_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: failed to read '{}': {}", input_path, e);
            process::exit(1);
        }
    };

    // Extract base name from input path
    let base_name = std::path::Path::new(input_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("main");

    // Load objdb for named argument output
    let objdb = flutmax_validate::try_load_max_objdb();

    if multi {
        // Multi-file decompile: extract subpatchers as separate .flutmax files
        run_decompile_multi(
            &json_str,
            base_name,
            input_path,
            output_path,
            objdb.as_ref(),
        );
    } else {
        // Single-file decompile with named args when objdb is available
        let flutmax_source =
            match flutmax_decompile::decompile_with_objdb(&json_str, objdb.as_ref()) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("error: decompilation failed: {}", e);
                    process::exit(1);
                }
            };

        write_output(output_path, &flutmax_source);
        eprintln!("decompiled {} -> {}", input_path, output_path);
    }
}

fn run_decompile_multi(
    json_str: &str,
    base_name: &str,
    input_path: &str,
    output_path: &str,
    objdb: Option<&flutmax_objdb::ObjectDb>,
) {
    use std::path::Path;

    let result = match flutmax_decompile::decompile_multi_with_objdb(json_str, base_name, objdb) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: multi-file decompilation failed: {}", e);
            process::exit(1);
        }
    };

    if result.files.len() == 1 && result.code_files.is_empty() {
        // Single file, no code files — write directly to output_path
        let source = result.files.values().next().unwrap();
        write_output(output_path, source);

        // Write .uiflutmax sidecar files alongside the main output
        let dir = Path::new(output_path).parent().unwrap_or(Path::new("."));
        for (filename, content) in &result.ui_files {
            let file_path = dir.join(filename);
            write_output(&file_path.to_string_lossy(), content);
        }

        if result.ui_files.is_empty() {
            eprintln!("decompiled {} -> {}", input_path, output_path);
        } else {
            eprintln!(
                "decompiled {} -> {} + {} ui file(s)",
                input_path,
                output_path,
                result.ui_files.len()
            );
        }
    } else {
        // Multiple files — write to output directory
        let dir = Path::new(output_path).parent().unwrap_or(Path::new("."));
        if !dir.exists() {
            if let Err(e) = fs::create_dir_all(dir) {
                eprintln!(
                    "error: failed to create directory '{}': {}",
                    dir.display(),
                    e
                );
                process::exit(1);
            }
        }

        for (filename, content) in &result.files {
            let file_path = dir.join(filename);
            write_output(&file_path.to_string_lossy(), content);
        }

        // Write code files (.js, .genexpr) extracted from codebox objects
        for (filename, content) in &result.code_files {
            let file_path = dir.join(filename);
            write_output(&file_path.to_string_lossy(), content);
        }

        // Write .uiflutmax sidecar files
        for (filename, content) in &result.ui_files {
            let file_path = dir.join(filename);
            write_output(&file_path.to_string_lossy(), content);
        }

        let total = result.files.len() + result.code_files.len() + result.ui_files.len();
        eprintln!(
            "decompiled {} -> {} files in {}",
            input_path,
            total,
            dir.display()
        );
    }
}

fn run_compile_directory(
    input_dir: &str,
    output_dir: &str,
    objdb: Option<&flutmax_objdb::ObjectDb>,
    gen_mode: bool,
    rnbo_mode: bool,
) {
    use flutmax_sema::registry::AbstractionRegistry;
    use std::path::Path;

    // 1. Glob all .flutmax files
    let input_path = Path::new(input_dir);
    let mut flutmax_files: Vec<std::path::PathBuf> = Vec::new();

    let entries = match fs::read_dir(input_path) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("error: failed to read directory '{}': {}", input_dir, e);
            process::exit(1);
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                eprintln!("error: failed to read directory entry: {}", e);
                process::exit(1);
            }
        };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("flutmax") {
            flutmax_files.push(path);
        }
    }

    if flutmax_files.is_empty() {
        eprintln!("error: no .flutmax files found in '{}'", input_dir);
        process::exit(1);
    }

    // Sort for deterministic order
    flutmax_files.sort();

    // 2. Parse all files
    let mut parsed: Vec<(String, String, flutmax_ast::Program)> = Vec::new(); // (stem, source, ast)

    for path in &flutmax_files {
        let source = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: failed to read '{}': {}", path.display(), e);
                process::exit(1);
            }
        };

        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        let ast = match flutmax_parser::parse(&source) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("error: failed to parse '{}': {}", path.display(), e);
                process::exit(1);
            }
        };

        parsed.push((stem, source, ast));
    }

    // 3. Register all in AbstractionRegistry
    let mut registry = AbstractionRegistry::new();
    for (stem, _, ast) in &parsed {
        registry.register(stem, ast);
    }

    // 4. Auto-detect gen~ subpatchers from gen~(name) / mc.gen~(name) references
    let auto_gen_files = collect_gen_references(&parsed);

    // 4b. Auto-detect rnbo~ subpatchers from rnbo~("name") references
    let auto_rnbo_files = collect_rnbo_references(&parsed);

    // 5. Read code files (.js, .genexpr) from input directory
    let code_files = load_code_files_from_dir(input_dir);
    let code_files_ref = if code_files.is_empty() {
        None
    } else {
        Some(&code_files)
    };

    // 6. Compile each file with the registry, code files, and UI data
    let mut compiled: HashMap<String, String> = HashMap::new(); // stem -> json string

    for (i, (stem, source, _)) in parsed.iter().enumerate() {
        let is_gen = gen_mode || auto_gen_files.contains(stem);
        let is_rnbo = rnbo_mode || auto_rnbo_files.contains(stem);
        let json = if is_gen {
            match flutmax_cli::compile_gen(source) {
                Ok(j) => j,
                Err(e) => {
                    eprintln!(
                        "error: gen~ compilation of '{}' failed: {}",
                        flutmax_files[i].display(),
                        e
                    );
                    process::exit(1);
                }
            }
        } else if is_rnbo {
            match flutmax_cli::compile_rnbo(source) {
                Ok(j) => j,
                Err(e) => {
                    eprintln!(
                        "error: RNBO compilation of '{}' failed: {}",
                        flutmax_files[i].display(),
                        e
                    );
                    process::exit(1);
                }
            }
        } else {
            let ui_data = load_ui_data(&flutmax_files[i].to_string_lossy());
            match flutmax_cli::compile_full_with_ui(
                source,
                Some(&registry),
                code_files_ref,
                objdb,
                ui_data.as_ref(),
            ) {
                Ok(j) => j,
                Err(e) => {
                    eprintln!(
                        "error: compilation of '{}' failed: {}",
                        flutmax_files[i].display(),
                        e
                    );
                    process::exit(1);
                }
            }
        };

        let mode_label = if is_gen {
            " (gen~)"
        } else if is_rnbo {
            " (RNBO)"
        } else {
            ""
        };
        eprintln!("compiled {}{}", flutmax_files[i].display(), mode_label);
        compiled.insert(stem.clone(), json);
    }

    // 7. Post-process: embed subpatchers (gen~ into RNBO, RNBO into top-level)
    embed_subpatchers(&mut compiled, &auto_gen_files, &auto_rnbo_files);

    // 8. Write each .maxpat
    for (stem, json) in &compiled {
        let output_file = Path::new(output_dir).join(format!("{}.maxpat", stem));
        let output_str = output_file.to_string_lossy().to_string();
        write_output(&output_str, json);
        eprintln!("wrote {}", output_str);
    }
}

/// Load .uiflutmax sidecar file alongside a .flutmax input file.
/// Returns None if the file doesn't exist or can't be parsed.
fn load_ui_data(input_path: &str) -> Option<flutmax_codegen::UiData> {
    let ui_path = input_path.replace(".flutmax", ".uiflutmax");
    let content = fs::read_to_string(&ui_path).ok()?;
    flutmax_codegen::UiData::from_json(&content)
}

/// Load code files (.js, .genexpr) from the same directory as the input file.
fn load_code_files(input_path: &str) -> std::collections::HashMap<String, String> {
    let dir = std::path::Path::new(input_path)
        .parent()
        .unwrap_or(std::path::Path::new("."));
    load_code_files_from_dir(&dir.to_string_lossy())
}

/// Load code files (.js, .genexpr) from a directory.
fn load_code_files_from_dir(dir_path: &str) -> std::collections::HashMap<String, String> {
    let mut code_files = std::collections::HashMap::new();
    let dir = std::path::Path::new(dir_path);
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if matches!(ext, "js" | "genexpr") {
                if let Ok(content) = fs::read_to_string(&path) {
                    let filename = path.file_name().unwrap().to_string_lossy().to_string();
                    code_files.insert(filename, content);
                }
            }
        }
    }
    code_files
}

fn write_output(output_path: &str, content: &str) {
    // Create parent directories if they don't exist
    if let Some(parent) = std::path::Path::new(output_path).parent() {
        if !parent.exists() {
            if let Err(e) = fs::create_dir_all(parent) {
                eprintln!(
                    "error: failed to create directory '{}': {}",
                    parent.display(),
                    e
                );
                process::exit(1);
            }
        }
    }

    if let Err(e) = fs::write(output_path, content) {
        eprintln!("error: failed to write '{}': {}", output_path, e);
        process::exit(1);
    }
}

/// Collect gen~ subpatcher names referenced by `gen~(name)` or `mc.gen~(name)` calls
/// across all parsed programs.
fn collect_gen_references(programs: &[(String, String, flutmax_ast::Program)]) -> HashSet<String> {
    let mut gen_files = HashSet::new();
    for (_, _, ast) in programs {
        for wire in &ast.wires {
            collect_gen_refs_from_expr(&wire.value, &mut gen_files);
        }
        for out in &ast.out_decls {
            if let Some(ref expr) = out.value {
                collect_gen_refs_from_expr(expr, &mut gen_files);
            }
        }
        for out in &ast.out_assignments {
            collect_gen_refs_from_expr(&out.value, &mut gen_files);
        }
        for dc in &ast.direct_connections {
            collect_gen_refs_from_expr(&dc.value, &mut gen_files);
        }
        for dw in &ast.destructuring_wires {
            collect_gen_refs_from_expr(&dw.value, &mut gen_files);
        }
    }
    gen_files
}

fn collect_gen_refs_from_expr(expr: &flutmax_ast::Expr, gen_files: &mut HashSet<String>) {
    match expr {
        flutmax_ast::Expr::Call { object, args } => {
            if object == "gen~" || object == "mc.gen~" {
                // First argument of gen~() is the subpatcher name (Ref or string literal)
                if let Some(first_arg) = args.first() {
                    match &first_arg.value {
                        flutmax_ast::Expr::Ref(name) => {
                            gen_files.insert(name.clone());
                        }
                        flutmax_ast::Expr::Lit(flutmax_ast::LitValue::Str(name)) => {
                            gen_files.insert(name.clone());
                        }
                        _ => {}
                    }
                }
            }
            for arg in args {
                collect_gen_refs_from_expr(&arg.value, gen_files);
            }
        }
        flutmax_ast::Expr::Tuple(exprs) => {
            for e in exprs {
                collect_gen_refs_from_expr(e, gen_files);
            }
        }
        _ => {}
    }
}

/// Collect rnbo~ subpatcher names referenced by `rnbo~("name")` calls
/// across all parsed programs.
fn collect_rnbo_references(programs: &[(String, String, flutmax_ast::Program)]) -> HashSet<String> {
    let mut rnbo_files = HashSet::new();
    for (_, _, ast) in programs {
        for wire in &ast.wires {
            collect_rnbo_refs_from_expr(&wire.value, &mut rnbo_files);
        }
        for out in &ast.out_decls {
            if let Some(ref expr) = out.value {
                collect_rnbo_refs_from_expr(expr, &mut rnbo_files);
            }
        }
        for out in &ast.out_assignments {
            collect_rnbo_refs_from_expr(&out.value, &mut rnbo_files);
        }
        for dc in &ast.direct_connections {
            collect_rnbo_refs_from_expr(&dc.value, &mut rnbo_files);
        }
        for dw in &ast.destructuring_wires {
            collect_rnbo_refs_from_expr(&dw.value, &mut rnbo_files);
        }
    }
    rnbo_files
}

fn collect_rnbo_refs_from_expr(expr: &flutmax_ast::Expr, rnbo_files: &mut HashSet<String>) {
    match expr {
        flutmax_ast::Expr::Call { object, args } => {
            if object == "rnbo~" {
                // First argument of rnbo~() is the subpatcher name (string literal or Ref)
                if let Some(first_arg) = args.first() {
                    match &first_arg.value {
                        flutmax_ast::Expr::Ref(name) => {
                            rnbo_files.insert(name.clone());
                        }
                        flutmax_ast::Expr::Lit(flutmax_ast::LitValue::Str(name)) => {
                            rnbo_files.insert(name.clone());
                        }
                        _ => {}
                    }
                }
            }
            for arg in args {
                collect_rnbo_refs_from_expr(&arg.value, rnbo_files);
            }
        }
        flutmax_ast::Expr::Tuple(exprs) => {
            for e in exprs {
                collect_rnbo_refs_from_expr(e, rnbo_files);
            }
        }
        _ => {}
    }
}

/// Generate a simple UUID-like string for RNBO saved_object_attributes.
fn generate_uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // Format as UUID-like: 8-4-4-4-12 hex
    let hex = format!("{:032x}", ts);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Post-process compiled JSON to embed subpatchers:
/// 1. Embed gen~ patchers into RNBO patchers
/// 2. Embed RNBO patchers into top-level patches
fn embed_subpatchers(
    compiled: &mut HashMap<String, String>,
    gen_refs: &HashSet<String>,
    rnbo_refs: &HashSet<String>,
) {
    use serde_json::{json, Value};

    // Parse all compiled JSON into serde_json::Value
    let mut values: HashMap<String, Value> = HashMap::new();
    for (stem, json_str) in compiled.iter() {
        if let Ok(v) = serde_json::from_str::<Value>(json_str) {
            values.insert(stem.clone(), v);
        }
    }

    // Phase 5: Embed gen~ patchers into RNBO patchers
    // For each RNBO file, find gen~ boxes and embed the compiled gen~ patcher
    let rnbo_stems: Vec<String> = rnbo_refs.iter().cloned().collect();
    let gen_values: HashMap<String, Value> = gen_refs
        .iter()
        .filter_map(|name| values.get(name).map(|v| (name.clone(), v.clone())))
        .collect();

    for rnbo_stem in &rnbo_stems {
        if let Some(rnbo_val) = values.get_mut(rnbo_stem) {
            let mut serial = 1u64;
            if let Some(boxes) = rnbo_val
                .pointer_mut("/patcher/boxes")
                .and_then(|b| b.as_array_mut())
            {
                for box_wrapper in boxes.iter_mut() {
                    if let Some(box_obj) = box_wrapper.get_mut("box") {
                        let text = box_obj
                            .get("text")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string();
                        // Match "gen~ <name>" or "gen~ <name> @attr ..." pattern
                        if let Some(gen_rest) = text.strip_prefix("gen~ ") {
                            let gen_name = gen_rest.split_whitespace().next().unwrap_or(gen_rest);
                            if let Some(gen_json) = gen_values.get(gen_name) {
                                // Embed gen~ patcher
                                if let Some(gen_patcher) = gen_json.get("patcher") {
                                    box_obj["patcher"] = gen_patcher.clone();
                                }
                                // Update text to use @title attribute
                                box_obj["text"] = json!(format!("gen~ @title {}", gen_name));
                                // Add RNBO-specific attributes
                                box_obj["rnbo_classname"] = json!("gen~");
                                let box_id = box_obj
                                    .get("id")
                                    .and_then(|i| i.as_str())
                                    .unwrap_or("unknown")
                                    .to_string();
                                box_obj["rnbo_serial"] = json!(serial);
                                box_obj["rnbo_uniqueid"] = json!(format!("gen~_{}", box_id));
                                serial += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    // Phase 4: Embed RNBO patchers into top-level patches
    // For each non-gen, non-rnbo file, find rnbo~ boxes and embed the RNBO patcher
    let top_level_stems: Vec<String> = values
        .keys()
        .filter(|s| !gen_refs.contains(s.as_str()) && !rnbo_refs.contains(s.as_str()))
        .cloned()
        .collect();

    // Re-collect RNBO values after gen~ embedding (they may have been modified)
    let rnbo_values: HashMap<String, Value> = rnbo_refs
        .iter()
        .filter_map(|name| values.get(name).map(|v| (name.clone(), v.clone())))
        .collect();

    for top_stem in &top_level_stems {
        if let Some(top_val) = values.get_mut(top_stem) {
            if let Some(boxes) = top_val
                .pointer_mut("/patcher/boxes")
                .and_then(|b| b.as_array_mut())
            {
                for box_wrapper in boxes.iter_mut() {
                    if let Some(box_obj) = box_wrapper.get_mut("box") {
                        let text = box_obj
                            .get("text")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string();
                        // Match "rnbo~ <name>" or "rnbo~ <name> @attr ..." pattern
                        if let Some(rnbo_rest) = text.strip_prefix("rnbo~ ") {
                            let rnbo_name =
                                rnbo_rest.split_whitespace().next().unwrap_or(rnbo_rest);
                            // Collect @attributes to preserve after embedding
                            let rnbo_attrs =
                                rnbo_rest.find('@').map(|i| &rnbo_rest[i..]).unwrap_or("");
                            if let Some(rnbo_json) = rnbo_values.get(rnbo_name) {
                                // Embed RNBO patcher
                                if let Some(rnbo_patcher) = rnbo_json.get("patcher") {
                                    box_obj["patcher"] = rnbo_patcher.clone();

                                    // Count inports and signal outlets from the embedded patcher
                                    let (n_inports, n_signal_outlets) =
                                        count_rnbo_ports(rnbo_patcher);

                                    // Update inlet/outlet counts
                                    // rnbo~ always has at least 2 inlets:
                                    //   inlet 0: parameter/message
                                    //   inlet 1: MIDI
                                    box_obj["numinlets"] = json!(std::cmp::max(n_inports + 2, 2));
                                    // numoutlets = signal outlets + 1 (list outlet)
                                    box_obj["numoutlets"] = json!(n_signal_outlets + 1);

                                    // Build outlettype array: ["signal", ...] + ["list"]
                                    let mut outlet_types: Vec<Value> =
                                        (0..n_signal_outlets).map(|_| json!("signal")).collect();
                                    outlet_types.push(json!("list"));
                                    box_obj["outlettype"] = Value::Array(outlet_types);

                                    // Add inletInfo (default MIDI inlet)
                                    box_obj["inletInfo"] = json!({
                                        "IOInfo": [{"type": "midi", "index": -1, "tag": "", "comment": ""}]
                                    });

                                    // Add outletInfo from out~ boxes
                                    let mut io_info: Vec<Value> = Vec::new();
                                    for i in 1..=n_signal_outlets {
                                        io_info.push(json!({
                                            "type": "signal",
                                            "index": i,
                                            "tag": format!("out{}", i),
                                            "comment": ""
                                        }));
                                    }
                                    box_obj["outletInfo"] = json!({"IOInfo": io_info});
                                }

                                // Change text from "rnbo~ name @attrs" to "rnbo~ @attrs" or "rnbo~"
                                if rnbo_attrs.is_empty() {
                                    box_obj["text"] = json!("rnbo~");
                                } else {
                                    box_obj["text"] = json!(format!("rnbo~ {}", rnbo_attrs));
                                }

                                // Add saved_object_attributes
                                box_obj["saved_object_attributes"] = json!({
                                    "optimization": "O1",
                                    "parameter_enable": 1,
                                    "uuid": generate_uuid()
                                });
                                box_obj["autosave"] = json!(1);
                            }
                        }
                    }
                }
            }
        }
    }

    // Serialize back to compiled map
    for (stem, val) in &values {
        if let Ok(json_str) = serde_json::to_string_pretty(val) {
            compiled.insert(stem.clone(), json_str);
        }
    }
}

/// Count inport boxes and signal outlet (out~) boxes in an RNBO patcher.
/// Returns (n_inports, n_signal_outlets).
fn count_rnbo_ports(patcher: &serde_json::Value) -> (u64, u64) {
    let mut n_inports = 0u64;
    let mut n_signal_outlets = 0u64;

    if let Some(boxes) = patcher.get("boxes").and_then(|b| b.as_array()) {
        for box_wrapper in boxes {
            if let Some(text) = box_wrapper.pointer("/box/text").and_then(|t| t.as_str()) {
                if text.starts_with("inport") {
                    n_inports += 1;
                } else if text.starts_with("out~") {
                    n_signal_outlets += 1;
                }
            }
        }
    }

    (n_inports, n_signal_outlets)
}
