//! Small arithmetic-expression evaluator used by inline numeric fields.

/// Evaluates a finite arithmetic expression containing `+`, `-`, `*`, `/`,
/// parentheses, decimal numbers, and unary minus.
pub fn eval(expr: &str) -> Option<f64> {
    eval_with(expr, &|_| None)
}

/// Evaluates an expression and resolves identifiers through `vars`.
///
/// Identifiers use the ASCII pattern `[a-zA-Z_][a-zA-Z0-9_]*`.
pub fn eval_with(expr: &str, vars: &dyn Fn(&str) -> Option<f64>) -> Option<f64> {
    let mut parser = Parser {
        bytes: expr.as_bytes(),
        cursor: 0,
        vars,
    };
    let value = parser.expression()?;
    parser.skip_whitespace();
    (parser.cursor == parser.bytes.len() && value.is_finite()).then_some(value)
}

/// Returns the first identifier that the resolver does not know.
pub fn first_unknown_identifier(expr: &str, vars: &dyn Fn(&str) -> Option<f64>) -> Option<String> {
    let bytes = expr.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor].is_ascii_alphabetic() || bytes[cursor] == b'_' {
            let start = cursor;
            cursor += 1;
            while bytes
                .get(cursor)
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            {
                cursor += 1;
            }
            let name = std::str::from_utf8(&bytes[start..cursor]).ok()?;
            if vars(name).is_none() {
                return Some(name.to_owned());
            }
        } else {
            cursor += 1;
        }
    }
    None
}

/// Returns whether an expression contains at least one identifier token.
pub fn contains_identifier(expr: &str) -> bool {
    expr.bytes()
        .any(|byte| byte.is_ascii_alphabetic() || byte == b'_')
}

struct Parser<'a> {
    bytes: &'a [u8],
    cursor: usize,
    vars: &'a dyn Fn(&str) -> Option<f64>,
}

impl Parser<'_> {
    fn expression(&mut self) -> Option<f64> {
        let mut value = self.term()?;
        loop {
            if self.eat(b'+') {
                value += self.term()?;
            } else if self.eat(b'-') {
                value -= self.term()?;
            } else {
                return Some(value);
            }
        }
    }

    fn term(&mut self) -> Option<f64> {
        let mut value = self.factor()?;
        loop {
            if self.eat(b'*') {
                value *= self.factor()?;
            } else if self.eat(b'/') {
                value /= self.factor()?;
            } else {
                return value.is_finite().then_some(value);
            }
        }
    }

    fn factor(&mut self) -> Option<f64> {
        if self.eat(b'-') {
            return Some(-self.factor()?);
        }
        if self.eat(b'(') {
            let value = self.expression()?;
            return self.eat(b')').then_some(value);
        }
        self.number().or_else(|| self.identifier())
    }

    fn number(&mut self) -> Option<f64> {
        self.skip_whitespace();
        let start = self.cursor;
        while self
            .bytes
            .get(self.cursor)
            .is_some_and(|byte| byte.is_ascii_digit() || *byte == b'.')
        {
            self.cursor += 1;
        }
        (self.cursor > start).then(|| {
            std::str::from_utf8(&self.bytes[start..self.cursor])
                .ok()?
                .parse()
                .ok()
        })?
    }

    fn identifier(&mut self) -> Option<f64> {
        self.skip_whitespace();
        let start = self.cursor;
        let first = *self.bytes.get(self.cursor)?;
        if !first.is_ascii_alphabetic() && first != b'_' {
            return None;
        }
        self.cursor += 1;
        while self
            .bytes
            .get(self.cursor)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            self.cursor += 1;
        }
        let name = std::str::from_utf8(&self.bytes[start..self.cursor]).ok()?;
        (self.vars)(name)
    }

    fn eat(&mut self, expected: u8) -> bool {
        self.skip_whitespace();
        if self.bytes.get(self.cursor) == Some(&expected) {
            self.cursor += 1;
            true
        } else {
            false
        }
    }

    fn skip_whitespace(&mut self) {
        while self
            .bytes
            .get(self.cursor)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.cursor += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{eval, eval_with};

    #[test]
    fn evaluates_supported_expressions() {
        assert_eq!(eval("12+34"), Some(46.0));
        assert_eq!(eval("50/2"), Some(25.0));
        assert_eq!(eval("2*(3+4)"), Some(14.0));
        assert_eq!(eval("-5+1"), Some(-4.0));
    }

    #[test]
    fn rejects_garbage_and_non_finite_results() {
        assert_eq!(eval("garbage"), None);
        assert_eq!(eval("1+"), None);
        assert_eq!(eval("1/0"), None);
    }

    #[test]
    fn resolves_identifiers_and_rejects_unknown_names() {
        let vars = |name: &str| (name == "width").then_some(20.0);
        assert_eq!(
            eval_with("width * 2 + _gap", &|name| {
                vars(name).or_else(|| (name == "_gap").then_some(3.0))
            }),
            Some(43.0)
        );
        assert_eq!(eval_with("missing + 1", &vars), None);
        assert_eq!(eval("width"), None);
    }
}
