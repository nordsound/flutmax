/// Hand-written recursive descent parser for flutmax.
///
/// Consumes a token stream from the lexer and produces
/// `flutmax_ast::Program` — the exact same AST as the tree-sitter parser.
use crate::lexer::{LexError, Lexer};
use crate::tokens::{Token, TokenType};
use flutmax_ast::{
    AttrPair, AttrValue, CallArg, DestructuringWire, DirectConnection, Expr, FeedbackAssignment,
    FeedbackDecl, InDecl, InputPortAccess, LitValue, MsgDecl, OutAssignment, OutDecl,
    OutputPortAccess, PortType, Program, Span, StateAssignment, StateDecl, Wire,
};

// ────────────────────────────────────────────────────────────
// Error type
// ────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum ParseError {
    Lex(LexError),
    Syntax {
        message: String,
        line: usize,
        column: usize,
    },
}

impl From<LexError> for ParseError {
    fn from(e: LexError) -> Self {
        ParseError::Lex(e)
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Lex(e) => write!(f, "{}", e),
            ParseError::Syntax {
                message,
                line,
                column,
            } => write!(f, "Parse error at {}:{}: {}", line, column, message),
        }
    }
}

impl std::error::Error for ParseError {}

// ────────────────────────────────────────────────────────────
// Parser
// ────────────────────────────────────────────────────────────

pub struct FlutmaxParser {
    tokens: Vec<Token>,
    pos: usize,
    /// Counter for implicit `in` port indices.
    implicit_in_index: u32,
    /// Counter for implicit `out` port indices.
    implicit_out_index: u32,
}

impl FlutmaxParser {
    /// Parse a .flutmax source string into a Program.
    /// Returns the first error encountered (for backward compatibility).
    pub fn parse(source: &str) -> Result<Program, ParseError> {
        let (program, errors) = Self::parse_with_errors(source)?;
        if let Some(first_err) = errors.into_iter().next() {
            return Err(first_err);
        }
        Ok(program)
    }

    /// Parse with error recovery: returns a (possibly partial) Program and all errors.
    /// Continues parsing after errors by skipping to the next semicolon.
    pub fn parse_with_errors(source: &str) -> Result<(Program, Vec<ParseError>), ParseError> {
        let tokens = Lexer::tokenize(source)?;
        let mut parser = FlutmaxParser {
            tokens,
            pos: 0,
            implicit_in_index: 0,
            implicit_out_index: 0,
        };
        let (program, errors) = parser.parse_program_recovering();
        Ok((program, errors))
    }

    // ── Helpers ──────────────────────────────────────────────

    fn peek(&self) -> &Token {
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    fn peek_type(&self) -> TokenType {
        self.peek().token_type
    }

    fn peek_at(&self, offset: usize) -> &Token {
        let idx = (self.pos + offset).min(self.tokens.len() - 1);
        &self.tokens[idx]
    }

    fn advance(&mut self) -> &Token {
        let tok = &self.tokens[self.pos.min(self.tokens.len() - 1)];
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn expect(&mut self, tt: TokenType) -> Result<Token, ParseError> {
        let tok = self.peek().clone();
        if tok.token_type == tt {
            self.advance();
            Ok(tok)
        } else {
            Err(self.error(format!(
                "Expected {:?}, got {:?} '{}'",
                tt, tok.token_type, tok.lexeme
            )))
        }
    }

    fn check(&self, tt: TokenType) -> bool {
        self.peek_type() == tt
    }

    fn error(&self, message: String) -> ParseError {
        let tok = self.peek();
        ParseError::Syntax {
            message,
            line: tok.line,
            column: tok.column,
        }
    }

    /// Check if the current token starts a new statement (for error recovery).
    fn is_statement_start(&self) -> bool {
        matches!(
            self.peek_type(),
            TokenType::Wire
                | TokenType::In
                | TokenType::Out
                | TokenType::Msg
                | TokenType::State
                | TokenType::Feedback
        )
    }

    fn make_span_from(&self, start: &Token, end: &Token) -> Option<Span> {
        Some(Span {
            start_line: start.line,
            start_column: start.column,
            end_line: end.line,
            end_column: end.column + end.lexeme.len(),
        })
    }

    /// Look at the previous token (the one just consumed).
    fn previous(&self) -> &Token {
        &self.tokens[(self.pos - 1).max(0)]
    }

    // ── Top-level ────────────────────────────────────────────

    /// Parse with error recovery: skip to next statement boundary on error and continue.
    fn parse_program_recovering(&mut self) -> (Program, Vec<ParseError>) {
        let mut program = Program::new();
        let mut errors = Vec::new();

        while !self.check(TokenType::Eof) {
            match self.parse_statement(&mut program) {
                Ok(()) => {}
                Err(e) => {
                    errors.push(e);
                    // Skip to next statement boundary: `;` or a statement-starting keyword
                    while !self.check(TokenType::Eof)
                        && !self.check(TokenType::Semicolon)
                        && !self.is_statement_start()
                    {
                        self.advance();
                    }
                    if self.check(TokenType::Semicolon) {
                        self.advance(); // consume the ;
                    }
                    // Don't consume statement-starting keywords — they begin the next statement
                }
            }
        }

        (program, errors)
    }

    /// Parse a single top-level statement and add it to the Program.
    fn parse_statement(&mut self, program: &mut Program) -> Result<(), ParseError> {
        match self.peek_type() {
            TokenType::Wire => self.parse_wire_or_destructuring(program),
            TokenType::In => self.parse_in_decl_or_nothing(program),
            TokenType::Out => self.parse_out_decl_or_assignment(program),
            TokenType::Msg => self.parse_msg_declaration(program),
            TokenType::Feedback => self.parse_feedback(program),
            TokenType::State => self.parse_state(program),
            TokenType::Identifier => self.parse_direct_connection(program),
            _ => {
                let tok = self.peek().clone();
                Err(self.error(format!(
                    "Unexpected token {:?} '{}'",
                    tok.token_type, tok.lexeme
                )))
            }
        }
    }

    // ── In declaration ───────────────────────────────────────
    // Explicit: `in 0 (freq): float;`
    // Implicit: `in freq: float;`

    fn parse_in_decl_or_nothing(&mut self, program: &mut Program) -> Result<(), ParseError> {
        // `in` keyword
        self.expect(TokenType::In)?;

        if self.check(TokenType::NumberLit) {
            // Explicit: in 0 (name): type;
            let index = self.parse_integer()?;
            self.expect(TokenType::LParen)?;
            let name = self.expect_identifier()?;
            self.expect(TokenType::RParen)?;
            self.expect(TokenType::Colon)?;
            let port_type = self.parse_port_type()?;
            self.expect(TokenType::Semicolon)?;

            program.in_decls.push(InDecl {
                index,
                name,
                port_type,
            });
        } else {
            // Implicit: in name: type;
            let name = self.expect_identifier()?;
            self.expect(TokenType::Colon)?;
            let port_type = self.parse_port_type()?;
            self.expect(TokenType::Semicolon)?;

            let index = self.implicit_in_index;
            self.implicit_in_index += 1;

            program.in_decls.push(InDecl {
                index,
                name,
                port_type,
            });
        }

        Ok(())
    }

    // ── Out declaration or assignment ────────────────────────
    // Explicit: `out 0 (audio): signal;`
    // Implicit: `out audio: signal;`
    // Assignment: `out[0] = osc;`

    fn parse_out_decl_or_assignment(&mut self, program: &mut Program) -> Result<(), ParseError> {
        let start_tok = self.peek().clone();
        self.expect(TokenType::Out)?;

        if self.check(TokenType::LBracket) {
            // out[0] = expr;
            self.expect(TokenType::LBracket)?;
            let index = self.parse_integer()?;
            self.expect(TokenType::RBracket)?;
            self.expect(TokenType::Eq)?;
            let value = self.parse_expression()?;
            let end_tok = self.expect(TokenType::Semicolon)?;

            program.out_assignments.push(OutAssignment {
                index,
                value,
                span: self.make_span_from(&start_tok, &end_tok),
            });
        } else if self.check(TokenType::NumberLit) {
            // Explicit: out 0 (name): type; or out 0 (name): type = expr;
            let index = self.parse_integer()?;
            self.expect(TokenType::LParen)?;
            let name = self.expect_identifier()?;
            self.expect(TokenType::RParen)?;
            self.expect(TokenType::Colon)?;
            let port_type = self.parse_port_type()?;
            let value = if self.check(TokenType::Eq) {
                self.advance(); // consume =
                Some(self.parse_expression()?)
            } else {
                None
            };
            self.expect(TokenType::Semicolon)?;

            program.out_decls.push(OutDecl {
                index,
                name,
                port_type,
                value,
            });
        } else {
            // Implicit: out name: type; or out name: type = expr;
            let name = self.expect_identifier()?;
            self.expect(TokenType::Colon)?;
            let port_type = self.parse_port_type()?;
            let value = if self.check(TokenType::Eq) {
                self.advance(); // consume =
                Some(self.parse_expression()?)
            } else {
                None
            };
            self.expect(TokenType::Semicolon)?;

            let index = self.implicit_out_index;
            self.implicit_out_index += 1;

            program.out_decls.push(OutDecl {
                index,
                name,
                port_type,
                value,
            });
        }

        Ok(())
    }

    // ── Wire or destructuring wire ───────────────────────────
    // `wire osc = cycle~(440);`
    // `wire (a, b) = unpack(x);`

    fn parse_wire_or_destructuring(&mut self, program: &mut Program) -> Result<(), ParseError> {
        let start_tok = self.peek().clone();
        self.expect(TokenType::Wire)?;

        if self.check(TokenType::LParen) {
            // Destructuring wire: `wire (a, b, c) = expr;`
            self.expect(TokenType::LParen)?;
            let mut names = Vec::new();
            names.push(self.expect_identifier()?);
            while self.check(TokenType::Comma) {
                self.advance(); // consume `,`
                names.push(self.expect_identifier()?);
            }
            self.expect(TokenType::RParen)?;
            self.expect(TokenType::Eq)?;
            let value = self.parse_expression()?;
            let end_tok = self.expect(TokenType::Semicolon)?;

            program.destructuring_wires.push(DestructuringWire {
                names,
                value,
                span: self.make_span_from(&start_tok, &end_tok),
            });
        } else {
            // Regular wire: `wire name = expr (.attr(...))?;`
            let name = self.expect_identifier()?;
            self.expect(TokenType::Eq)?;
            let value = self.parse_expression()?;

            // Optional attr chain
            let attrs = if self.check(TokenType::DotAttrLParen) {
                self.parse_attr_chain()?
            } else {
                vec![]
            };

            let end_tok = self.expect(TokenType::Semicolon)?;

            program.wires.push(Wire {
                name,
                value,
                span: self.make_span_from(&start_tok, &end_tok),
                attrs,
            });
        }

        Ok(())
    }

    // ── Message declaration ──────────────────────────────────
    // `msg click = "bang";`
    // `msg click = "bang".attr(key: val);`

    fn parse_msg_declaration(&mut self, program: &mut Program) -> Result<(), ParseError> {
        let start_tok = self.peek().clone();
        self.expect(TokenType::Msg)?;
        let name = self.expect_identifier()?;
        self.expect(TokenType::Eq)?;

        let content_tok = self.expect(TokenType::StringLit)?;
        let content = unescape_string_content(&content_tok.lexeme);

        let attrs = if self.check(TokenType::DotAttrLParen) {
            self.parse_attr_chain()?
        } else {
            vec![]
        };

        let end_tok = self.expect(TokenType::Semicolon)?;

        program.msg_decls.push(MsgDecl {
            name,
            content,
            span: self.make_span_from(&start_tok, &end_tok),
            attrs,
        });
        Ok(())
    }

    // ── Feedback ─────────────────────────────────────────────
    // `feedback fb: signal;`           — declaration
    // `feedback fb = tapin~(mixed);`   — assignment

    fn parse_feedback(&mut self, program: &mut Program) -> Result<(), ParseError> {
        let start_tok = self.peek().clone();
        self.expect(TokenType::Feedback)?;
        let name = self.expect_identifier()?;

        if self.check(TokenType::Colon) {
            // Declaration: `feedback name: type;`
            self.advance(); // consume `:`
            let port_type = self.parse_port_type()?;
            let end_tok = self.expect(TokenType::Semicolon)?;

            program.feedback_decls.push(FeedbackDecl {
                name,
                port_type,
                span: self.make_span_from(&start_tok, &end_tok),
            });
        } else {
            // Assignment: `feedback name = expr;`
            self.expect(TokenType::Eq)?;
            let value = self.parse_expression()?;
            let end_tok = self.expect(TokenType::Semicolon)?;

            program.feedback_assignments.push(FeedbackAssignment {
                target: name,
                value,
                span: self.make_span_from(&start_tok, &end_tok),
            });
        }

        Ok(())
    }

    // ── State ────────────────────────────────────────────────
    // `state counter: int = 0;`    — declaration
    // `state counter = next;`      — assignment

    fn parse_state(&mut self, program: &mut Program) -> Result<(), ParseError> {
        let start_tok = self.peek().clone();
        self.expect(TokenType::State)?;
        let name = self.expect_identifier()?;

        if self.check(TokenType::Colon) {
            // Declaration: `state name: type = init;`
            self.advance(); // consume `:`
            let port_type = self.parse_control_type()?;
            self.expect(TokenType::Eq)?;
            let init_value = self.parse_expression()?;
            let end_tok = self.expect(TokenType::Semicolon)?;

            program.state_decls.push(StateDecl {
                name,
                port_type,
                init_value,
                span: self.make_span_from(&start_tok, &end_tok),
            });
        } else {
            // Assignment: `state name = expr;`
            self.expect(TokenType::Eq)?;
            let value = self.parse_expression()?;
            let end_tok = self.expect(TokenType::Semicolon)?;

            program.state_assignments.push(StateAssignment {
                name,
                value,
                span: self.make_span_from(&start_tok, &end_tok),
            });
        }

        Ok(())
    }

    // ── Direct connection ────────────────────────────────────
    // `node_a.in[0] = trigger;`
    // The leading identifier has already been peeked.

    fn parse_direct_connection(&mut self, program: &mut Program) -> Result<(), ParseError> {
        let object = self.expect_identifier()?;
        self.expect(TokenType::Dot)?;
        self.expect(TokenType::In)?;
        // [index] is optional — defaults to 0 (hot inlet)
        let index = if self.check(TokenType::LBracket) {
            self.advance(); // consume '['
            let idx = self.parse_integer()?;
            self.expect(TokenType::RBracket)?;
            idx
        } else {
            0
        };
        self.expect(TokenType::Eq)?;
        let value = self.parse_expression()?;
        self.expect(TokenType::Semicolon)?;

        program.direct_connections.push(DirectConnection {
            target: InputPortAccess { object, index },
            value,
        });
        Ok(())
    }

    // ── Expressions ──────────────────────────────────────────

    /// Parse an expression:
    /// - call_expr: `object_name(args)`
    /// - output_port_access: `node.out[N]`
    /// - tuple: `(x, y, z)` (2+ elements)
    /// - ref: plain identifier (possibly dotted like `jit.gl.render` used as ref)
    /// - literal: number, string
    fn parse_expression(&mut self) -> Result<Expr, ParseError> {
        match self.peek_type() {
            TokenType::LParen => self.parse_tuple_expression(),
            TokenType::NumberLit => self.parse_number_literal(),
            TokenType::StringLit => self.parse_string_literal(),
            TokenType::Identifier | TokenType::Operator => self.parse_name_based_expression(),
            // Keywords used as identifiers in expression context:
            // e.g., thispatcher(int), poly~(in), pack(float, float)
            TokenType::In
            | TokenType::Out
            | TokenType::Int
            | TokenType::Float
            | TokenType::Bang
            | TokenType::List
            | TokenType::Symbol
            | TokenType::Signal
            | TokenType::State
            | TokenType::Msg
            | TokenType::Feedback
            | TokenType::Wire => self.parse_name_based_expression(),
            _ => {
                let tok = self.peek().clone();
                Err(self.error(format!(
                    "Expected expression, got {:?} '{}'",
                    tok.token_type, tok.lexeme
                )))
            }
        }
    }

    /// Parse expressions starting with an identifier or operator:
    /// Handles object calls, output port access, and simple refs.
    fn parse_name_based_expression(&mut self) -> Result<Expr, ParseError> {
        // Consume the full object/ref name: `ident(.ident)*` or `operator`
        // Then check for `~`, then `(` for call, `.out[` for port access, or just ref.
        let name = self.parse_object_name()?;

        if self.check(TokenType::LParen) {
            // Call expression: `name(args)`
            self.advance(); // consume `(`
            let args = if self.check(TokenType::RParen) {
                vec![]
            } else {
                self.parse_argument_list()?
            };
            self.expect(TokenType::RParen)?;
            Ok(Expr::Call { object: name, args })
        } else if self.check(TokenType::Dot) {
            // Could be `.out[N]` for output port access
            // We already consumed the name; peek ahead to see if it's `.out[`
            if self.peek_at(1).token_type == TokenType::Out
                && self.peek_at(2).token_type == TokenType::LBracket
            {
                self.advance(); // consume `.`
                self.advance(); // consume `out`
                self.expect(TokenType::LBracket)?;
                let index = self.parse_integer()?;
                self.expect(TokenType::RBracket)?;
                Ok(Expr::OutputPortAccess(OutputPortAccess {
                    object: name,
                    index,
                }))
            } else {
                // Not port access — it's a dotted identifier that continues
                // This shouldn't happen because parse_object_name already consumed dots
                Ok(Expr::Ref(name))
            }
        } else {
            // Simple ref
            Ok(Expr::Ref(name))
        }
    }

    /// Parse an object/identifier name, including dots and tilde:
    /// `identifier ("." segment)* "~"?`
    /// Also handles operator names like `?`, `*`, `+`, etc.
    ///
    /// Stops consuming dots before `.in[`, `.out[`, `.attr(`.
    fn parse_object_name(&mut self) -> Result<String, ParseError> {
        let mut name = String::new();

        match self.peek_type() {
            TokenType::Identifier | TokenType::Operator => {
                name.push_str(&self.advance().lexeme.clone());
            }
            // Keywords used as object/reference names in expression context
            TokenType::In
            | TokenType::Out
            | TokenType::Int
            | TokenType::Float
            | TokenType::Bang
            | TokenType::List
            | TokenType::Symbol
            | TokenType::Signal
            | TokenType::State
            | TokenType::Msg
            | TokenType::Feedback
            | TokenType::Wire => {
                name.push_str(&self.advance().lexeme.clone());
            }
            _ => {
                return Err(self.error(format!(
                    "Expected identifier or operator, got {:?}",
                    self.peek_type()
                )));
            }
        }

        // Consume dotted segments: `.segment`
        // Stop before `.in[`, `.out[`, `.attr(`
        while self.check(TokenType::Dot) {
            // Peek at what follows the dot
            let after_dot = &self.peek_at(1);
            match after_dot.token_type {
                TokenType::In => {
                    // `.in[` → stop, it's port access (handled by caller)
                    if self.peek_at(2).token_type == TokenType::LBracket {
                        break;
                    }
                    // `.in` not followed by `[` — treat as dotted segment (unusual but possible)
                    self.advance(); // consume `.`
                    name.push('.');
                    name.push_str(&self.advance().lexeme.clone());
                }
                TokenType::Out => {
                    // `.out[` → stop, it's port access
                    if self.peek_at(2).token_type == TokenType::LBracket {
                        break;
                    }
                    self.advance(); // consume `.`
                    name.push('.');
                    name.push_str(&self.advance().lexeme.clone());
                }
                TokenType::Identifier => {
                    self.advance(); // consume `.`
                    name.push('.');
                    name.push_str(&self.advance().lexeme.clone());
                }
                TokenType::NumberLit => {
                    // Digit-starting segment: `jit.3m`, `omx.5band~`
                    // The lexer may have emitted this as a separate NumberLit token
                    // followed by an Identifier for the rest.
                    self.advance(); // consume `.`
                    name.push('.');
                    // Consume the number part
                    let num_part = self.advance().lexeme.clone();
                    name.push_str(&num_part);
                    // If immediately followed by an identifier (no space), consume it too
                    // e.g. `3` followed by `m` → `3m`
                    if self.check(TokenType::Identifier) {
                        // Check if they are adjacent (no whitespace between)
                        let prev_end = self.tokens[self.pos - 1].column
                            + self.tokens[self.pos - 1].lexeme.len();
                        let next_start = self.peek().column;
                        if prev_end == next_start
                            && self.tokens[self.pos - 1].line == self.peek().line
                        {
                            name.push_str(&self.advance().lexeme.clone());
                        }
                    }
                }
                TokenType::Operator => {
                    // Operator segment in dotted name: `mc.+~`, `mc.*~`
                    self.advance(); // consume `.`
                    name.push('.');
                    name.push_str(&self.advance().lexeme.clone());
                }
                // Keywords as dotted segments: `jit.bang`, `live.float`, etc.
                // Note: In/Out are handled above (with `.in[`/`.out[` lookahead)
                TokenType::Bang
                | TokenType::Float
                | TokenType::Int
                | TokenType::List
                | TokenType::Symbol
                | TokenType::Signal
                | TokenType::State
                | TokenType::Msg
                | TokenType::Feedback
                | TokenType::Wire => {
                    self.advance(); // consume `.`
                    name.push('.');
                    name.push_str(&self.advance().lexeme.clone());
                }
                _ => break,
            }
        }

        // Optional `=` suffix on dotted names (e.g., `gbr.wind=`)
        // Only when `=` is adjacent (no space) and followed by `(` (call context)
        if self.check(TokenType::Eq) && name.contains('.') {
            let prev_end = self.previous().column + self.previous().lexeme.len();
            let eq_col = self.peek().column;
            if prev_end == eq_col
                && self.previous().line == self.peek().line
                && self.peek_at(1).token_type == TokenType::LParen
            {
                self.advance(); // consume `=`
                name.push('=');
            }
        }

        // Optional tilde suffix
        if self.check(TokenType::Tilde) {
            // Check adjacency: tilde must be immediately after the name
            let prev_end_col = self.previous().column + self.previous().lexeme.len();
            let tilde_col = self.peek().column;
            if prev_end_col == tilde_col && self.previous().line == self.peek().line {
                self.advance(); // consume `~`
                name.push('~');
            }
        }

        Ok(name)
    }

    /// Parse a comma-separated list of call arguments (positional or named).
    fn parse_argument_list(&mut self) -> Result<Vec<CallArg>, ParseError> {
        let mut args = Vec::new();
        args.push(self.parse_call_arg()?);
        while self.check(TokenType::Comma) {
            self.advance(); // consume `,`
            args.push(self.parse_call_arg()?);
        }
        Ok(args)
    }

    /// Parse a single call argument: either `name: expr` (named) or `expr` (positional).
    fn parse_call_arg(&mut self) -> Result<CallArg, ParseError> {
        // Check for named argument: identifier followed by ':'
        // We must ensure this isn't confused with port type declarations.
        if self.check_named_arg_ahead() {
            let name = self.expect_identifier()?;
            self.expect(TokenType::Colon)?;
            let value = self.parse_expression()?;
            Ok(CallArg::named(name, value))
        } else {
            let value = self.parse_expression()?;
            Ok(CallArg::positional(value))
        }
    }

    /// Look ahead to detect `identifier ":"` pattern for named arguments.
    /// Returns true if current token is an identifier (or contextual identifier)
    /// followed by a colon, AND the token after the colon is NOT a port type keyword
    /// (to avoid confusing `in freq: float;` with named args — though that
    /// context doesn't arise inside call argument lists).
    fn check_named_arg_ahead(&self) -> bool {
        let cur = self.peek_type();
        if cur != TokenType::Identifier && !is_contextual_identifier(cur) {
            return false;
        }
        if self.peek_at(1).token_type != TokenType::Colon {
            return false;
        }
        true
    }

    /// Parse a tuple expression: `(expr, expr, ...)`
    /// Must have at least 2 elements.
    fn parse_tuple_expression(&mut self) -> Result<Expr, ParseError> {
        self.expect(TokenType::LParen)?;
        let mut elements = Vec::new();
        elements.push(self.parse_expression()?);

        // Must have at least one comma for it to be a tuple
        if !self.check(TokenType::Comma) {
            return Err(self.error("Tuple must have at least 2 elements".to_string()));
        }
        while self.check(TokenType::Comma) {
            self.advance();
            elements.push(self.parse_expression()?);
        }
        self.expect(TokenType::RParen)?;

        Ok(Expr::Tuple(elements))
    }

    fn parse_number_literal(&mut self) -> Result<Expr, ParseError> {
        let tok = self.expect(TokenType::NumberLit)?;
        let text = &tok.lexeme;

        if text.contains('.') || text.contains('e') || text.contains('E') {
            let val: f64 = text.parse().map_err(|_| ParseError::Syntax {
                message: format!("Invalid float literal '{}'", text),
                line: tok.line,
                column: tok.column,
            })?;
            Ok(Expr::Lit(LitValue::Float(val)))
        } else {
            let val: i64 = text.parse().map_err(|_| ParseError::Syntax {
                message: format!("Invalid integer literal '{}'", text),
                line: tok.line,
                column: tok.column,
            })?;
            Ok(Expr::Lit(LitValue::Int(val)))
        }
    }

    fn parse_string_literal(&mut self) -> Result<Expr, ParseError> {
        let tok = self.expect(TokenType::StringLit)?;
        let content = unescape_string_content(&tok.lexeme);
        Ok(Expr::Lit(LitValue::Str(content)))
    }

    // ── Attribute chain ──────────────────────────────────────
    // `.attr(key: value, ...)`

    fn parse_attr_chain(&mut self) -> Result<Vec<AttrPair>, ParseError> {
        self.expect(TokenType::DotAttrLParen)?;
        let mut pairs = Vec::new();

        if !self.check(TokenType::RParen) {
            pairs.push(self.parse_attr_pair()?);
            while self.check(TokenType::Comma) {
                self.advance();
                pairs.push(self.parse_attr_pair()?);
            }
        }

        self.expect(TokenType::RParen)?;
        Ok(pairs)
    }

    fn parse_attr_pair(&mut self) -> Result<AttrPair, ParseError> {
        let key = self.expect_identifier()?;
        self.expect(TokenType::Colon)?;
        let value = self.parse_attr_value()?;
        Ok(AttrPair { key, value })
    }

    fn parse_attr_value(&mut self) -> Result<AttrValue, ParseError> {
        match self.peek_type() {
            TokenType::NumberLit => {
                let tok = self.advance().clone();
                let text = &tok.lexeme;
                if text.contains('.') || text.contains('e') || text.contains('E') {
                    let val: f64 = text.parse().map_err(|_| ParseError::Syntax {
                        message: format!("Invalid float literal '{}'", text),
                        line: tok.line,
                        column: tok.column,
                    })?;
                    Ok(AttrValue::Float(val))
                } else {
                    let val: i64 = text.parse().map_err(|_| ParseError::Syntax {
                        message: format!("Invalid integer literal '{}'", text),
                        line: tok.line,
                        column: tok.column,
                    })?;
                    Ok(AttrValue::Int(val))
                }
            }
            TokenType::StringLit => {
                let tok = self.advance().clone();
                let content = unescape_string_content(&tok.lexeme);
                Ok(AttrValue::Str(content))
            }
            TokenType::Identifier => {
                let tok = self.advance().clone();
                Ok(AttrValue::Ident(tok.lexeme))
            }
            _ => Err(self.error(format!(
                "Expected attribute value (number, string, or identifier), got {:?}",
                self.peek_type()
            ))),
        }
    }

    // ── Utility parsers ──────────────────────────────────────

    /// Parse an integer from a NumberLit token.
    fn parse_integer(&mut self) -> Result<u32, ParseError> {
        let tok = self.expect(TokenType::NumberLit)?;
        tok.lexeme.parse().map_err(|_| ParseError::Syntax {
            message: format!("Expected integer, got '{}'", tok.lexeme),
            line: tok.line,
            column: tok.column,
        })
    }

    /// Expect an identifier token and return its text.
    /// Accepts `Identifier` token type, and also keyword tokens when used
    /// in identifier position (e.g., `wire msg = ...` where `msg` is both
    /// a keyword and a valid wire name).
    fn expect_identifier(&mut self) -> Result<String, ParseError> {
        let tok = self.peek().clone();
        if tok.token_type == TokenType::Identifier || is_contextual_identifier(tok.token_type) {
            self.advance();
            Ok(tok.lexeme)
        } else {
            Err(self.error(format!(
                "Expected identifier, got {:?} '{}'",
                tok.token_type, tok.lexeme
            )))
        }
    }

    /// Parse a port type keyword: signal, float, int, bang, list, symbol.
    fn parse_port_type(&mut self) -> Result<PortType, ParseError> {
        let tok = self.peek().clone();
        let pt = match tok.token_type {
            TokenType::Signal => PortType::Signal,
            TokenType::Float => PortType::Float,
            TokenType::Int => PortType::Int,
            TokenType::Bang => PortType::Bang,
            TokenType::List => PortType::List,
            TokenType::Symbol => PortType::Symbol,
            _ => {
                return Err(self.error(format!(
                    "Expected port type (signal/float/int/bang/list/symbol), got '{}'",
                    tok.lexeme
                )));
            }
        };
        self.advance();
        Ok(pt)
    }

    /// Parse a control type keyword (same as port_type but without signal).
    fn parse_control_type(&mut self) -> Result<PortType, ParseError> {
        let tok = self.peek().clone();
        let pt = match tok.token_type {
            TokenType::Float => PortType::Float,
            TokenType::Int => PortType::Int,
            TokenType::Bang => PortType::Bang,
            TokenType::List => PortType::List,
            TokenType::Symbol => PortType::Symbol,
            _ => {
                return Err(self.error(format!(
                    "Expected control type (float/int/bang/list/symbol), got '{}'",
                    tok.lexeme
                )));
            }
        };
        self.advance();
        Ok(pt)
    }
}

// ────────────────────────────────────────────────────────────
// Contextual identifiers
// ────────────────────────────────────────────────────────────

/// Keywords that can also be used as identifiers in certain contexts
/// (e.g., wire names, object names, attribute keys).
/// This excludes structural keywords like `wire`, `in`, `out`, `state`, `feedback`
/// which have unambiguous syntactic roles at statement level.
fn is_contextual_identifier(tt: TokenType) -> bool {
    matches!(
        tt,
        TokenType::Msg
            | TokenType::Signal
            | TokenType::Float
            | TokenType::Int
            | TokenType::Bang
            | TokenType::List
            | TokenType::Symbol
            | TokenType::State
            | TokenType::Feedback
            | TokenType::In
            | TokenType::Out
            | TokenType::Wire
    )
}

// ────────────────────────────────────────────────────────────
// String unescaping
// ────────────────────────────────────────────────────────────

/// Strip surrounding quotes from a string token lexeme and unescape.
fn unescape_string_content(lexeme: &str) -> String {
    // Remove surrounding quotes
    let inner = &lexeme[1..lexeme.len() - 1];
    let mut result = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some('\\') => result.push('\\'),
                Some('"') => result.push('"'),
                Some(other) => {
                    result.push('\\');
                    result.push(other);
                }
                None => result.push('\\'),
            }
        } else {
            result.push(ch);
        }
    }
    result
}

// ────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Shorthand
    fn parse(source: &str) -> Program {
        FlutmaxParser::parse(source).expect("parse failed")
    }

    // ── L1: minimal ──────────────────────────────────────────

    #[test]
    fn test_l1_minimal() {
        let prog = parse(
            r#"
out 0 (audio): signal;
wire osc = cycle~(440);
out[0] = osc;
"#,
        );

        assert_eq!(prog.out_decls.len(), 1);
        assert_eq!(prog.out_decls[0].index, 0);
        assert_eq!(prog.out_decls[0].name, "audio");
        assert_eq!(prog.out_decls[0].port_type, PortType::Signal);

        assert_eq!(prog.wires.len(), 1);
        assert_eq!(prog.wires[0].name, "osc");
        assert_eq!(
            prog.wires[0].value,
            Expr::Call {
                object: "cycle~".to_string(),
                args: vec![CallArg::positional(Expr::Lit(LitValue::Int(440)))],
            }
        );
        assert!(prog.wires[0].span.is_some());

        assert_eq!(prog.out_assignments.len(), 1);
        assert_eq!(prog.out_assignments[0].index, 0);
        assert_eq!(prog.out_assignments[0].value, Expr::Ref("osc".to_string()));
        assert!(prog.out_assignments[0].span.is_some());

        assert!(prog.in_decls.is_empty());
        assert!(prog.direct_connections.is_empty());
    }

    // ── L2: simple synth ─────────────────────────────────────

    #[test]
    fn test_l2_simple_synth() {
        let prog = parse(
            r#"
// L2_simple_synth.flutmax
in 0 (freq): float;
out 0 (audio): signal;

wire osc = cycle~(freq);
wire amp = mul~(osc, 0.5);

out[0] = amp;
"#,
        );

        assert_eq!(prog.in_decls.len(), 1);
        assert_eq!(prog.in_decls[0].name, "freq");
        assert_eq!(prog.in_decls[0].port_type, PortType::Float);

        assert_eq!(prog.out_decls.len(), 1);
        assert_eq!(prog.wires.len(), 2);
        assert_eq!(prog.wires[0].name, "osc");
        assert_eq!(
            prog.wires[0].value,
            Expr::Call {
                object: "cycle~".to_string(),
                args: vec![CallArg::positional(Expr::Ref("freq".to_string()))],
            }
        );
        assert_eq!(prog.wires[1].name, "amp");
        assert_eq!(
            prog.wires[1].value,
            Expr::Call {
                object: "mul~".to_string(),
                args: vec![
                    CallArg::positional(Expr::Ref("osc".to_string())),
                    CallArg::positional(Expr::Lit(LitValue::Float(0.5))),
                ],
            }
        );
        assert_eq!(prog.out_assignments.len(), 1);
    }

    // ── L3b: control fanout ──────────────────────────────────

    #[test]
    fn test_l3b_control_fanout() {
        let prog = parse(
            r#"
wire trigger = button();
wire counter = counter(trigger);
wire msg = print(counter);

node_a.in[0] = trigger;
node_b.in[0] = trigger;
"#,
        );

        assert_eq!(prog.wires.len(), 3);
        assert_eq!(
            prog.wires[0].value,
            Expr::Call {
                object: "button".to_string(),
                args: vec![],
            }
        );

        assert_eq!(prog.direct_connections.len(), 2);
        assert_eq!(prog.direct_connections[0].target.object, "node_a");
        assert_eq!(prog.direct_connections[0].target.index, 0);
        assert_eq!(
            prog.direct_connections[0].value,
            Expr::Ref("trigger".to_string())
        );
        assert_eq!(prog.direct_connections[1].target.object, "node_b");
    }

    // ── Multiple ports ───────────────────────────────────────

    #[test]
    fn test_multiple_in_out_ports() {
        let prog = parse(
            r#"
in 0 (input_sig): signal;
in 1 (cutoff): float;
in 2 (q_factor): float;

out 0 (lowpass): signal;
out 1 (highpass): signal;
"#,
        );

        assert_eq!(prog.in_decls.len(), 3);
        assert_eq!(prog.in_decls[0].name, "input_sig");
        assert_eq!(prog.in_decls[0].port_type, PortType::Signal);
        assert_eq!(prog.in_decls[1].name, "cutoff");
        assert_eq!(prog.in_decls[1].port_type, PortType::Float);
        assert_eq!(prog.in_decls[2].name, "q_factor");

        assert_eq!(prog.out_decls.len(), 2);
        assert_eq!(prog.out_decls[0].name, "lowpass");
        assert_eq!(prog.out_decls[1].name, "highpass");
    }

    // ── Nested call ──────────────────────────────────────────

    #[test]
    fn test_nested_object_call() {
        let prog = parse("wire sig = biquad~(cycle~(440), 1000, 0.7);");

        assert_eq!(prog.wires.len(), 1);
        assert_eq!(
            prog.wires[0].value,
            Expr::Call {
                object: "biquad~".to_string(),
                args: vec![
                    CallArg::positional(Expr::Call {
                        object: "cycle~".to_string(),
                        args: vec![CallArg::positional(Expr::Lit(LitValue::Int(440)))],
                    }),
                    CallArg::positional(Expr::Lit(LitValue::Int(1000))),
                    CallArg::positional(Expr::Lit(LitValue::Float(0.7))),
                ],
            }
        );
    }

    // ── String literal ───────────────────────────────────────

    #[test]
    fn test_string_literal() {
        let prog = parse(r#"wire msg = print("hello world");"#);
        assert_eq!(
            prog.wires[0].value,
            Expr::Call {
                object: "print".to_string(),
                args: vec![CallArg::positional(Expr::Lit(LitValue::Str(
                    "hello world".to_string()
                )))],
            }
        );
    }

    // ── Zero-arg call ────────────────────────────────────────

    #[test]
    fn test_zero_arg_object_call() {
        let prog = parse("wire btn = button();");
        assert_eq!(
            prog.wires[0].value,
            Expr::Call {
                object: "button".to_string(),
                args: vec![],
            }
        );
    }

    // ── Empty source ─────────────────────────────────────────

    #[test]
    fn test_empty_source() {
        let prog = parse("");
        assert!(prog.in_decls.is_empty());
        assert!(prog.out_decls.is_empty());
        assert!(prog.wires.is_empty());
        assert!(prog.out_assignments.is_empty());
        assert!(prog.direct_connections.is_empty());
    }

    // ── Comments ignored ─────────────────────────────────────

    #[test]
    fn test_comments_ignored() {
        let prog = parse(
            r#"
// This is a comment
wire osc = cycle~(440);
// Another comment
out 0 (audio): signal;
"#,
        );
        assert_eq!(prog.wires.len(), 1);
        assert_eq!(prog.out_decls.len(), 1);
    }

    // ── Span populated ──────────────────────────────────────

    #[test]
    fn test_span_populated() {
        let prog = parse("wire osc = cycle~(440);");
        assert!(prog.wires[0].span.is_some());
        let span = prog.wires[0].span.as_ref().unwrap();
        assert_eq!(span.start_line, 1);
        assert_eq!(span.start_column, 1);
    }

    #[test]
    fn test_out_assignment_span_populated() {
        let prog = parse("out 0 (audio): signal;\nwire osc = cycle~(440);\nout[0] = osc;");
        assert!(prog.out_assignments[0].span.is_some());
        let span = prog.out_assignments[0].span.as_ref().unwrap();
        assert_eq!(span.start_line, 3);
    }

    // ── Tuple / Destructuring ────────────────────────────────

    #[test]
    fn test_tuple_expression() {
        let prog = parse("wire t = (a, b, c);");
        assert_eq!(
            prog.wires[0].value,
            Expr::Tuple(vec![
                Expr::Ref("a".to_string()),
                Expr::Ref("b".to_string()),
                Expr::Ref("c".to_string()),
            ])
        );
    }

    #[test]
    fn test_tuple_two_elements() {
        let prog = parse("wire pair = (x, y);");
        assert_eq!(
            prog.wires[0].value,
            Expr::Tuple(vec![Expr::Ref("x".to_string()), Expr::Ref("y".to_string()),])
        );
    }

    #[test]
    fn test_tuple_with_literals() {
        let prog = parse("wire nums = (1, 2, 3);");
        assert_eq!(
            prog.wires[0].value,
            Expr::Tuple(vec![
                Expr::Lit(LitValue::Int(1)),
                Expr::Lit(LitValue::Int(2)),
                Expr::Lit(LitValue::Int(3)),
            ])
        );
    }

    #[test]
    fn test_destructuring_wire() {
        let prog = parse("wire (a, b, c) = unpack(coords);");
        assert_eq!(prog.destructuring_wires.len(), 1);
        let dw = &prog.destructuring_wires[0];
        assert_eq!(dw.names, vec!["a", "b", "c"]);
        assert_eq!(
            dw.value,
            Expr::Call {
                object: "unpack".to_string(),
                args: vec![CallArg::positional(Expr::Ref("coords".to_string()))],
            }
        );
        assert!(dw.span.is_some());
    }

    #[test]
    fn test_destructuring_wire_two_names() {
        let prog = parse("wire (x, y) = data;");
        assert_eq!(prog.destructuring_wires.len(), 1);
        let dw = &prog.destructuring_wires[0];
        assert_eq!(dw.names, vec!["x", "y"]);
        assert_eq!(dw.value, Expr::Ref("data".to_string()));
    }

    #[test]
    fn test_destructuring_wire_with_tuple_value() {
        let prog = parse("wire (a, b) = (x, y);");
        let dw = &prog.destructuring_wires[0];
        assert_eq!(dw.names, vec!["a", "b"]);
        assert_eq!(
            dw.value,
            Expr::Tuple(vec![Expr::Ref("x".to_string()), Expr::Ref("y".to_string()),])
        );
    }

    #[test]
    fn test_l4_tuple_full() {
        let prog = parse(
            r#"
in 0 (x): float;
in 1 (y): float;
in 2 (z): float;
out 0 (coords): list;

wire packed = (x, y, z);
out[0] = packed;
"#,
        );

        assert_eq!(prog.in_decls.len(), 3);
        assert_eq!(prog.out_decls.len(), 1);
        assert_eq!(prog.wires.len(), 1);
        assert_eq!(
            prog.wires[0].value,
            Expr::Tuple(vec![
                Expr::Ref("x".to_string()),
                Expr::Ref("y".to_string()),
                Expr::Ref("z".to_string()),
            ])
        );
        assert_eq!(prog.out_assignments.len(), 1);
    }

    #[test]
    fn test_l5_destructure_full() {
        let prog = parse(
            r#"
in 0 (coords): list;
out 0 (x): float;
out 1 (y): float;

wire (a, b) = unpack(coords);
out[0] = a;
out[1] = b;
"#,
        );

        assert_eq!(prog.in_decls.len(), 1);
        assert_eq!(prog.out_decls.len(), 2);
        assert_eq!(prog.destructuring_wires.len(), 1);
        assert_eq!(prog.destructuring_wires[0].names, vec!["a", "b"]);
        assert_eq!(prog.out_assignments.len(), 2);
    }

    // ── Feedback ─────────────────────────────────────────────

    #[test]
    fn test_feedback_declaration() {
        let prog = parse("feedback fb: signal;");
        assert_eq!(prog.feedback_decls.len(), 1);
        assert_eq!(prog.feedback_decls[0].name, "fb");
        assert_eq!(prog.feedback_decls[0].port_type, PortType::Signal);
        assert!(prog.feedback_decls[0].span.is_some());
    }

    #[test]
    fn test_feedback_assignment() {
        let prog = parse("feedback fb = tapin~(mixed, 1000);");
        assert_eq!(prog.feedback_assignments.len(), 1);
        assert_eq!(prog.feedback_assignments[0].target, "fb");
        assert_eq!(
            prog.feedback_assignments[0].value,
            Expr::Call {
                object: "tapin~".to_string(),
                args: vec![
                    CallArg::positional(Expr::Ref("mixed".to_string())),
                    CallArg::positional(Expr::Lit(LitValue::Int(1000))),
                ],
            }
        );
        assert!(prog.feedback_assignments[0].span.is_some());
    }

    #[test]
    fn test_feedback_full_patch() {
        let prog = parse(
            r#"
in 0 (input): signal;
out 0 (output): signal;

feedback fb: signal;
wire delayed = tapout~(fb, 500);
wire mixed = add~(input, mul~(delayed, 0.3));
feedback fb = tapin~(mixed, 1000);
out[0] = mixed;
"#,
        );

        assert_eq!(prog.feedback_decls.len(), 1);
        assert_eq!(prog.feedback_decls[0].name, "fb");
        assert_eq!(prog.wires.len(), 2);
        assert_eq!(prog.feedback_assignments.len(), 1);
        assert_eq!(prog.out_assignments.len(), 1);
    }

    // ── Output port access ───────────────────────────────────

    #[test]
    fn test_output_port_access_in_wire() {
        let prog = parse("wire x = node.out[0];");
        match &prog.wires[0].value {
            Expr::OutputPortAccess(opa) => {
                assert_eq!(opa.object, "node");
                assert_eq!(opa.index, 0);
            }
            other => panic!("expected OutputPortAccess, got {:?}", other),
        }
    }

    #[test]
    fn test_output_port_access_in_call_arg() {
        let prog = parse("wire y = mul~(node.out[0], 0.5);");
        if let Expr::Call { args, .. } = &prog.wires[0].value {
            match &args[0].value {
                Expr::OutputPortAccess(opa) => {
                    assert_eq!(opa.object, "node");
                    assert_eq!(opa.index, 0);
                }
                other => panic!("expected OutputPortAccess, got {:?}", other),
            }
        } else {
            panic!("expected Call");
        }
    }

    #[test]
    fn test_output_port_access_higher_index() {
        let prog = parse("wire z = node.out[2];");
        match &prog.wires[0].value {
            Expr::OutputPortAccess(opa) => {
                assert_eq!(opa.object, "node");
                assert_eq!(opa.index, 2);
            }
            other => panic!("expected OutputPortAccess, got {:?}", other),
        }
    }

    #[test]
    fn test_input_port_access_in_direct_connection() {
        let prog = parse("node_a.in[0] = trigger;");
        assert_eq!(prog.direct_connections.len(), 1);
        assert_eq!(prog.direct_connections[0].target.object, "node_a");
        assert_eq!(prog.direct_connections[0].target.index, 0);
    }

    #[test]
    fn test_direct_connection_index_omitted() {
        // .in without [N] defaults to inlet 0
        let prog = parse("tap_l.in = add~(input, fb_r);");
        assert_eq!(prog.direct_connections.len(), 1);
        assert_eq!(prog.direct_connections[0].target.object, "tap_l");
        assert_eq!(prog.direct_connections[0].target.index, 0);
    }

    #[test]
    fn test_direct_connection_index_explicit() {
        // .in[2] still works
        let prog = parse("filter.in[2] = resonance;");
        assert_eq!(prog.direct_connections.len(), 1);
        assert_eq!(prog.direct_connections[0].target.object, "filter");
        assert_eq!(prog.direct_connections[0].target.index, 2);
    }

    // ── State ────────────────────────────────────────────────

    #[test]
    fn test_state_declaration_int() {
        let prog = parse("state counter: int = 0;");
        assert_eq!(prog.state_decls.len(), 1);
        assert_eq!(prog.state_decls[0].name, "counter");
        assert_eq!(prog.state_decls[0].port_type, PortType::Int);
        assert_eq!(prog.state_decls[0].init_value, Expr::Lit(LitValue::Int(0)));
        assert!(prog.state_decls[0].span.is_some());
    }

    #[test]
    fn test_state_declaration_float() {
        let prog = parse("state volume: float = 0.5;");
        assert_eq!(prog.state_decls[0].name, "volume");
        assert_eq!(prog.state_decls[0].port_type, PortType::Float);
        assert_eq!(
            prog.state_decls[0].init_value,
            Expr::Lit(LitValue::Float(0.5))
        );
    }

    #[test]
    fn test_state_assignment() {
        let prog = parse("state counter = next;");
        assert_eq!(prog.state_assignments.len(), 1);
        assert_eq!(prog.state_assignments[0].name, "counter");
        assert_eq!(
            prog.state_assignments[0].value,
            Expr::Ref("next".to_string())
        );
        assert!(prog.state_assignments[0].span.is_some());
    }

    #[test]
    fn test_state_assignment_with_call() {
        let prog = parse("state counter = add(counter, 1);");
        assert_eq!(
            prog.state_assignments[0].value,
            Expr::Call {
                object: "add".to_string(),
                args: vec![
                    CallArg::positional(Expr::Ref("counter".to_string())),
                    CallArg::positional(Expr::Lit(LitValue::Int(1))),
                ],
            }
        );
    }

    #[test]
    fn test_state_full_counter_patch() {
        let prog = parse(
            r#"
state counter: int = 0;
wire next = add(counter, 1);
state counter = next;
out 0 (count): int;
out[0] = next;
"#,
        );
        assert_eq!(prog.state_decls.len(), 1);
        assert_eq!(prog.wires.len(), 1);
        assert_eq!(prog.state_assignments.len(), 1);
        assert_eq!(prog.out_decls.len(), 1);
        assert_eq!(prog.out_assignments.len(), 1);
    }

    // ── Dotted identifiers ───────────────────────────────────

    #[test]
    fn test_dotted_identifier_object_call() {
        let prog = parse("wire vid = jit.gl.videoplane();");
        assert_eq!(
            prog.wires[0].value,
            Expr::Call {
                object: "jit.gl.videoplane".to_string(),
                args: vec![],
            }
        );
    }

    #[test]
    fn test_dotted_identifier_with_args() {
        let prog = parse("wire dial = live.dial(0.5);");
        assert_eq!(
            prog.wires[0].value,
            Expr::Call {
                object: "live.dial".to_string(),
                args: vec![CallArg::positional(Expr::Lit(LitValue::Float(0.5)))],
            }
        );
    }

    #[test]
    fn test_dotted_identifier_does_not_conflict_with_port_access() {
        let prog = parse(
            r#"
wire node = cycle~(440);
wire x = node.out[0];
node.in[0] = 440;
"#,
        );

        assert_eq!(prog.wires.len(), 2);
        assert_eq!(
            prog.wires[1].value,
            Expr::OutputPortAccess(OutputPortAccess {
                object: "node".to_string(),
                index: 0,
            })
        );
        assert_eq!(prog.direct_connections.len(), 1);
        assert_eq!(prog.direct_connections[0].target.object, "node");
        assert_eq!(prog.direct_connections[0].target.index, 0);
    }

    // ── Hyphenated identifiers ───────────────────────────────

    #[test]
    fn test_hyphenated_identifier() {
        let prog = parse("wire x = drunk-walk(10);");
        assert_eq!(
            prog.wires[0].value,
            Expr::Call {
                object: "drunk-walk".to_string(),
                args: vec![CallArg::positional(Expr::Lit(LitValue::Int(10)))],
            }
        );
    }

    // ── Message declarations ─────────────────────────────────

    #[test]
    fn test_msg_declaration() {
        let prog = parse(r#"msg click = "bang";"#);
        assert_eq!(prog.msg_decls.len(), 1);
        assert_eq!(prog.msg_decls[0].name, "click");
        assert_eq!(prog.msg_decls[0].content, "bang");
        assert!(prog.msg_decls[0].span.is_some());
    }

    #[test]
    fn test_msg_declaration_with_template() {
        let prog = parse(r#"msg format = "set $1 $2";"#);
        assert_eq!(prog.msg_decls[0].content, "set $1 $2");
    }

    #[test]
    fn test_msg_declaration_multiple() {
        let prog = parse(
            r#"
msg bang_msg = "bang";
msg set_msg = "set 42";
"#,
        );
        assert_eq!(prog.msg_decls.len(), 2);
        assert_eq!(prog.msg_decls[0].content, "bang");
        assert_eq!(prog.msg_decls[1].content, "set 42");
    }

    #[test]
    fn test_msg_with_wire_and_connection() {
        let prog = parse(
            r#"
msg click = "bang";
wire btn = button();
btn.in[0] = click;
"#,
        );
        assert_eq!(prog.msg_decls.len(), 1);
        assert_eq!(prog.wires.len(), 1);
        assert_eq!(prog.direct_connections.len(), 1);
        assert_eq!(
            prog.direct_connections[0].value,
            Expr::Ref("click".to_string())
        );
    }

    // ── Attr chain ───────────────────────────────────────────

    #[test]
    fn test_wire_with_attrs() {
        let prog = parse(r#"wire w = flonum(x).attr(minimum: 0., maximum: 100.);"#);
        let wire = &prog.wires[0];
        assert_eq!(wire.attrs.len(), 2);
        assert_eq!(wire.attrs[0].key, "minimum");
        assert_eq!(wire.attrs[0].value, AttrValue::Float(0.0));
        assert_eq!(wire.attrs[1].key, "maximum");
        assert_eq!(wire.attrs[1].value, AttrValue::Float(100.0));
    }

    #[test]
    fn test_wire_without_attrs() {
        let prog = parse("wire osc = cycle~(440);");
        assert!(prog.wires[0].attrs.is_empty());
    }

    #[test]
    fn test_wire_with_string_attr() {
        let prog = parse(r#"wire dial = live.dial().attr(parameter_longname: "Cutoff");"#);
        assert_eq!(prog.wires[0].attrs.len(), 1);
        assert_eq!(prog.wires[0].attrs[0].key, "parameter_longname");
        assert_eq!(
            prog.wires[0].attrs[0].value,
            AttrValue::Str("Cutoff".to_string())
        );
    }

    #[test]
    fn test_wire_with_ident_attr() {
        let prog = parse("wire osc = cycle~(freq).attr(phase: half);");
        assert_eq!(prog.wires[0].attrs[0].key, "phase");
        assert_eq!(
            prog.wires[0].attrs[0].value,
            AttrValue::Ident("half".to_string())
        );
    }

    #[test]
    fn test_wire_with_int_attr() {
        let prog = parse("wire w = flonum(x).attr(minimum: 0, maximum: 100);");
        assert_eq!(prog.wires[0].attrs[0].value, AttrValue::Int(0));
        assert_eq!(prog.wires[0].attrs[1].value, AttrValue::Int(100));
    }

    #[test]
    fn test_msg_with_attrs() {
        let prog = parse(r#"msg click = "bang".attr(patching_rect: 100.);"#);
        let msg = &prog.msg_decls[0];
        assert_eq!(msg.attrs.len(), 1);
        assert_eq!(msg.attrs[0].key, "patching_rect");
        assert_eq!(msg.attrs[0].value, AttrValue::Float(100.0));
    }

    #[test]
    fn test_msg_without_attrs() {
        let prog = parse(r#"msg click = "bang";"#);
        assert!(prog.msg_decls[0].attrs.is_empty());
    }

    #[test]
    fn test_wire_multiline_attrs() {
        let prog = parse(
            r#"
wire dial = live.dial().attr(
    parameter_longname: "Cutoff",
    parameter_shortname: "Cut",
    minimum: 20.,
    maximum: 20000.
);
"#,
        );
        assert_eq!(prog.wires[0].attrs.len(), 4);
        assert_eq!(prog.wires[0].attrs[0].key, "parameter_longname");
        assert_eq!(prog.wires[0].attrs[1].key, "parameter_shortname");
        assert_eq!(prog.wires[0].attrs[2].key, "minimum");
        assert_eq!(prog.wires[0].attrs[3].key, "maximum");
    }

    // ── Negative numbers ─────────────────────────────────────

    #[test]
    fn test_negative_integer() {
        let prog = parse("wire x = foo(-7);");
        assert_eq!(
            prog.wires[0].value,
            Expr::Call {
                object: "foo".to_string(),
                args: vec![CallArg::positional(Expr::Lit(LitValue::Int(-7)))],
            }
        );
    }

    #[test]
    fn test_negative_float() {
        let prog = parse("wire x = foo(-3.14);");
        assert_eq!(
            prog.wires[0].value,
            Expr::Call {
                object: "foo".to_string(),
                args: vec![CallArg::positional(Expr::Lit(LitValue::Float(-3.14)))],
            }
        );
    }

    // ── Operator object names ────────────────────────────────

    #[test]
    fn test_operator_object_call() {
        let prog = parse("wire x = ?(a, b);");
        assert_eq!(
            prog.wires[0].value,
            Expr::Call {
                object: "?".to_string(),
                args: vec![
                    CallArg::positional(Expr::Ref("a".to_string())),
                    CallArg::positional(Expr::Ref("b".to_string())),
                ],
            }
        );
    }

    #[test]
    fn test_mul_operator_call() {
        let prog = parse("wire x = *(a, b);");
        assert_eq!(
            prog.wires[0].value,
            Expr::Call {
                object: "*".to_string(),
                args: vec![
                    CallArg::positional(Expr::Ref("a".to_string())),
                    CallArg::positional(Expr::Ref("b".to_string())),
                ],
            }
        );
    }

    // ── Real-world: simpleFM~ ────────────────────────────────

    #[test]
    fn test_simple_fm_tilde() {
        let prog = parse(
            r##"
in 0 (Carrier_frequency): float;
in 1 (Harmonicity_ratio): float;
in 2 (Modulation_index): float;
out 0 (FM_signal): signal;

wire w_1 = mul~("#1");
wire w_2 = mul~("#2");
wire w_3 = cycle~(w_1);
wire w_4 = mul~(w_3, w_2);
wire w_5 = add~(Carrier_frequency, w_4);
wire w_6 = cycle~(w_5);

w_1.in[1] = Harmonicity_ratio;
w_1.in[0] = Carrier_frequency;
w_2.in[0] = w_1;
w_2.in[1] = Modulation_index;

out[0] = w_6;
"##,
        );

        assert_eq!(prog.in_decls.len(), 3);
        assert_eq!(prog.out_decls.len(), 1);
        assert_eq!(prog.wires.len(), 6);
        assert_eq!(prog.direct_connections.len(), 4);
        assert_eq!(prog.out_assignments.len(), 1);

        // Verify wire with string arg
        assert_eq!(
            prog.wires[0].value,
            Expr::Call {
                object: "mul~".to_string(),
                args: vec![CallArg::positional(Expr::Lit(LitValue::Str(
                    "#1".to_string()
                )))],
            }
        );
    }

    // ── Real-world: synthFMvoice~ (complex) ──────────────────

    #[test]
    fn test_synth_fm_voice() {
        let prog = parse(
            r#"
in 0 (MIDi_key___and_velocity): float;
in 1 (frequency_bend): float;
in 2 (additional_modulation_depth): float;
out 0 (voice_output): signal;

msg msg_1 = "0 100".attr(background: 0, bgcolor2: "0.867 0.867 0.867 1.0", gradient: 0);
msg msg_2 = "setdomain $1".attr(background: 0, bgcolor2: "0.867 0.867 0.867 1.0", gradient: 0);

wire w_1 = unpack(MIDi_key___and_velocity).attr(background: 0);
wire w_2 = add~(8.0).attr(background: 0);
wire w_3 = mtof(w_1.out[0]).attr(background: 0);
wire w_4 = select(0).attr(background: 0);

w_2.in[0] = additional_modulation_depth;
w_4.in[0] = w_1.out[1];

out[0] = w_4;
"#,
        );

        assert_eq!(prog.in_decls.len(), 3);
        assert_eq!(prog.out_decls.len(), 1);
        assert_eq!(prog.msg_decls.len(), 2);
        assert_eq!(prog.wires.len(), 4);

        // msg with multiple attrs
        assert_eq!(prog.msg_decls[0].content, "0 100");
        assert_eq!(prog.msg_decls[0].attrs.len(), 3);
        assert_eq!(prog.msg_decls[0].attrs[0].key, "background");

        // wire with .out[N] in args
        assert_eq!(
            prog.wires[2].value,
            Expr::Call {
                object: "mtof".to_string(),
                args: vec![CallArg::positional(Expr::OutputPortAccess(
                    OutputPortAccess {
                        object: "w_1".to_string(),
                        index: 0,
                    }
                ))],
            }
        );

        // direct connection with .out[N] on RHS
        assert_eq!(
            prog.direct_connections[1].value,
            Expr::OutputPortAccess(OutputPortAccess {
                object: "w_1".to_string(),
                index: 1,
            })
        );
    }

    // ── Wire with string arg containing special chars ────────

    #[test]
    fn test_wire_with_complex_string_arg() {
        let prog = parse(r#"wire w_6 = t("b", "f", "f");"#);
        assert_eq!(
            prog.wires[0].value,
            Expr::Call {
                object: "t".to_string(),
                args: vec![
                    CallArg::positional(Expr::Lit(LitValue::Str("b".to_string()))),
                    CallArg::positional(Expr::Lit(LitValue::Str("f".to_string()))),
                    CallArg::positional(Expr::Lit(LitValue::Str("f".to_string()))),
                ],
            }
        );
    }

    // ── Wire with expr containing escapes ────────────────────

    #[test]
    fn test_wire_with_escaped_string() {
        let prog = parse(r#"wire w = expr("pow($f1/127.\\,4.)");"#);
        assert_eq!(
            prog.wires[0].value,
            Expr::Call {
                object: "expr".to_string(),
                args: vec![CallArg::positional(Expr::Lit(LitValue::Str(
                    "pow($f1/127.\\,4.)".to_string()
                )))],
            }
        );
    }

    // ── Trailing-dot float ───────────────────────────────────

    #[test]
    fn test_trailing_dot_float() {
        let prog = parse("wire x = foo(100.);");
        assert_eq!(
            prog.wires[0].value,
            Expr::Call {
                object: "foo".to_string(),
                args: vec![CallArg::positional(Expr::Lit(LitValue::Float(100.0)))],
            }
        );
    }

    // ── Scientific notation ──────────────────────────────────

    #[test]
    fn test_scientific_notation() {
        let prog = parse("wire x = foo(1e-6);");
        assert_eq!(
            prog.wires[0].value,
            Expr::Call {
                object: "foo".to_string(),
                args: vec![CallArg::positional(Expr::Lit(LitValue::Float(1e-6)))],
            }
        );
    }

    // ── msg.in[N] direct connection (msg as identifier) ──────

    #[test]
    fn test_msg_identifier_direct_connection() {
        // `msg_1.in[0] = w_4.out[0];` — `msg_1` is an identifier, not keyword `msg`
        let prog = parse("msg_1.in[0] = w_4.out[0];");
        assert_eq!(prog.direct_connections.len(), 1);
        assert_eq!(prog.direct_connections[0].target.object, "msg_1");
        assert_eq!(
            prog.direct_connections[0].value,
            Expr::OutputPortAccess(OutputPortAccess {
                object: "w_4".to_string(),
                index: 0,
            })
        );
    }

    // ── Implicit port index ──────────────────────────────────

    #[test]
    fn test_implicit_in_single() {
        let prog = parse("in freq: float;");
        assert_eq!(prog.in_decls.len(), 1);
        assert_eq!(prog.in_decls[0].index, 0);
        assert_eq!(prog.in_decls[0].name, "freq");
        assert_eq!(prog.in_decls[0].port_type, PortType::Float);
    }

    #[test]
    fn test_implicit_in_multiple() {
        let prog = parse("in freq: float;\nin cutoff: float;");
        assert_eq!(prog.in_decls.len(), 2);
        assert_eq!(prog.in_decls[0].index, 0);
        assert_eq!(prog.in_decls[0].name, "freq");
        assert_eq!(prog.in_decls[1].index, 1);
        assert_eq!(prog.in_decls[1].name, "cutoff");
    }

    #[test]
    fn test_implicit_out_single() {
        let prog = parse("out audio: signal;");
        assert_eq!(prog.out_decls.len(), 1);
        assert_eq!(prog.out_decls[0].index, 0);
        assert_eq!(prog.out_decls[0].name, "audio");
        assert_eq!(prog.out_decls[0].port_type, PortType::Signal);
    }

    #[test]
    fn test_implicit_out_multiple() {
        let prog = parse("out left: signal;\nout right: signal;");
        assert_eq!(prog.out_decls.len(), 2);
        assert_eq!(prog.out_decls[0].index, 0);
        assert_eq!(prog.out_decls[0].name, "left");
        assert_eq!(prog.out_decls[1].index, 1);
        assert_eq!(prog.out_decls[1].name, "right");
    }

    #[test]
    fn test_explicit_index_still_works() {
        let prog = parse("in 5 (freq): float;");
        assert_eq!(prog.in_decls.len(), 1);
        assert_eq!(prog.in_decls[0].index, 5);
        assert_eq!(prog.in_decls[0].name, "freq");
    }

    #[test]
    fn test_implicit_separate_counters_in_out() {
        // in and out should have independent counters
        let prog = parse("in a: float;\nout x: signal;\nin b: float;\nout y: signal;");
        assert_eq!(prog.in_decls.len(), 2);
        assert_eq!(prog.in_decls[0].index, 0);
        assert_eq!(prog.in_decls[0].name, "a");
        assert_eq!(prog.in_decls[1].index, 1);
        assert_eq!(prog.in_decls[1].name, "b");
        assert_eq!(prog.out_decls.len(), 2);
        assert_eq!(prog.out_decls[0].index, 0);
        assert_eq!(prog.out_decls[0].name, "x");
        assert_eq!(prog.out_decls[1].index, 1);
        assert_eq!(prog.out_decls[1].name, "y");
    }

    #[test]
    fn test_implicit_all_port_types() {
        let prog = parse(
            "in a: signal;\nin b: float;\nin c: int;\nin d: bang;\nin e: list;\nin f: symbol;",
        );
        assert_eq!(prog.in_decls.len(), 6);
        assert_eq!(prog.in_decls[0].port_type, PortType::Signal);
        assert_eq!(prog.in_decls[1].port_type, PortType::Float);
        assert_eq!(prog.in_decls[2].port_type, PortType::Int);
        assert_eq!(prog.in_decls[3].port_type, PortType::Bang);
        assert_eq!(prog.in_decls[4].port_type, PortType::List);
        assert_eq!(prog.in_decls[5].port_type, PortType::Symbol);
        for (i, decl) in prog.in_decls.iter().enumerate() {
            assert_eq!(decl.index, i as u32);
        }
    }

    #[test]
    fn test_implicit_in_full_patch() {
        let prog = parse(
            r#"
in carrier_freq: float;
in harmonicity: float;
out fm_signal: signal;

wire osc = cycle~(carrier_freq);
out[0] = osc;
"#,
        );
        assert_eq!(prog.in_decls.len(), 2);
        assert_eq!(prog.in_decls[0].index, 0);
        assert_eq!(prog.in_decls[0].name, "carrier_freq");
        assert_eq!(prog.in_decls[1].index, 1);
        assert_eq!(prog.in_decls[1].name, "harmonicity");
        assert_eq!(prog.out_decls.len(), 1);
        assert_eq!(prog.out_decls[0].index, 0);
        assert_eq!(prog.out_decls[0].name, "fm_signal");
        assert_eq!(prog.wires.len(), 1);
        assert_eq!(prog.out_assignments.len(), 1);
    }

    // ── E52: Out declaration with inline assignment ──────────

    #[test]
    fn test_out_decl_inline_value_implicit() {
        // out audio: signal = osc;
        let prog = parse(
            r#"
wire osc = cycle~(440);
out audio: signal = osc;
"#,
        );
        assert_eq!(prog.out_decls.len(), 1);
        assert_eq!(prog.out_decls[0].index, 0);
        assert_eq!(prog.out_decls[0].name, "audio");
        assert_eq!(prog.out_decls[0].port_type, PortType::Signal);
        assert_eq!(prog.out_decls[0].value, Some(Expr::Ref("osc".to_string())));
        // No separate out_assignment
        assert!(prog.out_assignments.is_empty());
    }

    #[test]
    fn test_out_decl_inline_value_explicit() {
        // out 0 (audio): signal = osc;
        let prog = parse(
            r#"
wire osc = cycle~(440);
out 0 (audio): signal = osc;
"#,
        );
        assert_eq!(prog.out_decls.len(), 1);
        assert_eq!(prog.out_decls[0].index, 0);
        assert_eq!(prog.out_decls[0].name, "audio");
        assert_eq!(prog.out_decls[0].port_type, PortType::Signal);
        assert_eq!(prog.out_decls[0].value, Some(Expr::Ref("osc".to_string())));
        assert!(prog.out_assignments.is_empty());
    }

    #[test]
    fn test_out_decl_without_value_backward_compat() {
        // out audio: signal; (no inline value)
        let prog = parse(
            r#"
out audio: signal;
wire osc = cycle~(440);
out[0] = osc;
"#,
        );
        assert_eq!(prog.out_decls.len(), 1);
        assert_eq!(prog.out_decls[0].value, None);
        assert_eq!(prog.out_assignments.len(), 1);
        assert_eq!(prog.out_assignments[0].index, 0);
    }

    #[test]
    fn test_out_decl_inline_with_call_expr() {
        // out audio: signal = cycle~(440);
        let prog = parse(
            r#"
out audio: signal = cycle~(440);
"#,
        );
        assert_eq!(prog.out_decls.len(), 1);
        assert_eq!(prog.out_decls[0].name, "audio");
        assert_eq!(
            prog.out_decls[0].value,
            Some(Expr::Call {
                object: "cycle~".to_string(),
                args: vec![CallArg::positional(Expr::Lit(LitValue::Int(440)))],
            })
        );
        assert!(prog.out_assignments.is_empty());
    }

    #[test]
    fn test_out_assignment_unchanged() {
        // out[0] = osc; remains as OutAssignment
        let prog = parse(
            r#"
out audio: signal;
wire osc = cycle~(440);
out[0] = osc;
"#,
        );
        assert_eq!(prog.out_decls.len(), 1);
        assert_eq!(prog.out_decls[0].value, None);
        assert_eq!(prog.out_assignments.len(), 1);
        assert_eq!(prog.out_assignments[0].index, 0);
        assert_eq!(prog.out_assignments[0].value, Expr::Ref("osc".to_string()));
    }

    #[test]
    fn test_out_decl_inline_multiple() {
        // Multiple out declarations with inline values
        let prog = parse(
            r#"
wire left = cycle~(440);
wire right = cycle~(880);
out left_out: signal = left;
out right_out: signal = right;
"#,
        );
        assert_eq!(prog.out_decls.len(), 2);
        assert_eq!(prog.out_decls[0].index, 0);
        assert_eq!(prog.out_decls[0].name, "left_out");
        assert_eq!(prog.out_decls[0].value, Some(Expr::Ref("left".to_string())));
        assert_eq!(prog.out_decls[1].index, 1);
        assert_eq!(prog.out_decls[1].name, "right_out");
        assert_eq!(
            prog.out_decls[1].value,
            Some(Expr::Ref("right".to_string()))
        );
        assert!(prog.out_assignments.is_empty());
    }

    // ── Named argument tests ──────────────────────────────────

    #[test]
    fn test_named_args_single() {
        let prog = parse("wire x = cycle~(freq: 440);");
        assert_eq!(prog.wires.len(), 1);
        if let Expr::Call { object, args } = &prog.wires[0].value {
            assert_eq!(object, "cycle~");
            assert_eq!(args.len(), 1);
            assert_eq!(args[0].name, Some("freq".to_string()));
            assert_eq!(args[0].value, Expr::Lit(LitValue::Int(440)));
        } else {
            panic!("expected Call");
        }
    }

    #[test]
    fn test_named_args_multiple() {
        let prog = parse("wire x = biquad~(input: osc, freq: cutoff, q: resonance);");
        if let Expr::Call { args, .. } = &prog.wires[0].value {
            assert_eq!(args.len(), 3);
            assert_eq!(args[0].name, Some("input".to_string()));
            assert_eq!(args[0].value, Expr::Ref("osc".to_string()));
            assert_eq!(args[1].name, Some("freq".to_string()));
            assert_eq!(args[1].value, Expr::Ref("cutoff".to_string()));
            assert_eq!(args[2].name, Some("q".to_string()));
            assert_eq!(args[2].value, Expr::Ref("resonance".to_string()));
        } else {
            panic!("expected Call");
        }
    }

    #[test]
    fn test_mixed_positional_and_named_args() {
        let prog = parse("wire x = biquad~(osc, freq: cutoff, 0.7);");
        if let Expr::Call { args, .. } = &prog.wires[0].value {
            assert_eq!(args.len(), 3);
            assert_eq!(args[0].name, None);
            assert_eq!(args[0].value, Expr::Ref("osc".to_string()));
            assert_eq!(args[1].name, Some("freq".to_string()));
            assert_eq!(args[1].value, Expr::Ref("cutoff".to_string()));
            assert_eq!(args[2].name, None);
            assert_eq!(args[2].value, Expr::Lit(LitValue::Float(0.7)));
        } else {
            panic!("expected Call");
        }
    }

    #[test]
    fn test_named_arg_with_literal() {
        let prog = parse(r#"wire x = print(msg: "hello");"#);
        if let Expr::Call { args, .. } = &prog.wires[0].value {
            assert_eq!(args.len(), 1);
            assert_eq!(args[0].name, Some("msg".to_string()));
            assert_eq!(args[0].value, Expr::Lit(LitValue::Str("hello".to_string())));
        } else {
            panic!("expected Call");
        }
    }

    #[test]
    fn test_positional_args_still_work() {
        // Ensure backward compatibility: positional args have name = None
        let prog = parse("wire x = cycle~(440);");
        if let Expr::Call { args, .. } = &prog.wires[0].value {
            assert_eq!(args.len(), 1);
            assert_eq!(args[0].name, None);
            assert_eq!(args[0].value, Expr::Lit(LitValue::Int(440)));
        } else {
            panic!("expected Call");
        }
    }
}
