//! Typed expression evaluation for calculated properties.
//!
//! The original formula engine substituted `{{ref}}` textually and handed the
//! result to a numeric evaluator, so a calculated property could only ever be a
//! number — and a negative value had to be wrapped in parentheses to survive the
//! substitution. That shape cannot express "Overdue" vs "On track", or a name
//! built from two fields.
//!
//! Here a reference is a *token*, resolved at evaluation time with its type
//! intact. That removes the substitution hack and makes text, booleans and dates
//! first-class without a second expression language: the same `{{property}}`
//! placeholder means the same thing everywhere.

use chrono::{Datelike, NaiveDate, Utc};

/// A value flowing through an expression.
///
/// `Missing` is an absent property, carrying the name that was looked up. In text
/// it renders empty — "a null segment becomes an empty string" — but in arithmetic
/// it fails, naming the reference: silently counting an unfilled field as zero is
/// how a wrong total gets shipped looking right.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Number(f64),
    Text(String),
    Bool(bool),
    Missing(String),
}

impl Value {
    pub fn as_number(&self) -> Result<f64, String> {
        match self {
            Value::Number(n) => Ok(*n),
            Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            // A numeric-looking string is treated as the number it spells: values
            // arrive from the store as text and their datatype is not always declared.
            Value::Text(t) => t.trim().parse::<f64>()
                .map_err(|_| format!("'{}' is not a number", t)),
            Value::Missing(name) if name.is_empty() => Err("missing value".to_string()),
            Value::Missing(name) => Err(format!("Missing value for {{{{{}}}}}", name)),
        }
    }

    pub fn as_text(&self) -> String {
        match self {
            Value::Number(n) => crate::owl::formula::format_calculated_number(*n),
            Value::Text(t) => t.clone(),
            Value::Bool(b) => b.to_string(),
            Value::Missing(_) => String::new(),
        }
    }

    pub fn as_bool(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Number(n) => *n != 0.0,
            Value::Text(t) => !t.is_empty() && t != "false",
            Value::Missing(_) => false,
        }
    }

    /// True only for text that is not a number in disguise.
    ///
    /// Values arrive from the store as strings whether or not the property declares
    /// a numeric range, so treating every string as text would turn `2 + 3` into
    /// "23" for any undeclared property. Text that parses as a number stays a
    /// number; `CONCAT()` is there when joining digits is what you actually meant.
    fn forces_text(&self) -> bool {
        matches!(self, Value::Text(t) if t.trim().parse::<f64>().is_err())
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Text(String),
    Ref(String),
    Ident(String),
    Op(String),
    LParen,
    RParen,
    Comma,
}

fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = input.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        if c.is_whitespace() { i += 1; continue; }

        // {{property:iri}} — a reference, kept whole so the IRI's colon and dots
        // never reach the operator scanner.
        if c == '{' && i + 1 < chars.len() && chars[i + 1] == '{' {
            let mut j = i + 2;
            let mut name = String::new();
            while j + 1 < chars.len() && !(chars[j] == '}' && chars[j + 1] == '}') {
                name.push(chars[j]);
                j += 1;
            }
            if j + 1 >= chars.len() {
                return Err("unterminated {{ reference".to_string());
            }
            out.push(Token::Ref(name.trim().to_string()));
            i = j + 2;
            continue;
        }

        if c == '"' || c == '\'' {
            let quote = c;
            let mut j = i + 1;
            let mut text = String::new();
            while j < chars.len() && chars[j] != quote {
                if chars[j] == '\\' && j + 1 < chars.len() {
                    j += 1;
                    text.push(chars[j]);
                } else {
                    text.push(chars[j]);
                }
                j += 1;
            }
            if j >= chars.len() {
                return Err("unterminated string literal".to_string());
            }
            out.push(Token::Text(text));
            i = j + 1;
            continue;
        }

        if c.is_ascii_digit() || (c == '.' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit()) {
            let mut j = i;
            let mut num = String::new();
            while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == '.') {
                num.push(chars[j]);
                j += 1;
            }
            let parsed = num.parse::<f64>().map_err(|_| format!("bad number '{}'", num))?;
            out.push(Token::Number(parsed));
            i = j;
            continue;
        }

        if c.is_alphabetic() || c == '_' {
            let mut j = i;
            let mut word = String::new();
            while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
                word.push(chars[j]);
                j += 1;
            }
            let upper = word.to_uppercase();
            if upper == "AND" || upper == "OR" || upper == "NOT" {
                out.push(Token::Op(upper));
            } else {
                out.push(Token::Ident(upper));
            }
            i = j;
            continue;
        }

        match c {
            '(' => { out.push(Token::LParen); i += 1; }
            ')' => { out.push(Token::RParen); i += 1; }
            ',' => { out.push(Token::Comma); i += 1; }
            '*' => {
                if i + 1 < chars.len() && chars[i + 1] == '*' {
                    out.push(Token::Op("**".to_string())); i += 2;
                } else {
                    out.push(Token::Op("*".to_string())); i += 1;
                }
            }
            '=' | '!' | '<' | '>' => {
                if i + 1 < chars.len() && chars[i + 1] == '=' {
                    out.push(Token::Op(format!("{}=", c))); i += 2;
                } else if c == '=' {
                    // A single '=' is almost always a typo for '==' rather than an
                    // assignment; accepting it silently would compare nothing.
                    return Err("use '==' to compare, not '='".to_string());
                } else if c == '!' {
                    return Err("'!' alone is not an operator — use NOT or '!='".to_string());
                } else {
                    out.push(Token::Op(c.to_string())); i += 1;
                }
            }
            '+' | '-' | '/' | '%' => { out.push(Token::Op(c.to_string())); i += 1; }
            _ => return Err(format!("unexpected character '{}'", c)),
        }
    }

    Ok(out)
}

/// Resolves a `{{reference}}` to a value. Returning `None` means the instance has
/// no value for it — the expression sees `Empty`, never a failure, so one blank
/// field cannot break the whole calculation.
pub type Resolver<'a> = dyn Fn(&str) -> Option<Value> + 'a;

struct Parser<'a> {
    tokens: Vec<Token>,
    pos: usize,
    resolve: &'a Resolver<'a>,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Token> { self.tokens.get(self.pos) }

    fn eat_op(&mut self, ops: &[&str]) -> Option<String> {
        if let Some(Token::Op(o)) = self.peek() {
            if ops.contains(&o.as_str()) {
                let o = o.clone();
                self.pos += 1;
                return Some(o);
            }
        }
        None
    }

    fn expect(&mut self, t: Token, what: &str) -> Result<(), String> {
        if self.peek() == Some(&t) {
            self.pos += 1;
            Ok(())
        } else {
            Err(format!("expected {}", what))
        }
    }

    fn expression(&mut self) -> Result<Value, String> { self.or_expr() }

    fn or_expr(&mut self) -> Result<Value, String> {
        let mut left = self.and_expr()?;
        while self.eat_op(&["OR"]).is_some() {
            let right = self.and_expr()?;
            left = Value::Bool(left.as_bool() || right.as_bool());
        }
        Ok(left)
    }

    fn and_expr(&mut self) -> Result<Value, String> {
        let mut left = self.not_expr()?;
        while self.eat_op(&["AND"]).is_some() {
            let right = self.not_expr()?;
            left = Value::Bool(left.as_bool() && right.as_bool());
        }
        Ok(left)
    }

    /// `NOT` negates the whole comparison to its right, so `NOT a == b` reads as
    /// "a is not b". Binding it tighter — at unary level, next to `-` — would make
    /// it negate only `a` and then compare that to `b`, which is never what anyone
    /// means and fails silently rather than erroring.
    fn not_expr(&mut self) -> Result<Value, String> {
        if self.eat_op(&["NOT"]).is_some() {
            return Ok(Value::Bool(!self.not_expr()?.as_bool()));
        }
        self.comparison()
    }

    fn comparison(&mut self) -> Result<Value, String> {
        let left = self.additive()?;
        if let Some(op) = self.eat_op(&["==", "!=", ">=", "<=", ">", "<"]) {
            let right = self.additive()?;
            return compare(&left, &op, &right);
        }
        Ok(left)
    }

    fn additive(&mut self) -> Result<Value, String> {
        let mut left = self.multiplicative()?;
        while let Some(op) = self.eat_op(&["+", "-"]) {
            let right = self.multiplicative()?;
            left = if op == "+" && (left.forces_text() || right.forces_text()) {
                // '+' joins when either side is text — this is what makes
                // "{{first}} + ' ' + {{last}}" read the way it looks.
                Value::Text(format!("{}{}", left.as_text(), right.as_text()))
            } else if op == "+" {
                Value::Number(left.as_number()? + right.as_number()?)
            } else {
                Value::Number(left.as_number()? - right.as_number()?)
            };
        }
        Ok(left)
    }

    fn multiplicative(&mut self) -> Result<Value, String> {
        let mut left = self.power()?;
        while let Some(op) = self.eat_op(&["*", "/", "%"]) {
            let right = self.power()?;
            let (a, b) = (left.as_number()?, right.as_number()?);
            left = match op.as_str() {
                "*" => Value::Number(a * b),
                "/" => {
                    if b == 0.0 { return Err("division by zero".to_string()); }
                    Value::Number(a / b)
                }
                _ => {
                    if b == 0.0 { return Err("modulo by zero".to_string()); }
                    Value::Number(a % b)
                }
            };
        }
        Ok(left)
    }

    fn power(&mut self) -> Result<Value, String> {
        let mut left = self.unary()?;
        while self.eat_op(&["**"]).is_some() {
            let right = self.unary()?;
            left = Value::Number(left.as_number()?.powf(right.as_number()?));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Value, String> {
        if self.eat_op(&["-"]).is_some() {
            return Ok(Value::Number(-self.unary()?.as_number()?));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Value, String> {
        match self.peek().cloned() {
            Some(Token::Number(n)) => { self.pos += 1; Ok(Value::Number(n)) }
            Some(Token::Text(t)) => { self.pos += 1; Ok(Value::Text(t)) }
            Some(Token::Ref(iri)) => {
                self.pos += 1;
                Ok((self.resolve)(&iri).unwrap_or(Value::Missing(iri)))
            }
            Some(Token::LParen) => {
                self.pos += 1;
                let v = self.expression()?;
                self.expect(Token::RParen, "')'")?;
                Ok(v)
            }
            Some(Token::Ident(name)) => {
                self.pos += 1;
                let args = self.call_args()?;
                call_function(&name, args)
            }
            other => Err(format!("unexpected {:?}", other)),
        }
    }

    fn call_args(&mut self) -> Result<Vec<Value>, String> {
        self.expect(Token::LParen, "'(' after a function name")?;
        let mut args = Vec::new();
        if self.peek() == Some(&Token::RParen) {
            self.pos += 1;
            return Ok(args);
        }
        loop {
            args.push(self.expression()?);
            match self.peek() {
                Some(Token::Comma) => { self.pos += 1; }
                Some(Token::RParen) => { self.pos += 1; break; }
                other => return Err(format!("expected ',' or ')' in arguments, got {:?}", other)),
            }
        }
        Ok(args)
    }
}

fn compare(left: &Value, op: &str, right: &Value) -> Result<Value, String> {
    // Text compares as text; anything else compares as a number, so a date string
    // and a number never silently compare as strings.
    let result = if left.forces_text() || right.forces_text() {
        let (a, b) = (left.as_text(), right.as_text());
        match op {
            "==" => a == b,
            "!=" => a != b,
            ">"  => a > b,
            "<"  => a < b,
            ">=" => a >= b,
            _    => a <= b,
        }
    } else {
        let (a, b) = (left.as_number()?, right.as_number()?);
        match op {
            "==" => (a - b).abs() < f64::EPSILON,
            "!=" => (a - b).abs() >= f64::EPSILON,
            ">"  => a > b,
            "<"  => a < b,
            ">=" => a >= b,
            _    => a <= b,
        }
    };
    Ok(Value::Bool(result))
}

fn arg(args: &[Value], i: usize, func: &str) -> Result<Value, String> {
    args.get(i).cloned().ok_or_else(|| format!("{} is missing argument {}", func, i + 1))
}

/// Parses a date from the shapes actually stored in the graph: an ISO date, or an
/// RFC3339 timestamp whose date half is taken.
fn parse_date(v: &Value) -> Result<NaiveDate, String> {
    let raw = v.as_text();
    let text = raw.trim();
    if text.is_empty() {
        return Err("empty date".to_string());
    }
    let head = text.split(['T', ' ']).next().unwrap_or(text);
    NaiveDate::parse_from_str(head, "%Y-%m-%d")
        .map_err(|_| format!("'{}' is not a date", text))
}

fn days_to_unit(days: i64, unit: &str) -> Value {
    match unit {
        "WEEKS" => Value::Number(days as f64 / 7.0),
        "MONTHS" => Value::Number(days as f64 / 30.436875),
        "YEARS" => Value::Number(days as f64 / 365.2425),
        _ => Value::Number(days as f64),
    }
}

fn unit_of(args: &[Value], i: usize) -> String {
    args.get(i).map(|v| v.as_text().to_uppercase()).unwrap_or_else(|| "DAYS".to_string())
}

fn call_function(name: &str, args: Vec<Value>) -> Result<Value, String> {
    match name {
        "IF" => {
            let cond = arg(&args, 0, "IF")?;
            let branch = if cond.as_bool() { arg(&args, 1, "IF")? } else { arg(&args, 2, "IF")? };
            Ok(branch)
        }
        "AND" => Ok(Value::Bool(args.iter().all(|a| a.as_bool()))),
        "OR" => Ok(Value::Bool(args.iter().any(|a| a.as_bool()))),
        "NOT" => Ok(Value::Bool(!arg(&args, 0, "NOT")?.as_bool())),
        "CONCAT" => Ok(Value::Text(args.iter().map(|a| a.as_text()).collect())),
        "TODAY" => Ok(Value::Text(Utc::now().date_naive().to_string())),
        "NOW" => Ok(Value::Text(Utc::now().to_rfc3339())),
        "DAYS_UNTIL" => {
            let target = parse_date(&arg(&args, 0, "DAYS_UNTIL")?)?;
            let days = (target - Utc::now().date_naive()).num_days();
            Ok(days_to_unit(days, &unit_of(&args, 1)))
        }
        "DAYS_SINCE" => {
            let target = parse_date(&arg(&args, 0, "DAYS_SINCE")?)?;
            let days = (Utc::now().date_naive() - target).num_days();
            Ok(days_to_unit(days, &unit_of(&args, 1)))
        }
        "DAYS_BETWEEN" => {
            let from = parse_date(&arg(&args, 0, "DAYS_BETWEEN")?)?;
            let to = parse_date(&arg(&args, 1, "DAYS_BETWEEN")?)?;
            let days = (to - from).num_days();
            Ok(days_to_unit(days, &unit_of(&args, 2)))
        }
        "YEAR" => Ok(Value::Number(parse_date(&arg(&args, 0, "YEAR")?)?.year() as f64)),
        "MONTH" => Ok(Value::Number(parse_date(&arg(&args, 0, "MONTH")?)?.month() as f64)),
        "DAY" => Ok(Value::Number(parse_date(&arg(&args, 0, "DAY")?)?.day() as f64)),
        "ROUND" => {
            let n = arg(&args, 0, "ROUND")?.as_number()?;
            let places = args.get(1).map(|v| v.as_number()).transpose()?.unwrap_or(0.0);
            let f = 10f64.powf(places);
            Ok(Value::Number((n * f).round() / f))
        }
        "ABS" => Ok(Value::Number(arg(&args, 0, "ABS")?.as_number()?.abs())),
        "MIN" => fold_numeric(&args, "MIN", f64::min),
        "MAX" => fold_numeric(&args, "MAX", f64::max),
        other => Err(format!("unknown function {}()", other)),
    }
}

fn fold_numeric(args: &[Value], name: &str, f: fn(f64, f64) -> f64) -> Result<Value, String> {
    if args.is_empty() {
        return Err(format!("{}() needs at least one argument", name));
    }
    let mut acc = args[0].as_number()?;
    for a in &args[1..] {
        acc = f(acc, a.as_number()?);
    }
    Ok(Value::Number(acc))
}

/// Evaluates `expression`, resolving `{{references}}` through `resolve`.
pub fn evaluate(expression: &str, resolve: &Resolver<'_>) -> Result<Value, String> {
    let tokens = tokenize(expression)?;
    if tokens.is_empty() {
        return Ok(Value::Missing(String::new()));
    }
    let mut parser = Parser { tokens, pos: 0, resolve };
    let value = parser.expression()?;
    if parser.pos != parser.tokens.len() {
        return Err(format!("unexpected trailing input at token {}", parser.pos + 1));
    }
    Ok(value)
}

/// Checks an expression's shape without needing an instance to evaluate against.
///
/// This runs when the property is DEFINED, so a typo is refused at the moment it
/// is written instead of surfacing later as a failure on every instance. It only
/// judges structure — whether a value is the right type is per-instance and is
/// reported there.
pub fn validate_syntax(expression: &str) -> Result<(), String> {
    let tokens = tokenize(expression)?;

    let mut depth = 0i32;
    for (i, token) in tokens.iter().enumerate() {
        match token {
            Token::LParen => depth += 1,
            Token::RParen => {
                depth -= 1;
                if depth < 0 {
                    return Err("closing ')' with nothing open".to_string());
                }
            }
            Token::Ident(name) => {
                if !KNOWN_FUNCTIONS.contains(&name.as_str()) {
                    return Err(format!(
                        "unknown function {}() — available: {}",
                        name,
                        KNOWN_FUNCTIONS.join(", ")
                    ));
                }
                if tokens.get(i + 1) != Some(&Token::LParen) {
                    return Err(format!("{} must be called with parentheses, as {}(…)", name, name));
                }
            }
            _ => {}
        }
    }
    if depth > 0 {
        return Err(format!("{} unclosed '('", depth));
    }
    Ok(())
}

const KNOWN_FUNCTIONS: &[&str] = &[
    "IF", "AND", "OR", "NOT", "CONCAT", "TODAY", "NOW",
    "DAYS_UNTIL", "DAYS_SINCE", "DAYS_BETWEEN",
    "YEAR", "MONTH", "DAY", "ROUND", "ABS", "MIN", "MAX",
];

/// True when the expression reads the clock, so its value goes stale on its own
/// and has to be recomputed on a schedule rather than only on write.
pub fn depends_on_clock(expression: &str) -> bool {
    let upper = expression.to_uppercase();
    ["TODAY(", "NOW(", "DAYS_UNTIL(", "DAYS_SINCE("]
        .iter()
        .any(|needle| upper.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(expr: &str) -> Result<Value, String> {
        evaluate(expr, &|_| None)
    }

    fn eval_with(expr: &str, pairs: &[(&str, Value)]) -> Result<Value, String> {
        let owned: Vec<(String, Value)> = pairs.iter()
            .map(|(k, v)| (k.to_string(), v.clone())).collect();
        evaluate(expr, &move |iri| {
            owned.iter().find(|(k, _)| k == iri).map(|(_, v)| v.clone())
        })
    }

    // ── arithmetic still behaves as it did ───────────────────────────────────

    #[test]
    fn arithmetic_keeps_precedence() {
        assert_eq!(eval("2 + 3 * 4").unwrap(), Value::Number(14.0));
        assert_eq!(eval("(2 + 3) * 4").unwrap(), Value::Number(20.0));
        assert_eq!(eval("2 ** 3").unwrap(), Value::Number(8.0));
        assert_eq!(eval("7 % 3").unwrap(), Value::Number(1.0));
    }

    #[test]
    fn a_negative_reference_no_longer_needs_wrapping() {
        // The old engine substituted text and broke on `a + -3`, so values had to
        // be parenthesised. A parsed reference is just a number.
        let v = eval_with("{{a}} + {{b}}", &[
            ("a", Value::Number(10.0)), ("b", Value::Number(-3.0)),
        ]).unwrap();
        assert_eq!(v, Value::Number(7.0));
    }

    #[test]
    fn division_by_zero_is_an_error_not_infinity() {
        assert!(eval("1 / 0").unwrap_err().contains("division by zero"));
    }

    // ── IF / THEN / ELSE ─────────────────────────────────────────────────────

    #[test]
    fn if_picks_the_branch_the_condition_selects() {
        let v = eval_with(r#"IF({{h}} > 40, "Large", "Small")"#, &[("h", Value::Number(60.0))]).unwrap();
        assert_eq!(v, Value::Text("Large".to_string()));
    }

    #[test]
    fn if_nests() {
        let expr = r#"IF({{h}} > 40, "Large", IF({{h}} > 8, "Medium", "Small"))"#;
        assert_eq!(eval_with(expr, &[("h", Value::Number(20.0))]).unwrap(), Value::Text("Medium".into()));
        assert_eq!(eval_with(expr, &[("h", Value::Number(2.0))]).unwrap(), Value::Text("Small".into()));
        assert_eq!(eval_with(expr, &[("h", Value::Number(80.0))]).unwrap(), Value::Text("Large".into()));
    }

    #[test]
    fn every_comparison_operator_works() {
        for (expr, expected) in [
            ("1 == 1", true), ("1 != 2", true), ("2 > 1", true),
            ("1 < 2", true), ("2 >= 2", true), ("1 <= 2", true),
            ("1 == 2", false), ("2 < 1", false),
        ] {
            assert_eq!(eval(expr).unwrap(), Value::Bool(expected), "{}", expr);
        }
    }

    #[test]
    fn logical_operators_combine_conditions() {
        assert_eq!(eval("1 == 1 AND 2 > 1").unwrap(), Value::Bool(true));
        assert_eq!(eval("1 == 2 OR 2 > 1").unwrap(), Value::Bool(true));
        assert_eq!(eval("NOT 1 == 2").unwrap(), Value::Bool(true));
        assert_eq!(eval("1 == 1 AND 1 == 2").unwrap(), Value::Bool(false));
    }

    #[test]
    fn comparison_works_on_text_and_booleans_too() {
        let v = eval_with(r#"IF({{state}} == "open", "still open", "closed")"#,
            &[("state", Value::Text("open".into()))]).unwrap();
        assert_eq!(v, Value::Text("still open".into()));

        let b = eval_with(r#"IF({{active}}, "Active", "Inactive")"#,
            &[("active", Value::Bool(false))]).unwrap();
        assert_eq!(b, Value::Text("Inactive".into()));
    }

    #[test]
    fn a_single_equals_is_refused_instead_of_silently_comparing_nothing() {
        assert!(eval("1 = 1").unwrap_err().contains("'=='"));
    }

    #[test]
    fn an_unknown_function_says_which_one() {
        let err = eval("FROBNICATE(1)").unwrap_err();
        assert!(err.contains("FROBNICATE"), "{}", err);
    }

    // ── string interpolation ─────────────────────────────────────────────────

    #[test]
    fn text_joins_with_plus() {
        let v = eval_with(r#"{{first}} + " " + {{last}}"#, &[
            ("first", Value::Text("Daniel".into())),
            ("last", Value::Text("Terra".into())),
        ]).unwrap();
        assert_eq!(v, Value::Text("Daniel Terra".into()));
    }

    #[test]
    fn a_missing_value_renders_as_an_empty_segment_not_a_failure() {
        let v = eval_with(r#"{{first}} + " " + {{last}}"#, &[
            ("first", Value::Text("Daniel".into())),
        ]).unwrap();
        assert_eq!(v, Value::Text("Daniel ".into()),
            "a blank field must leave a gap, not take the whole formula down");
    }

    #[test]
    fn a_number_inside_text_renders_without_decimal_noise() {
        let v = eval_with(r#""total: " + {{n}}"#, &[("n", Value::Number(42.0))]).unwrap();
        assert_eq!(v, Value::Text("total: 42".into()));
    }

    #[test]
    fn text_and_conditionals_combine() {
        let expr = r#"IF({{active}}, "Active", "Inactive") + ": " + {{name}}"#;
        let v = eval_with(expr, &[
            ("active", Value::Bool(true)),
            ("name", Value::Text("Foundation".into())),
        ]).unwrap();
        assert_eq!(v, Value::Text("Active: Foundation".into()));
    }

    #[test]
    fn concat_joins_any_number_of_pieces() {
        let v = eval_with(r#"CONCAT({{a}}, "-", {{b}})"#, &[
            ("a", Value::Text("x".into())), ("b", Value::Number(2.0)),
        ]).unwrap();
        assert_eq!(v, Value::Text("x-2".into()));
    }

    // ── dates ────────────────────────────────────────────────────────────────

    #[test]
    fn days_between_two_dates() {
        let v = eval_with("DAYS_BETWEEN({{a}}, {{b}})", &[
            ("a", Value::Text("2026-01-01".into())),
            ("b", Value::Text("2026-01-31".into())),
        ]).unwrap();
        assert_eq!(v, Value::Number(30.0));
    }

    #[test]
    fn days_between_accepts_a_unit() {
        let v = eval_with("DAYS_BETWEEN({{a}}, {{b}}, \"weeks\")", &[
            ("a", Value::Text("2026-01-01".into())),
            ("b", Value::Text("2026-01-15".into())),
        ]).unwrap();
        assert_eq!(v, Value::Number(2.0));
    }

    #[test]
    fn days_until_is_negative_for_a_date_already_past() {
        let v = eval_with("DAYS_UNTIL({{d}})", &[("d", Value::Text("2020-01-01".into()))]).unwrap();
        assert!(v.as_number().unwrap() < 0.0, "a past due date must read negative");
    }

    #[test]
    fn days_since_is_positive_for_a_date_already_past() {
        let v = eval_with("DAYS_SINCE({{d}})", &[("d", Value::Text("2020-01-01".into()))]).unwrap();
        assert!(v.as_number().unwrap() > 0.0);
    }

    #[test]
    fn a_timestamp_is_accepted_where_a_date_is_expected() {
        let v = eval_with("DAYS_BETWEEN({{a}}, {{b}})", &[
            ("a", Value::Text("2026-01-01T10:30:00+00:00".into())),
            ("b", Value::Text("2026-01-03".into())),
        ]).unwrap();
        assert_eq!(v, Value::Number(2.0));
    }

    #[test]
    fn today_returns_an_iso_date_usable_in_a_comparison() {
        let today = eval("TODAY()").unwrap().as_text();
        assert_eq!(today.len(), 10, "got {}", today);
        let overdue = eval_with(r#"IF({{due}} < TODAY(), "Overdue", "On track")"#,
            &[("due", Value::Text("2020-01-01".into()))]).unwrap();
        assert_eq!(overdue, Value::Text("Overdue".into()));
    }

    #[test]
    fn a_value_that_is_not_a_date_says_so() {
        let err = eval_with("DAYS_UNTIL({{d}})", &[("d", Value::Text("banana".into()))]).unwrap_err();
        assert!(err.contains("not a date"), "{}", err);
    }

    #[test]
    fn clock_dependence_is_detectable_so_the_value_can_be_refreshed_daily() {
        assert!(depends_on_clock("IF({{due}} < TODAY(), 1, 0)"));
        assert!(depends_on_clock("DAYS_SINCE({{createdAt}})"));
        assert!(!depends_on_clock("{{a}} + {{b}}"));
    }

    #[test]
    fn year_month_day_pull_the_parts_of_a_date() {
        let pairs = [("d", Value::Text("2026-08-24".into()))];
        assert_eq!(eval_with("YEAR({{d}})", &pairs).unwrap(), Value::Number(2026.0));
        assert_eq!(eval_with("MONTH({{d}})", &pairs).unwrap(), Value::Number(8.0));
        assert_eq!(eval_with("DAY({{d}})", &pairs).unwrap(), Value::Number(24.0));
    }

    #[test]
    fn round_abs_min_max() {
        assert_eq!(eval("ROUND(3.14159, 2)").unwrap(), Value::Number(3.14));
        assert_eq!(eval("ABS(0 - 5)").unwrap(), Value::Number(5.0));
        assert_eq!(eval("MIN(3, 1, 2)").unwrap(), Value::Number(1.0));
        assert_eq!(eval("MAX(3, 1, 2)").unwrap(), Value::Number(3.0));
    }

    #[test]
    fn trailing_input_is_reported_rather_than_ignored() {
        assert!(eval("1 + 2 3").unwrap_err().contains("trailing"));
    }

    #[test]
    fn syntax_check_catches_an_unclosed_paren() {
        assert!(validate_syntax("IF({{a}} > 1, \"x\", \"y\"").unwrap_err().contains("unclosed"));
    }

    #[test]
    fn syntax_check_catches_an_unknown_function_before_it_ever_runs() {
        let err = validate_syntax("FROB({{a}})").unwrap_err();
        assert!(err.contains("FROB") && err.contains("available"), "{}", err);
    }

    #[test]
    fn syntax_check_catches_an_unterminated_string() {
        assert!(validate_syntax("\"open").unwrap_err().contains("unterminated"));
    }

    #[test]
    fn syntax_check_accepts_the_examples_from_the_acceptance_criteria() {
        for expr in [
            r#"IF({{estimatedHours}} > 40, "Large", IF({{estimatedHours}} > 8, "Medium", "Small"))"#,
            r#"IF({{dueDate}} < TODAY(), "Overdue", "On track")"#,
            r#"{{firstName}} + " " + {{lastName}} + " — " + {{role}}"#,
            "DAYS_BETWEEN({{startDate}}, {{endDate}}, \"weeks\")",
            "{{a}} + {{b}} * 2",
        ] {
            assert!(validate_syntax(expr).is_ok(), "should accept: {}", expr);
        }
    }


    #[test]
    fn digits_stored_as_text_still_add_rather_than_join() {
        // Properties without a declared numeric range arrive as strings. Joining
        // them would turn 2 + 3 into "23" and nobody would see the error.
        let v = eval_with("{{a}} + {{b}}", &[
            ("a", Value::Text("2".into())), ("b", Value::Text("3".into())),
        ]).unwrap();
        assert_eq!(v, Value::Number(5.0));
    }

    #[test]
    fn a_missing_value_in_arithmetic_names_what_is_missing() {
        let err = eval_with("{{price}} + 1", &[]).unwrap_err();
        assert!(err.contains("price"), "the error must name the absent field: {}", err);
    }

    #[test]
    fn a_missing_value_next_to_text_still_renders_as_a_gap() {
        let v = eval_with(r#"{{a}} + "-" + {{b}}"#, &[("a", Value::Text("x".into()))]).unwrap();
        assert_eq!(v, Value::Text("x-".into()));
    }

}
