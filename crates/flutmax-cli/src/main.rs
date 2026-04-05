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
    eprintln!("    --multi        Multi-file decompile: extract subpatchers as separate files");
    eprintln!("    -h, --help     Print help information");
    eprintln!("    -V, --version  Print version information");
}

fn run_compile(args: &[String]) {
    let mut input_path: Option<&str> = None;
    let mut output_path: Option<&str> = None;

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
        run_compile_directory(input_path, output_path, objdb.as_ref());
    } else {
        run_compile_single(input_path, output_path, objdb.as_ref());
    }
}

fn run_compile_single(
    input_path: &str,
    output_path: &str,
    objdb: Option<&flutmax_objdb::ObjectDb>,
) {
    // Read input file
    let source = match fs::read_to_string(input_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: failed to read '{}': {}", input_path, e);
            process::exit(1);
        }
    };

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

    // 4. Read code files (.js, .genexpr) from input directory
    let code_files = load_code_files_from_dir(input_dir);
    let code_files_ref = if code_files.is_empty() {
        None
    } else {
        Some(&code_files)
    };

    // 5. Compile each file with the registry, code files, and UI data
    for (i, (stem, source, _)) in parsed.iter().enumerate() {
        // Load .uiflutmax sidecar file for this .flutmax file
        let ui_data = load_ui_data(&flutmax_files[i].to_string_lossy());

        let json = match flutmax_cli::compile_full_with_ui(
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
        };

        // 6. Write each .maxpat
        let output_file = Path::new(output_dir).join(format!("{}.maxpat", stem));
        let output_str = output_file.to_string_lossy().to_string();
        write_output(&output_str, &json);
        eprintln!("compiled {} -> {}", flutmax_files[i].display(), output_str);
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
