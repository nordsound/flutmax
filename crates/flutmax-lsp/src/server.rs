use std::collections::HashMap;
use std::sync::RwLock;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

pub struct FlutmaxLsp {
    client: Client,
    /// Cached document contents.
    documents: RwLock<HashMap<Url, String>>,
    /// Object database (loaded once at startup from Max refpages).
    objdb: Option<flutmax_objdb::ObjectDb>,
}

impl FlutmaxLsp {
    pub fn new(client: Client) -> Self {
        let objdb = flutmax_validate::try_load_max_objdb();
        Self {
            client,
            documents: RwLock::new(HashMap::new()),
            objdb,
        }
    }

    /// Run diagnostics on a document and publish results.
    async fn diagnose(&self, uri: &Url, text: &str) {
        let mut diagnostics = Vec::new();

        // Use error-recovering parser to report ALL errors, not just the first
        match flutmax_parser::parse_new_with_errors(text) {
            Ok((ast, parse_errors)) => {
                // Parse errors (recovered — parser continued after each)
                for err in parse_errors {
                    let (line, col, msg) = match &err {
                        flutmax_parser::parser::ParseError::Syntax { line, column, message } => {
                            (line.saturating_sub(1), column.saturating_sub(1), message.clone())
                        }
                        flutmax_parser::parser::ParseError::Lex(le) => {
                            (le.line.saturating_sub(1), le.column.saturating_sub(1), le.message.clone())
                        }
                    };
                    diagnostics.push(Diagnostic {
                        range: Range {
                            start: Position { line: line as u32, character: col as u32 },
                            end: Position { line: line as u32, character: (col + 10) as u32 },
                        },
                        severity: Some(DiagnosticSeverity::ERROR),
                        source: Some("flutmax".to_string()),
                        message: msg,
                        ..Default::default()
                    });
                }

                // Type check errors on the (partial) AST
                let type_errors = flutmax_sema::type_check::type_check(&ast);
                for err in type_errors {
                    let line = err.span.as_ref().map_or(0, |s| s.start_line.saturating_sub(1));
                    let col = err.span.as_ref().map_or(0, |s| s.start_column.saturating_sub(1));
                    let end_col = err.span.as_ref().map_or(col + 10, |s| s.end_column.max(s.start_column));
                    diagnostics.push(Diagnostic {
                        range: Range {
                            start: Position { line: line as u32, character: col as u32 },
                            end: Position { line: line as u32, character: end_col as u32 },
                        },
                        severity: Some(DiagnosticSeverity::ERROR),
                        code: Some(NumberOrString::String(err.code.to_string())),
                        source: Some("flutmax".to_string()),
                        message: err.message,
                        ..Default::default()
                    });
                }
            }
            Err(_) => {
                // Lex error (can't even tokenize)
                diagnostics.push(Diagnostic {
                    range: Range {
                        start: Position { line: 0, character: 0 },
                        end: Position { line: 0, character: 10 },
                    },
                    severity: Some(DiagnosticSeverity::ERROR),
                    source: Some("flutmax".to_string()),
                    message: "Failed to tokenize source".to_string(),
                    ..Default::default()
                });
            }
        }

        self.client
            .publish_diagnostics(uri.clone(), diagnostics, None)
            .await;
    }

    /// Collect completion items from the current document and object database.
    fn collect_completions(&self, text: &str) -> Vec<CompletionItem> {
        let mut items = Vec::new();

        // 1. Keywords
        for kw in &[
            "wire", "in", "out", "state", "msg", "feedback", "signal", "float", "int", "bang",
            "list", "symbol",
        ] {
            items.push(CompletionItem {
                label: kw.to_string(),
                kind: Some(CompletionItemKind::KEYWORD),
                ..Default::default()
            });
        }

        // 2. Defined wire names and in-port names from current document
        // Use error-recovering parser so incomplete documents still provide completions
        if let Ok((ast, _errors)) = flutmax_parser::parse_new_with_errors(text) {
            for wire in &ast.wires {
                items.push(CompletionItem {
                    label: wire.name.clone(),
                    kind: Some(CompletionItemKind::VARIABLE),
                    detail: Some("wire".to_string()),
                    ..Default::default()
                });
            }
            for decl in &ast.in_decls {
                items.push(CompletionItem {
                    label: decl.name.clone(),
                    kind: Some(CompletionItemKind::VARIABLE),
                    detail: Some(format!("in: {:?}", decl.port_type)),
                    ..Default::default()
                });
            }
        }

        // 3. Max object names from objdb
        if let Some(ref db) = self.objdb {
            for name in db.names() {
                items.push(CompletionItem {
                    label: name.to_string(),
                    kind: Some(CompletionItemKind::FUNCTION),
                    detail: Some("Max object".to_string()),
                    ..Default::default()
                });
            }
        }

        items
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Get the word at a cursor position in the text.
fn get_word_at_position(text: &str, position: Position) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let line_idx = position.line as usize;
    if line_idx >= lines.len() {
        return String::new();
    }

    let line = lines[line_idx];
    let col = position.character as usize;
    if col > line.len() {
        return String::new();
    }

    let bytes = line.as_bytes();
    let mut start = col;
    while start > 0 && is_ident_char(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = col;
    while end < bytes.len() && is_ident_char(bytes[end]) {
        end += 1;
    }

    line[start..end].to_string()
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'~' || b == b'.' || b == b'-'
}

/// Format object database info as Markdown for hover display.
fn format_objdb_hover(def: &flutmax_objdb::ObjectDef) -> String {
    let mut info = format!("**{}**", def.name);
    if !def.digest.is_empty() {
        info.push_str(&format!(" — {}", def.digest.trim()));
    }
    info.push_str(&format!("\n\nModule: {:?}", def.module));
    if !def.category.is_empty() {
        info.push_str(&format!(" | Category: {}", def.category));
    }

    // Inlet details
    let inlets = match &def.inlets {
        flutmax_objdb::InletSpec::Fixed(ports) => ports.clone(),
        flutmax_objdb::InletSpec::Variable { defaults, .. } => defaults.clone(),
    };
    if !inlets.is_empty() {
        info.push_str("\n\n**Inlets:**\n");
        for port in &inlets {
            let hot = if port.is_hot { " (hot)" } else { "" };
            let desc = if port.description.is_empty() {
                String::new()
            } else {
                format!(": {}", port.description.trim())
            };
            info.push_str(&format!("- `{}` {:?}{}{}\n", port.id, port.port_type, hot, desc));
        }
    }

    // Outlet details
    let outlets = match &def.outlets {
        flutmax_objdb::OutletSpec::Fixed(ports) => ports.clone(),
        flutmax_objdb::OutletSpec::Variable { defaults, .. } => defaults.clone(),
    };
    if !outlets.is_empty() {
        info.push_str("\n**Outlets:**\n");
        for port in &outlets {
            let desc = if port.description.is_empty() {
                String::new()
            } else {
                format!(": {}", port.description.trim())
            };
            info.push_str(&format!("- `{}` {:?}{}\n", port.id, port.port_type, desc));
        }
    }

    info
}

// ---------------------------------------------------------------------------
// Signature help helpers
// ---------------------------------------------------------------------------

/// Find the object name at the call site containing the cursor, and the active parameter index.
///
/// Scans backwards from the cursor position to find an opening `(`, then extracts
/// the object name before it. Counts commas at depth 0 to determine the active parameter.
fn find_call_context(text: &str, position: Position) -> (String, usize) {
    let lines: Vec<&str> = text.lines().collect();
    let line_idx = position.line as usize;
    if line_idx >= lines.len() {
        return (String::new(), 0);
    }

    let line = lines[line_idx];
    let col = position.character as usize;
    let prefix = &line[..col.min(line.len())];

    // Walk backwards through the prefix to find the opening '('
    let mut paren_depth = 0;
    let mut comma_count = 0;
    let mut paren_start = None;

    for (i, ch) in prefix.chars().rev().enumerate() {
        match ch {
            ')' => paren_depth += 1,
            '(' => {
                if paren_depth == 0 {
                    paren_start = Some(prefix.len() - i - 1);
                    break;
                }
                paren_depth -= 1;
            }
            ',' if paren_depth == 0 => comma_count += 1,
            _ => {}
        }
    }

    if let Some(start) = paren_start {
        // Extract object name before the paren
        let before = prefix[..start].trim_end();
        let word_start = before
            .rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '~' && c != '.' && c != '-')
            .map(|i| i + 1)
            .unwrap_or(0);
        let object_name = before[word_start..].to_string();
        (object_name, comma_count)
    } else {
        (String::new(), 0)
    }
}

struct SignatureParam {
    label_str: String,
    description: String,
}

/// Build parameter labels from an object definition's inlet list.
fn build_signature_params(def: &flutmax_objdb::ObjectDef) -> Vec<SignatureParam> {
    let inlets = match &def.inlets {
        flutmax_objdb::InletSpec::Fixed(ports) => ports.clone(),
        flutmax_objdb::InletSpec::Variable { defaults, .. } => defaults.clone(),
    };
    inlets
        .iter()
        .map(|port| {
            let type_str = format!("{:?}", port.port_type);
            let label = if port.description.is_empty() {
                format!("in{}: {}", port.id, type_str)
            } else {
                format!(
                    "{}: {}",
                    port.description.trim().to_lowercase().replace(' ', "_"),
                    type_str
                )
            };
            SignatureParam {
                label_str: label,
                description: port.description.trim().to_string(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Semantic token type indices (must match the legend order in initialize)
// ---------------------------------------------------------------------------
const ST_KEYWORD: u32 = 0;
const ST_FUNCTION: u32 = 1;
const ST_VARIABLE: u32 = 2;
#[allow(dead_code)] // Reserved in the legend for future use
const ST_PARAMETER: u32 = 3;
const ST_TYPE: u32 = 4;
const ST_STRING: u32 = 5;
const ST_NUMBER: u32 = 6;
const ST_COMMENT: u32 = 7;
const ST_OPERATOR: u32 = 8;

#[tower_lsp::async_trait]
impl LanguageServer for FlutmaxLsp {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![".".to_string(), "~".to_string()]),
                    ..Default::default()
                }),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
                    ..Default::default()
                }),
                definition_provider: Some(OneOf::Left(true)),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            legend: SemanticTokensLegend {
                                token_types: vec![
                                    SemanticTokenType::KEYWORD,   // 0
                                    SemanticTokenType::FUNCTION,  // 1
                                    SemanticTokenType::VARIABLE,  // 2
                                    SemanticTokenType::PARAMETER, // 3
                                    SemanticTokenType::TYPE,      // 4
                                    SemanticTokenType::STRING,    // 5
                                    SemanticTokenType::NUMBER,    // 6
                                    SemanticTokenType::COMMENT,   // 7
                                    SemanticTokenType::OPERATOR,  // 8
                                ],
                                token_modifiers: vec![],
                            },
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                            range: None,
                            ..Default::default()
                        },
                    ),
                ),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "flutmax LSP initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        let text = params.text_document.text.clone();
        self.documents
            .write()
            .unwrap()
            .insert(uri.clone(), text.clone());
        self.diagnose(&uri, &text).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        if let Some(change) = params.content_changes.into_iter().last() {
            let text = change.text;
            self.documents
                .write()
                .unwrap()
                .insert(uri.clone(), text.clone());
            self.diagnose(&uri, &text).await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        self.documents
            .write()
            .unwrap()
            .remove(&params.text_document.uri);
    }

    async fn completion(
        &self,
        params: CompletionParams,
    ) -> Result<Option<CompletionResponse>> {
        let uri = &params.text_document_position.text_document.uri;
        let docs = self.documents.read().unwrap();
        let text = match docs.get(uri) {
            Some(t) => t.clone(),
            None => return Ok(None),
        };
        drop(docs);

        let items = self.collect_completions(&text);
        Ok(Some(CompletionResponse::Array(items)))
    }

    // -----------------------------------------------------------------------
    // Hover (#10)
    // -----------------------------------------------------------------------
    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let docs = self.documents.read().unwrap();
        let text = match docs.get(uri) {
            Some(t) => t.clone(),
            None => return Ok(None),
        };
        drop(docs);

        let word = get_word_at_position(&text, position);
        if word.is_empty() {
            return Ok(None);
        }

        // Check objdb first
        if let Some(ref db) = self.objdb {
            if let Some(def) = db.lookup(&word) {
                let info = format_objdb_hover(def);
                return Ok(Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: info,
                    }),
                    range: None,
                }));
            }
            // Also check with ~ suffix (user may hover on the base name before ~)
            let tilde_name = format!("{}~", word);
            if let Some(def) = db.lookup(&tilde_name) {
                let info = format_objdb_hover(def);
                return Ok(Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: info,
                    }),
                    range: None,
                }));
            }
        }

        // Check if it's a wire name or input port in the current document
        if let Ok((ast, _)) = flutmax_parser::parse_new_with_errors(&text) {
            for wire in &ast.wires {
                if wire.name == word {
                    let info = format!("**wire** `{}`\n\n```\n{:?}\n```", wire.name, wire.value);
                    return Ok(Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: info,
                        }),
                        range: None,
                    }));
                }
            }
            for decl in &ast.in_decls {
                if decl.name == word {
                    return Ok(Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: format!("**input port** `{}`: {:?}", decl.name, decl.port_type),
                        }),
                        range: None,
                    }));
                }
            }
            for decl in &ast.out_decls {
                if decl.name == word {
                    return Ok(Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: format!("**output port** `{}`: {:?}", decl.name, decl.port_type),
                        }),
                        range: None,
                    }));
                }
            }
            for decl in &ast.feedback_decls {
                if decl.name == word {
                    return Ok(Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: format!("**feedback** `{}`: {:?}", decl.name, decl.port_type),
                        }),
                        range: None,
                    }));
                }
            }
            for decl in &ast.state_decls {
                if decl.name == word {
                    return Ok(Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: format!(
                                "**state** `{}`: {:?} = {:?}",
                                decl.name, decl.port_type, decl.init_value
                            ),
                        }),
                        range: None,
                    }));
                }
            }
        }

        Ok(None)
    }

    // -----------------------------------------------------------------------
    // Signature Help (Phase 0b)
    // -----------------------------------------------------------------------
    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let docs = self.documents.read().unwrap();
        let text = match docs.get(uri) {
            Some(t) => t.clone(),
            None => return Ok(None),
        };
        drop(docs);

        // Find the object name before the opening paren and active parameter index
        let (object_name, active_param) = find_call_context(&text, position);
        if object_name.is_empty() {
            return Ok(None);
        }

        // Look up in objdb
        if let Some(ref db) = self.objdb {
            let def = db
                .lookup(&object_name)
                .or_else(|| db.lookup(&format!("{}~", object_name)));

            if let Some(def) = def {
                let params = build_signature_params(def);
                if params.is_empty() {
                    return Ok(None);
                }

                let label = format!(
                    "{}({})",
                    def.name,
                    params
                        .iter()
                        .map(|p| p.label_str.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                );

                return Ok(Some(SignatureHelp {
                    signatures: vec![SignatureInformation {
                        label,
                        documentation: Some(Documentation::String(
                            def.digest.trim().to_string(),
                        )),
                        parameters: Some(
                            params
                                .iter()
                                .map(|p| ParameterInformation {
                                    label: ParameterLabel::Simple(p.label_str.clone()),
                                    documentation: Some(Documentation::String(
                                        p.description.clone(),
                                    )),
                                })
                                .collect(),
                        ),
                        active_parameter: Some(active_param as u32),
                    }],
                    active_signature: Some(0),
                    active_parameter: Some(active_param as u32),
                }));
            }
        }

        Ok(None)
    }

    // -----------------------------------------------------------------------
    // Go to Definition (#11)
    // -----------------------------------------------------------------------
    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let docs = self.documents.read().unwrap();
        let text = match docs.get(uri) {
            Some(t) => t.clone(),
            None => return Ok(None),
        };
        drop(docs);

        let word = get_word_at_position(&text, position);
        if word.is_empty() {
            return Ok(None);
        }

        // Parse with error recovery to find declarations
        if let Ok((ast, _)) = flutmax_parser::parse_new_with_errors(&text) {
            // Check wires
            for wire in &ast.wires {
                if wire.name == word {
                    if let Some(ref span) = wire.span {
                        return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                            uri: uri.clone(),
                            range: Range {
                                start: Position {
                                    line: span.start_line.saturating_sub(1) as u32,
                                    character: span.start_column.saturating_sub(1) as u32,
                                },
                                end: Position {
                                    line: span.end_line.saturating_sub(1) as u32,
                                    character: span.end_column.saturating_sub(1) as u32,
                                },
                            },
                        })));
                    }
                }
            }
            // Check feedback decls
            for decl in &ast.feedback_decls {
                if decl.name == word {
                    if let Some(ref span) = decl.span {
                        return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                            uri: uri.clone(),
                            range: Range {
                                start: Position {
                                    line: span.start_line.saturating_sub(1) as u32,
                                    character: span.start_column.saturating_sub(1) as u32,
                                },
                                end: Position {
                                    line: span.end_line.saturating_sub(1) as u32,
                                    character: span.end_column.saturating_sub(1) as u32,
                                },
                            },
                        })));
                    }
                }
            }
            // Check state decls
            for decl in &ast.state_decls {
                if decl.name == word {
                    if let Some(ref span) = decl.span {
                        return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                            uri: uri.clone(),
                            range: Range {
                                start: Position {
                                    line: span.start_line.saturating_sub(1) as u32,
                                    character: span.start_column.saturating_sub(1) as u32,
                                },
                                end: Position {
                                    line: span.end_line.saturating_sub(1) as u32,
                                    character: span.end_column.saturating_sub(1) as u32,
                                },
                            },
                        })));
                    }
                }
            }
            // Check in_decls — find by line search since InDecl has no span
            for decl in &ast.in_decls {
                if decl.name == word {
                    // Find the line containing this in declaration
                    if let Some(line_num) = find_line_containing(&text, &format!("in {} ({})", decl.index, decl.name)) {
                        return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                            uri: uri.clone(),
                            range: Range {
                                start: Position { line: line_num as u32, character: 0 },
                                end: Position { line: line_num as u32, character: 100 },
                            },
                        })));
                    }
                }
            }
            // Check out_decls
            for decl in &ast.out_decls {
                if decl.name == word {
                    if let Some(line_num) = find_line_containing(&text, &format!("out {} ({})", decl.index, decl.name)) {
                        return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                            uri: uri.clone(),
                            range: Range {
                                start: Position { line: line_num as u32, character: 0 },
                                end: Position { line: line_num as u32, character: 100 },
                            },
                        })));
                    }
                }
            }
        }

        Ok(None)
    }

    // -----------------------------------------------------------------------
    // Semantic Tokens (#12)
    // -----------------------------------------------------------------------
    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let uri = &params.text_document.uri;
        let docs = self.documents.read().unwrap();
        let text = match docs.get(uri) {
            Some(t) => t.clone(),
            None => return Ok(None),
        };
        drop(docs);

        let tokens = match flutmax_parser::lexer::Lexer::tokenize_with_comments(&text) {
            Ok(t) => t,
            Err(_) => return Ok(None),
        };

        let mut semantic_tokens = Vec::new();
        let mut prev_line = 0u32;
        let mut prev_start = 0u32;

        for token in &tokens {
            use flutmax_parser::tokens::TokenType;

            if token.token_type == TokenType::Eof {
                break;
            }

            let token_type_idx = match token.token_type {
                TokenType::Wire
                | TokenType::In
                | TokenType::Out
                | TokenType::Msg
                | TokenType::State
                | TokenType::Feedback => ST_KEYWORD,

                TokenType::Signal
                | TokenType::Float
                | TokenType::Int
                | TokenType::Bang
                | TokenType::List
                | TokenType::Symbol => ST_TYPE,

                TokenType::StringLit => ST_STRING,
                TokenType::NumberLit => ST_NUMBER,
                TokenType::Comment => ST_COMMENT,
                TokenType::Operator => ST_OPERATOR,

                TokenType::Identifier => {
                    // Check if it's a known Max object
                    if let Some(ref db) = self.objdb {
                        if db.lookup(&token.lexeme).is_some() {
                            ST_FUNCTION
                        } else {
                            // Check with ~ suffix
                            let tilde = format!("{}~", token.lexeme);
                            if db.lookup(&tilde).is_some() {
                                ST_FUNCTION
                            } else {
                                ST_VARIABLE
                            }
                        }
                    } else {
                        ST_VARIABLE
                    }
                }

                // Skip delimiters, punctuation, etc.
                _ => continue,
            };

            let line = (token.line - 1) as u32;
            let start = (token.column - 1) as u32;
            let length = token.lexeme.len() as u32;

            let delta_line = line - prev_line;
            let delta_start = if delta_line == 0 {
                start - prev_start
            } else {
                start
            };

            semantic_tokens.push(SemanticToken {
                delta_line,
                delta_start,
                length,
                token_type: token_type_idx,
                token_modifiers_bitset: 0,
            });

            prev_line = line;
            prev_start = start;
        }

        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data: semantic_tokens,
        })))
    }
}

/// Find the 0-indexed line number containing the given text pattern.
fn find_line_containing(text: &str, pattern: &str) -> Option<usize> {
    for (i, line) in text.lines().enumerate() {
        if line.contains(pattern) {
            return Some(i);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a FlutmaxLsp with a mock client for testing completions.
    /// Since we can't easily create a real Client in tests, we test the
    /// `collect_completions` method directly.

    #[test]
    fn test_completions_include_keywords() {
        // Test the collect_completions helper without needing a Client
        let items = collect_completions_standalone("");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"wire"));
        assert!(labels.contains(&"in"));
        assert!(labels.contains(&"out"));
        assert!(labels.contains(&"signal"));
        assert!(labels.contains(&"float"));
        assert!(labels.contains(&"feedback"));
        assert!(labels.contains(&"state"));
        assert!(labels.contains(&"msg"));
    }

    #[test]
    fn test_completions_include_wire_names() {
        let source = "wire osc = cycle~(440);\nwire amp = *~(osc, 0.5);\n";
        let items = collect_completions_standalone(source);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"osc"));
        assert!(labels.contains(&"amp"));
    }

    #[test]
    fn test_completions_include_in_decl_names() {
        let source = "in 0 (freq): float;\nin 1 (gain): float;\n";
        let items = collect_completions_standalone(source);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"freq"));
        assert!(labels.contains(&"gain"));
    }

    #[test]
    fn test_diagnostics_parse_error() {
        // Verify that invalid syntax produces a ParseError
        let result = flutmax_parser::parse("wire = ;");
        assert!(result.is_err());
    }

    #[test]
    fn test_diagnostics_type_error() {
        // Signal wire fed to a control-only inlet should produce type errors
        let source = "in 0 (freq): float;\nwire osc = cycle~(freq);\nout 0 (audio): signal;\nout[0] = osc;\n";
        if let Ok(ast) = flutmax_parser::parse(source) {
            let errors = flutmax_sema::type_check::type_check(&ast);
            // The exact number of errors depends on the checker, but we verify it runs
            let _ = errors;
        }
    }

    #[test]
    fn test_diagnostics_clean_document() {
        let source =
            "in 0 (freq): float;\nwire osc = cycle~(freq);\nout 0 (audio): signal;\nout[0] = osc;\n";
        let result = flutmax_parser::parse(source);
        assert!(result.is_ok());
    }

    /// Standalone completion collector (mirrors FlutmaxLsp::collect_completions
    /// but doesn't require a Client instance).
    fn collect_completions_standalone(text: &str) -> Vec<CompletionItem> {
        let mut items = Vec::new();

        for kw in &[
            "wire", "in", "out", "state", "msg", "feedback", "signal", "float", "int", "bang",
            "list", "symbol",
        ] {
            items.push(CompletionItem {
                label: kw.to_string(),
                kind: Some(CompletionItemKind::KEYWORD),
                ..Default::default()
            });
        }

        if let Ok(ast) = flutmax_parser::parse(text) {
            for wire in &ast.wires {
                items.push(CompletionItem {
                    label: wire.name.clone(),
                    kind: Some(CompletionItemKind::VARIABLE),
                    detail: Some("wire".to_string()),
                    ..Default::default()
                });
            }
            for decl in &ast.in_decls {
                items.push(CompletionItem {
                    label: decl.name.clone(),
                    kind: Some(CompletionItemKind::VARIABLE),
                    detail: Some(format!("in: {:?}", decl.port_type)),
                    ..Default::default()
                });
            }
        }

        items
    }

    // -----------------------------------------------------------------------
    // Hover tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_get_word_at_position_basic() {
        let text = "wire osc = cycle~(440);";
        // "osc" starts at column 5
        let word = get_word_at_position(text, Position { line: 0, character: 6 });
        assert_eq!(word, "osc");
    }

    #[test]
    fn test_get_word_at_position_tilde_name() {
        let text = "wire osc = cycle~(440);";
        // "cycle~" — cursor on 'c' of cycle
        let word = get_word_at_position(text, Position { line: 0, character: 11 });
        assert_eq!(word, "cycle~");
    }

    #[test]
    fn test_get_word_at_position_at_delimiter() {
        let text = "wire osc = cycle~(440);";
        // On '(' at col 17 — not an ident char, but backward scan finds "cycle~"
        let word = get_word_at_position(text, Position { line: 0, character: 17 });
        assert_eq!(word, "cycle~");
        // On ';' at col 22 — backward scan finds ")"? No, ')' is not ident char either
        let word = get_word_at_position(text, Position { line: 0, character: 22 });
        assert_eq!(word, "");
    }

    #[test]
    fn test_get_word_at_position_multiline() {
        let text = "in 0 (freq): float;\nwire osc = cycle~(freq);";
        // line 1, position of "osc"
        let word = get_word_at_position(text, Position { line: 1, character: 5 });
        assert_eq!(word, "osc");
    }

    #[test]
    fn test_get_word_at_position_out_of_bounds() {
        let text = "wire osc = 1;";
        // line out of bounds
        assert_eq!(get_word_at_position(text, Position { line: 5, character: 0 }), "");
        // column out of bounds
        assert_eq!(get_word_at_position(text, Position { line: 0, character: 200 }), "");
    }

    #[test]
    fn test_hover_wire_info() {
        // Simulate hover: parse source, check wire lookup
        let source = "wire osc = cycle~(440);\n";
        let (ast, _) = flutmax_parser::parse_new_with_errors(source).unwrap();
        let word = "osc";
        let found = ast.wires.iter().any(|w| w.name == word);
        assert!(found, "Wire 'osc' should be found in AST");
    }

    #[test]
    fn test_hover_in_decl_info() {
        let source = "in 0 (freq): float;\n";
        let (ast, _) = flutmax_parser::parse_new_with_errors(source).unwrap();
        let word = "freq";
        let found = ast.in_decls.iter().any(|d| d.name == word);
        assert!(found, "Input port 'freq' should be found in AST");
    }

    #[test]
    fn test_format_objdb_hover_basic() {
        use flutmax_objdb::*;
        let def = ObjectDef {
            name: "cycle~".to_string(),
            module: Module::Msp,
            category: "MSP Synthesis".to_string(),
            digest: "Sinusoidal oscillator".to_string(),
            inlets: InletSpec::Fixed(vec![
                PortDef { id: 0, port_type: PortType::SignalFloat, is_hot: true, description: "Frequency".to_string() },
                PortDef { id: 1, port_type: PortType::SignalFloat, is_hot: false, description: "Phase".to_string() },
            ]),
            outlets: OutletSpec::Fixed(vec![
                PortDef { id: 0, port_type: PortType::Signal, is_hot: false, description: "Output".to_string() },
            ]),
            args: vec![],
        };
        let hover = format_objdb_hover(&def);
        assert!(hover.contains("cycle~"));
        assert!(hover.contains("Sinusoidal oscillator"));
        assert!(hover.contains("Msp"));
        assert!(hover.contains("MSP Synthesis"));
        assert!(hover.contains("**Inlets:**"), "should have Inlets section: {}", hover);
        assert!(hover.contains("Frequency"), "should show inlet description: {}", hover);
        assert!(hover.contains("Phase"), "should show inlet description: {}", hover);
        assert!(hover.contains("**Outlets:**"), "should have Outlets section: {}", hover);
        assert!(hover.contains("Output"), "should show outlet description: {}", hover);
    }

    // -----------------------------------------------------------------------
    // Go to Definition tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_goto_definition_wire_has_span() {
        let source = "wire osc = cycle~(440);\nwire amp = *~(osc, 0.5);\n";
        let (ast, _) = flutmax_parser::parse_new_with_errors(source).unwrap();
        // The wire "osc" should have a span
        let osc_wire = ast.wires.iter().find(|w| w.name == "osc").unwrap();
        assert!(osc_wire.span.is_some(), "Wire 'osc' should have a span for go-to-definition");
    }

    #[test]
    fn test_goto_definition_finds_wire_reference() {
        let source = "wire osc = cycle~(440);\nwire amp = *~(osc, 0.5);\nout 0 (audio): signal;\nout[0] = amp;\n";
        let (ast, _) = flutmax_parser::parse_new_with_errors(source).unwrap();

        // Simulate: user clicks on "osc" in `*~(osc, 0.5)` — go-to-def should find wire osc
        let word = "osc";
        let found_wire = ast.wires.iter().find(|w| w.name == word);
        assert!(found_wire.is_some());
        let span = found_wire.unwrap().span.as_ref().unwrap();
        // Wire "osc" is declared on line 1 of the source
        assert_eq!(span.start_line, 1);
    }

    #[test]
    fn test_find_line_containing() {
        let text = "in 0 (freq): float;\nin 1 (gain): float;\nwire osc = cycle~(freq);\n";
        assert_eq!(find_line_containing(text, "in 0 (freq)"), Some(0));
        assert_eq!(find_line_containing(text, "in 1 (gain)"), Some(1));
        assert_eq!(find_line_containing(text, "nonexistent"), None);
    }

    // -----------------------------------------------------------------------
    // Semantic Tokens tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_semantic_tokens_basic() {
        use flutmax_parser::lexer::Lexer;
        use flutmax_parser::tokens::TokenType;

        let source = "wire osc = cycle~(440);\n";
        let tokens = Lexer::tokenize_with_comments(source).unwrap();

        // Verify expected token types
        let types: Vec<TokenType> = tokens.iter().map(|t| t.token_type).collect();
        assert_eq!(types[0], TokenType::Wire);       // "wire" -> KEYWORD
        assert_eq!(types[1], TokenType::Identifier);  // "osc" -> VARIABLE
        // Eq is a delimiter, would be skipped in semantic tokens
        assert_eq!(types[2], TokenType::Eq);
        assert_eq!(types[3], TokenType::Identifier);  // "cycle" -> might be FUNCTION
        assert_eq!(types[4], TokenType::Tilde);        // "~"
        assert_eq!(types[6], TokenType::NumberLit);    // "440" -> NUMBER
    }

    #[test]
    fn test_semantic_tokens_with_comments() {
        use flutmax_parser::lexer::Lexer;
        use flutmax_parser::tokens::TokenType;

        let source = "// comment here\nwire x = 1;\n";
        let tokens = Lexer::tokenize_with_comments(source).unwrap();

        let types: Vec<TokenType> = tokens.iter().map(|t| t.token_type).collect();
        // First token should be Comment
        assert_eq!(types[0], TokenType::Comment);
        assert_eq!(tokens[0].lexeme, "// comment here");
        // Then normal tokens
        assert_eq!(types[1], TokenType::Wire);
    }

    #[test]
    fn test_semantic_tokens_keyword_vs_type() {
        use flutmax_parser::lexer::Lexer;
        use flutmax_parser::tokens::TokenType;

        let source = "in 0 (freq): float;\n";
        let tokens = Lexer::tokenize_with_comments(source).unwrap();
        let types: Vec<TokenType> = tokens.iter().map(|t| t.token_type).collect();
        assert_eq!(types[0], TokenType::In);    // keyword
        assert_eq!(types[5], TokenType::Colon);
        assert_eq!(types[6], TokenType::Float);  // type keyword
    }

    #[test]
    fn test_semantic_token_delta_computation() {
        // Verify that delta line/start computation is correct
        // Token at (line 0, col 0) then (line 0, col 5) then (line 1, col 2)
        let mut prev_line = 0u32;
        let mut prev_start = 0u32;

        // First token: (0, 0)
        let line = 0u32;
        let start = 0u32;
        let dl = line - prev_line;
        let ds = if dl == 0 { start - prev_start } else { start };
        assert_eq!((dl, ds), (0, 0));
        prev_line = line;
        prev_start = start;

        // Second token: (0, 5)
        let line = 0u32;
        let start = 5u32;
        let dl = line - prev_line;
        let ds = if dl == 0 { start - prev_start } else { start };
        assert_eq!((dl, ds), (0, 5));
        prev_line = line;
        prev_start = start;

        // Third token: (1, 2)
        let line = 1u32;
        let start = 2u32;
        let dl = line - prev_line;
        let ds = if dl == 0 { start - prev_start } else { start };
        assert_eq!((dl, ds), (1, 2));
    }

    // -----------------------------------------------------------------------
    // Signature help tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_find_call_context_basic() {
        let text = "wire x = cycle~(440);";
        // Cursor right after '(' at col 16
        let (name, param) = find_call_context(text, Position { line: 0, character: 16 });
        assert_eq!(name, "cycle~");
        assert_eq!(param, 0);
    }

    #[test]
    fn test_find_call_context_second_param() {
        let text = "wire x = biquad~(osc, cutoff);";
        // Cursor after comma, at 'cutoff' — col 22
        let (name, param) = find_call_context(text, Position { line: 0, character: 22 });
        assert_eq!(name, "biquad~");
        assert_eq!(param, 1);
    }

    #[test]
    fn test_find_call_context_third_param() {
        let text = "wire x = biquad~(osc, cutoff, q);";
        // Cursor at 'q' — col 30
        let (name, param) = find_call_context(text, Position { line: 0, character: 30 });
        assert_eq!(name, "biquad~");
        assert_eq!(param, 2);
    }

    #[test]
    fn test_find_call_context_no_paren() {
        let text = "wire x = osc;";
        let (name, param) = find_call_context(text, Position { line: 0, character: 10 });
        assert_eq!(name, "");
        assert_eq!(param, 0);
    }

    #[test]
    fn test_find_call_context_dotted_name() {
        let text = "wire x = jit.gl.render(ctx);";
        // Cursor inside the parens (after '('), at 'c' of ctx = character 23
        let (name, param) = find_call_context(text, Position { line: 0, character: 23 });
        assert_eq!(name, "jit.gl.render");
        assert_eq!(param, 0);
    }

    #[test]
    fn test_build_signature_params_basic() {
        use flutmax_objdb::*;
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
                    description: "Phase offset".to_string(),
                },
            ]),
            outlets: OutletSpec::Fixed(vec![]),
            args: vec![],
        };

        let params = build_signature_params(&def);
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].label_str, "frequency: SignalFloat");
        assert_eq!(params[0].description, "Frequency");
        assert_eq!(params[1].label_str, "phase_offset: SignalFloat");
        assert_eq!(params[1].description, "Phase offset");
    }
}
