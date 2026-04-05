/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

module.exports = grammar({
  name: "flutmax",

  // -- whitespace and comment auto-skip --
  extras: $ => [
    /\s/,
    $.comment,
  ],

  // -- keyword extraction token --
  word: $ => $.plain_identifier,

  rules: {
    // ================================================
    // Top level
    // ================================================
    source_file: $ => repeat($._statement),

    _statement: $ => choice(
      $.port_declaration,
      $.destructuring_wire,
      $.wire_declaration,
      $.msg_declaration,
      $.out_assignment,
      $.direct_connection,
      $.feedback_declaration,
      $.feedback_assignment,
      $.state_declaration,
      $.state_assignment,
    ),

    // ================================================
    // Port declaration:
    //   Explicit: in 0 (input_sig): signal;
    //   Implicit: in input_sig: signal;
    //   With value: out audio: signal = expr;
    // ================================================
    port_declaration: $ => choice(
      // Explicit: in 0 (name): type; or out 0 (name): type = expr;
      seq(
        field("direction", $.direction),
        field("index", $.integer),
        "(",
        field("name", $.plain_identifier),
        ")",
        ":",
        field("type", $.type_name),
        optional(seq("=", field("value", $._expression))),
        ";",
      ),
      // Implicit: in name: type; or out name: type = expr;
      seq(
        field("direction", $.direction),
        field("name", $.plain_identifier),
        ":",
        field("type", $.type_name),
        optional(seq("=", field("value", $._expression))),
        ";",
      ),
    ),

    direction: $ => choice("in", "out"),

    type_name: $ => choice(
      "signal",
      "float",
      "int",
      "bang",
      "list",
      "symbol",
    ),

    // ================================================
    // Destructuring wire: wire (a, b, c) = expr;
    // ================================================
    destructuring_wire: $ => seq(
      "wire",
      "(",
      field("names", $.identifier_list),
      ")",
      "=",
      field("value", $._expression),
      ";",
    ),

    identifier_list: $ => seq(
      $.plain_identifier,
      repeat1(seq(",", $.plain_identifier)),
    ),

    // ================================================
    // Wire declaration: wire osc = cycle~(440);
    // Wire with attrs: wire w = flonum(x).attr(minimum: 0., maximum: 100.);
    // ================================================
    wire_declaration: $ => seq(
      "wire",
      field("name", $.plain_identifier),
      "=",
      field("value", $._expression),
      optional(field("attrs", $.attr_chain)),
      ";",
    ),

    // ================================================
    // Message declaration: msg click = "bang";
    // Message with attrs: msg click = "bang".attr(patching_rect: 100.);
    // ================================================
    msg_declaration: $ => seq(
      "msg",
      field("name", $.plain_identifier),
      "=",
      field("content", $.string),
      optional(field("attrs", $.attr_chain)),
      ";",
    ),

    // ================================================
    // Attribute chain: .attr(key: value, key: value, ...)
    // ================================================
    attr_chain: $ => seq(
      ".attr(",
      field("pairs", $.attr_list),
      ")",
    ),

    attr_list: $ => seq(
      $.attr_pair,
      repeat(seq(",", $.attr_pair)),
    ),

    attr_pair: $ => seq(
      field("key", $.plain_identifier),
      ":",
      field("value", $._attr_value),
    ),

    _attr_value: $ => choice(
      $.number,
      $.string,
      $.plain_identifier,
    ),

    // ================================================
    // Output assignment: out[0] = osc;
    // ================================================
    out_assignment: $ => seq(
      "out",
      "[",
      field("index", $.integer),
      "]",
      "=",
      field("value", $._expression),
      ";",
    ),

    // ================================================
    // Direct connection: node_a.in[0] = trigger;
    // lvalue is restricted to input_port_access only
    // ================================================
    direct_connection: $ => seq(
      field("target", $.input_port_access),
      "=",
      field("value", $._expression),
      ";",
    ),

    // ================================================
    // Feedback declaration: feedback fb: signal;
    // ================================================
    feedback_declaration: $ => seq(
      "feedback",
      field("name", $.plain_identifier),
      ":",
      field("type", $.type_name),
      ";",
    ),

    // ================================================
    // Feedback assignment: feedback fb = tapin~(mixed, 1000);
    // ================================================
    feedback_assignment: $ => seq(
      "feedback",
      field("target", $.plain_identifier),
      "=",
      field("value", $._expression),
      ";",
    ),

    // ================================================
    // State declaration: state counter: int = 0;
    // ================================================
    state_declaration: $ => seq(
      "state",
      field("name", $.plain_identifier),
      ":",
      field("type", $.control_type),
      "=",
      field("init", $._expression),
      ";",
    ),

    // ================================================
    // State assignment: state counter = next;
    // ================================================
    state_assignment: $ => seq(
      "state",
      field("target", $.plain_identifier),
      "=",
      field("value", $._expression),
      ";",
    ),

    // ================================================
    // Control type (used by state declaration)
    // ================================================
    control_type: $ => choice("float", "int", "bang", "list", "symbol"),

    // ================================================
    // Port access (lvalue/rvalue split)
    // ================================================

    // lvalue — used only as assignment target: node.in[N] or node.in (defaults to inlet 0)
    // prec(10) ensures priority over dotted_identifier
    input_port_access: $ => prec.dynamic(10, seq(
      field("object", $.plain_identifier),
      ".",
      "in",
      optional(seq("[", field("index", $.integer), "]")),
    )),

    // rvalue — used as expression: node.out[N]
    // prec(10) ensures priority over dotted_identifier
    output_port_access: $ => prec.dynamic(10, seq(
      field("object", $.plain_identifier),
      ".",
      "out",
      "[",
      field("index", $.integer),
      "]",
    )),

    // ================================================
    // Expression
    // ================================================
    _expression: $ => choice(
      $.object_call,
      $.output_port_access,
      $.tuple_expression,
      $.identifier,
      $.number,
      $.string,
    ),

    // ================================================
    // Tuple expression: (x, y, z) — must have 2+ elements
    // ================================================
    tuple_expression: $ => seq(
      "(",
      $._expression,
      repeat1(seq(",", $._expression)),
      ")",
    ),

    // ================================================
    // Object call: cycle~(440), biquad~(osc, cutoff)
    // ================================================
    object_call: $ => seq(
      field("object", $.object_name),
      "(",
      field("arguments", optional($.argument_list)),
      ")",
    ),

    object_name: $ => choice(
      $.tilde_identifier,    // cycle~, jit.gl.videoplane~
      $.dotted_identifier,   // jit.gl.videoplane, live.dial
      $.operator_name,       // ?, *, +, -, /, %, ==, !=, <, >, <=, >=, &&, ||, etc.
      $.plain_identifier,    // pack, trigger, drunk-walk
    ),

    // Operator names used as Max/gen~ objects: ?, *, +, -, /, %, ==, !=, etc.
    operator_name: $ => /[*/%!<>=+\-&|^?]+/,

    argument_list: $ => seq(
      $._expression,
      repeat(seq(",", $._expression)),
    ),

    // ================================================
    // Identifiers and literals
    // ================================================

    // Generic identifier (includes tilde and dotted variants)
    identifier: $ => choice(
      $.tilde_identifier,
      $.dotted_identifier,
      $.plain_identifier,
    ),

    // Tilde identifier: cycle~, phasor~, biquad~, jit.gl.videoplane~, mc.+~, mc.*~
    // token() ensures no whitespace between the name and ~
    // Supports dots and hyphens in segments before the tilde.
    // Dotted segments can be identifier segments or operator chars (for mc.+~ etc.)
    tilde_identifier: $ => token(
      seq(
        /[a-zA-Z_][a-zA-Z0-9_\-]*/,
        optional(
          /(\.[a-zA-Z0-9_*+\-/%!=<>&|^][a-zA-Z0-9_\-]*)*/,
        ),
        "~",
      ),
    ),

    // Dotted identifier: jit.gl.videoplane, live.dial, M4L.api.DeviceParameter, jit.3m
    // Must contain at least one dot to distinguish from plain_identifier.
    // NOT a token() — uses parser-level rules so port access can take priority
    // via dynamic precedence.
    // Segments after the first dot can start with digits (e.g., jit.3m, vz.4oscil8r).
    dotted_identifier: $ => prec.dynamic(-1, seq(
      $.plain_identifier,
      repeat1(seq(".", $.dotted_segment)),
    )),

    // A segment in a dotted identifier. Allows digit-starting segments (e.g., 3m in jit.3m)
    // and special chars like = (e.g., gbr.wind= in IRCAM objects).
    dotted_segment: $ => /[a-zA-Z0-9_][a-zA-Z0-9_=]*(\-[a-zA-Z0-9_]+)*/,

    // Plain identifier: allows hyphens (e.g., drunk-walk, if-then)
    // Hyphens must not be at the start (to avoid conflict with negative numbers)
    plain_identifier: $ => /[a-zA-Z_][a-zA-Z0-9_]*(\-[a-zA-Z0-9_]+)*/,

    // Integer
    integer: $ => /\d+/,

    // Number (integer or float, optionally negative)
    // Supports trailing-dot floats (e.g., 0., 100.) and scientific notation (e.g., 1e-6, 3.14e5)
    number: $ => choice(
      /-?\d+\.\d+[eE][+-]?\d+/,   // 3.14e-5, 1.0E+3
      /-?\d+[eE][+-]?\d+/,         // 1e-6, 2E5
      /-?\d+\.\d+/,                // 3.14, -0.5
      /-?\d+\./,                   // 0., 100.
      /-?\d+/,                     // 42, -7
    ),

    // String literal — must be token() to prevent extras (comments) from
    // being matched inside the string (e.g., "//" in URLs).
    string: $ => token(seq(
      '"',
      repeat(choice(
        /[^"\\]/,
        seq("\\", /./),
      )),
      '"',
    )),

    // ================================================
    // Comment
    // ================================================
    comment: $ => token(seq("//", /[^\n]*/)),
  },
});
