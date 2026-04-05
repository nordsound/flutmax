/// Reverse-map Max operator names to flutmax aliases.
///
/// In .maxpat files, arithmetic objects use operator symbols (e.g., `+`, `*~`).
/// flutmax uses word aliases (e.g., `add`, `mul~`).
/// Comparison operators (`>`, `<`, `>=`, `<=`, `==`, `!=`) and their signal-rate
/// variants are also mapped to word aliases.
pub fn reverse_alias(max_name: &str) -> &str {
    match max_name {
        // Arithmetic
        "+" => "add",
        "-" => "sub",
        "*" => "mul",
        "/" => "dvd",
        "%" => "mod",
        "+~" => "add~",
        "-~" => "sub~",
        "*~" => "mul~",
        "/~" => "dvd~",
        "%~" => "mod~",
        // Comparison
        ">" => "gt",
        "<" => "lt",
        ">=" => "gte",
        "<=" => "lte",
        "==" => "eq",
        "!=" => "neq",
        ">~" => "gt~",
        "<~" => "lt~",
        ">=~" => "gte~",
        "<=~" => "lte~",
        "==~" => "eq~",
        "!=~" => "neq~",
        // Reversed arithmetic (input goes to second operand)
        "!-" => "rsub",
        "!/" => "rdvd",
        "!%" => "rmod",
        "!-~" => "rsub~",
        "!/~" => "rdvd~",
        "!%~" => "rmod~",
        // Bitwise / logical
        "&&" => "and",
        "||" => "or",
        "<<" => "lshift",
        ">>" => "rshift",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arithmetic_aliases() {
        assert_eq!(reverse_alias("+"), "add");
        assert_eq!(reverse_alias("-"), "sub");
        assert_eq!(reverse_alias("*"), "mul");
        assert_eq!(reverse_alias("/"), "dvd");
        assert_eq!(reverse_alias("%"), "mod");
    }

    #[test]
    fn test_signal_aliases() {
        assert_eq!(reverse_alias("+~"), "add~");
        assert_eq!(reverse_alias("-~"), "sub~");
        assert_eq!(reverse_alias("*~"), "mul~");
        assert_eq!(reverse_alias("/~"), "dvd~");
        assert_eq!(reverse_alias("%~"), "mod~");
    }

    #[test]
    fn test_comparison_aliases() {
        assert_eq!(reverse_alias(">"), "gt");
        assert_eq!(reverse_alias("<"), "lt");
        assert_eq!(reverse_alias(">="), "gte");
        assert_eq!(reverse_alias("<="), "lte");
        assert_eq!(reverse_alias("=="), "eq");
        assert_eq!(reverse_alias("!="), "neq");
    }

    #[test]
    fn test_signal_comparison_aliases() {
        assert_eq!(reverse_alias(">~"), "gt~");
        assert_eq!(reverse_alias("<~"), "lt~");
        assert_eq!(reverse_alias(">=~"), "gte~");
        assert_eq!(reverse_alias("<=~"), "lte~");
        assert_eq!(reverse_alias("==~"), "eq~");
        assert_eq!(reverse_alias("!=~"), "neq~");
    }

    #[test]
    fn test_logical_aliases() {
        assert_eq!(reverse_alias("&&"), "and");
        assert_eq!(reverse_alias("||"), "or");
    }

    #[test]
    fn test_passthrough() {
        assert_eq!(reverse_alias("cycle~"), "cycle~");
        assert_eq!(reverse_alias("biquad~"), "biquad~");
        assert_eq!(reverse_alias("print"), "print");
    }
}
