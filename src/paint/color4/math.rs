//! Numeric CSS math for color components, independent of layout or I/O.

#[derive(Clone, Copy, Debug, PartialEq)]
enum Unit {
    Number,
    Percent,
    Angle,
    Length,
}
#[derive(Clone, Debug)]
struct Quantity {
    value: f64,
    unit: Unit,
    source: Option<String>,
    precedence: u8,
}

/// Explicit length-unit factors supplied by computed style resolution.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Context {
    pub font: f64,
    pub root_font: f64,
    pub viewport: [f64; 2],
    pub container: [f64; 2],
}

fn parse(text: &str, context: Option<Context>) -> Option<Quantity> {
    let mut parser = Parser {
        text,
        offset: 0,
        context,
    };
    let value = parser.expression()?;
    parser.whitespace();
    (parser.offset == text.len()).then_some(value)
}

pub(super) fn evaluate(text: &str, scale: f64, hue: bool, context: Option<Context>) -> Option<f64> {
    let value = parse(text, context)?;
    match value.unit {
        Unit::Number => Some(value.value),
        Unit::Percent if !hue => Some(value.value * scale / 100.0),
        Unit::Angle if hue => Some(value.value),
        _ => None,
    }
}

pub(super) fn canonical(text: &str) -> Option<String> {
    let quantity = parse(text, None)?;
    Some(format!("calc({})", quantity.text(0)))
}

impl Quantity {
    fn literal(value: f64, unit: Unit) -> Self {
        Self {
            value,
            unit,
            source: None,
            precedence: 3,
        }
    }
    fn text(&self, precedence: u8) -> String {
        let text = self.source.clone().unwrap_or_else(|| {
            let value = if self.value.is_nan() {
                "NaN".into()
            } else if self.value == f64::INFINITY {
                "infinity".into()
            } else if self.value == f64::NEG_INFINITY {
                "-infinity".into()
            } else {
                super::css_number(self.value)
            };
            let unit = match self.unit {
                Unit::Number => "",
                Unit::Percent => "%",
                Unit::Angle => "deg",
                Unit::Length => "px",
            };
            format!("{value}{unit}")
        });
        if self.precedence < precedence {
            format!("({text})")
        } else {
            text
        }
    }
}

struct Parser<'a> {
    text: &'a str,
    offset: usize,
    context: Option<Context>,
}
impl Parser<'_> {
    fn whitespace(&mut self) {
        while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            self.offset += 1;
        }
    }
    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.offset).copied()
    }
    fn consume(&mut self, c: u8) -> bool {
        self.whitespace();
        if self.peek() == Some(c) {
            self.offset += 1;
            true
        } else {
            false
        }
    }
    fn expression(&mut self) -> Option<Quantity> {
        let mut left = self.product()?;
        loop {
            self.whitespace();
            let op = match self.peek() {
                Some(b'+') => 1.0,
                Some(b'-') => -1.0,
                _ => break,
            };
            // CSS addition/subtraction requires whitespace on both sides.
            if self.offset == 0
                || !self.text.as_bytes()[self.offset - 1].is_ascii_whitespace()
                || !self
                    .text
                    .as_bytes()
                    .get(self.offset + 1)?
                    .is_ascii_whitespace()
            {
                return None;
            }
            self.offset += 1;
            let right = self.product()?;
            if left.unit != right.unit {
                return None;
            }
            if left.source.is_some() || right.source.is_some() {
                left.source = Some(format!(
                    "{} {} {}",
                    left.text(1),
                    if op == 1.0 { "+" } else { "-" },
                    right.text(3)
                ));
                left.precedence = 1;
            }
            left.value += op * right.value;
        }
        Some(left)
    }
    fn product(&mut self) -> Option<Quantity> {
        let mut left = self.atom()?;
        loop {
            self.whitespace();
            let op = match self.peek() {
                Some(b'*') => b'*',
                Some(b'/') => b'/',
                _ => break,
            };
            self.offset += 1;
            let right = self.atom()?;
            let source = if left.source.is_some() || right.source.is_some() {
                let (first, second) =
                    if op == b'*' && left.source.is_some() && right.source.is_none() {
                        (&right, &left)
                    } else {
                        (&left, &right)
                    };
                Some(format!(
                    "{} {} {}",
                    first.text(2),
                    char::from(op),
                    second.text(2)
                ))
            } else {
                None
            };
            if op == b'*' {
                if left.unit == Unit::Number {
                    left.unit = right.unit;
                } else if right.unit != Unit::Number {
                    return None;
                }
                left.value *= right.value;
            } else {
                if right.unit == left.unit {
                    left.unit = Unit::Number;
                } else if right.unit != Unit::Number {
                    return None;
                }
                left.value /= right.value;
            }
            left.source = source;
            left.precedence = 2;
        }
        Some(left)
    }
    fn atom(&mut self) -> Option<Quantity> {
        self.whitespace();
        if self.consume(b'(') {
            let value = self.expression()?;
            return self.consume(b')').then_some(value);
        }
        if self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
            let start = self.offset;
            while self
                .peek()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == b'-')
            {
                self.offset += 1;
            }
            let name = &self.text[start..self.offset];
            if self.consume(b'(') {
                return self.function(name);
            }
            let value = match name {
                "infinity" => f64::INFINITY,
                "nan" => f64::NAN,
                "pi" => std::f64::consts::PI,
                "e" => std::f64::consts::E,
                _ => return None,
            };
            return Some(Quantity::literal(value, Unit::Number));
        }
        self.number()
    }

    fn number(&mut self) -> Option<Quantity> {
        let start = self.offset;
        if matches!(self.peek(), Some(b'+') | Some(b'-')) {
            self.offset += 1;
        }
        if self.text[self.offset..].starts_with("infinity") {
            self.offset += 8;
            return Some(Quantity::literal(
                if &self.text[start..start + 1] == "-" {
                    f64::NEG_INFINITY
                } else {
                    f64::INFINITY
                },
                Unit::Number,
            ));
        }
        while self.peek().is_some_and(|c| c.is_ascii_digit() || c == b'.') {
            self.offset += 1;
        }
        if matches!(self.peek(), Some(b'e'))
            && self
                .text
                .as_bytes()
                .get(self.offset + 1)
                .is_some_and(|c| c.is_ascii_digit() || matches!(c, b'+' | b'-'))
        {
            self.offset += 1;
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.offset += 1;
            }
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.offset += 1;
            }
        }
        let literal = &self.text[start..self.offset];
        if literal.split('e').next()?.ends_with('.') {
            return None;
        }
        let value = literal.parse::<f64>().ok()?;
        let start = self.offset;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == b'%')
        {
            self.offset += 1;
        }
        let (unit, factor) = match &self.text[start..self.offset] {
            "" => (Unit::Number, 1.0),
            "%" => (Unit::Percent, 1.0),
            "deg" => (Unit::Angle, 1.0),
            "grad" => (Unit::Angle, 0.9),
            "turn" => (Unit::Angle, 360.0),
            "rad" => (Unit::Angle, 180.0 / std::f64::consts::PI),
            name => {
                let factor = length_factor(name, self.context)?;
                return Some(Quantity {
                    value: value * factor,
                    unit: Unit::Length,
                    source: Some(format!("{}{name}", super::css_number(value))),
                    precedence: 3,
                });
            }
        };
        Some(Quantity::literal(value * factor, unit))
    }
    fn function(&mut self, name: &str) -> Option<Quantity> {
        let first = self.expression()?;
        if name == "calc" {
            return self.consume(b')').then_some(first);
        }
        let mut values = vec![first.clone()];
        while self.consume(b',') {
            values.push(self.expression()?);
        }
        if !self.consume(b')') || values.iter().any(|v| v.unit != first.unit) {
            return None;
        }
        let value = match name {
            "min" => values.iter().map(|v| v.value).fold(f64::INFINITY, f64::min),
            "max" => values
                .iter()
                .map(|v| v.value)
                .fold(f64::NEG_INFINITY, f64::max),
            "clamp" if values.len() == 3 => {
                values[0].value.max(values[1].value.min(values[2].value))
            }
            "abs" if values.len() == 1 => first.value.abs(),
            "sign" if values.len() == 1 => {
                return Some(Quantity {
                    value: if first.value == 0.0 {
                        0.0
                    } else {
                        first.value.signum()
                    },
                    unit: Unit::Number,
                    precedence: 3,
                    source: first
                        .source
                        .as_ref()
                        .map(|_| format!("sign({})", first.text(0))),
                });
            }
            _ => return None,
        };
        // CSS math propagates NaN; Rust's min/max ignore a single NaN operand.
        let value = if values.iter().any(|v| v.value.is_nan()) {
            f64::NAN
        } else {
            value
        };
        Some(Quantity {
            value,
            unit: first.unit,
            precedence: 3,
            source: values.iter().any(|v| v.source.is_some()).then(|| {
                format!(
                    "{name}({})",
                    values
                        .iter()
                        .map(|v| v.text(0))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }),
        })
    }
}

fn length_factor(unit: &str, context: Option<Context>) -> Option<f64> {
    let factor = match unit {
        "px" => 1.0,
        "in" => 96.0,
        "cm" => 96.0 / 2.54,
        "mm" => 96.0 / 25.4,
        "pt" => 96.0 / 72.0,
        "pc" => 16.0,
        "em" => context.map_or(0.0, |c| c.font),
        "rem" => context.map_or(0.0, |c| c.root_font),
        "vw" => context.map_or(0.0, |c| c.viewport[0] / 100.0),
        "vh" => context.map_or(0.0, |c| c.viewport[1] / 100.0),
        "cqw" | "cqi" => context.map_or(0.0, |c| c.container[0] / 100.0),
        "cqh" | "cqb" => context.map_or(0.0, |c| c.container[1] / 100.0),
        _ => return None,
    };
    Some(factor)
}

/// Splits only top-level whitespace and slash, leaving nested math intact.
pub(super) fn tokens(text: &str) -> Option<Vec<&str>> {
    let mut depth = 0_u32;
    let mut start = None;
    let mut result = Vec::new();
    for (i, c) in text.char_indices() {
        if depth == 0 && (c.is_ascii_whitespace() || c == '/') {
            if let Some(begin) = start.take() {
                result.push(&text[begin..i]);
            }
            if c == '/' {
                result.push(&text[i..i + 1]);
            }
            continue;
        }
        if depth == 0 && c == ',' {
            return None;
        }
        if start.is_none() {
            start = Some(i);
        }
        if c == '(' {
            depth = depth.checked_add(1)?;
            if depth > 32 {
                return None;
            }
        }
        if c == ')' {
            depth = depth.checked_sub(1)?;
        }
    }
    if depth != 0 {
        return None;
    }
    if let Some(begin) = start {
        result.push(&text[begin..]);
    }
    Some(result)
}

/// Identifies length dependencies that must survive specified serialization.
pub(super) fn has_relative_units(text: &str) -> bool {
    text.split(|c: char| !c.is_ascii_alphabetic()).any(|unit| {
        matches!(
            unit,
            "em" | "rem" | "vw" | "vh" | "cqw" | "cqh" | "cqi" | "cqb"
        )
    })
}
