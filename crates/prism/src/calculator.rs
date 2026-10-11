//! Original bounded integer calculator. No scripting, names, or code execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Empty,
    Syntax,
    DivisionByZero,
    Overflow,
    TooComplex,
}
impl Error {
    pub const fn message(self) -> &'static str {
        match self {
            Self::Empty => "Enter an expression: calc (2 + 3) * 4",
            Self::Syntax => "Use integers, parentheses, and + - * / % only.",
            Self::DivisionByZero => "Cannot divide by zero.",
            Self::Overflow => "Result exceeds the signed 64-bit integer range.",
            Self::TooComplex => "Expression exceeds the 96-byte or 16-level limit.",
        }
    }
}
const MAX_BYTES: usize = 96;
const MAX_DEPTH: usize = 16;
pub fn evaluate(text: &str) -> Result<i64, Error> {
    if text.len() > MAX_BYTES {
        return Err(Error::TooComplex);
    }
    let mut parser = Parser {
        bytes: text.as_bytes(),
        position: 0,
    };
    parser.space();
    if parser.position == parser.bytes.len() {
        return Err(Error::Empty);
    }
    let value = parser.expression(0)?;
    parser.space();
    if parser.position != parser.bytes.len() {
        return Err(Error::Syntax);
    }
    Ok(value)
}
struct Parser<'a> {
    bytes: &'a [u8],
    position: usize,
}
impl Parser<'_> {
    fn space(&mut self) {
        while self
            .bytes
            .get(self.position)
            .is_some_and(|b| *b == b' ' || *b == b'\t')
        {
            self.position += 1;
        }
    }
    fn peek(&mut self) -> Option<u8> {
        self.space();
        self.bytes.get(self.position).copied()
    }
    fn expression(&mut self, depth: usize) -> Result<i64, Error> {
        let mut value = self.term(depth)?;
        loop {
            match self.peek() {
                Some(b'+') => {
                    self.position += 1;
                    value = value
                        .checked_add(self.term(depth)?)
                        .ok_or(Error::Overflow)?;
                }
                Some(b'-') => {
                    self.position += 1;
                    value = value
                        .checked_sub(self.term(depth)?)
                        .ok_or(Error::Overflow)?;
                }
                _ => return Ok(value),
            }
        }
    }
    fn term(&mut self, depth: usize) -> Result<i64, Error> {
        let mut value = self.prefix(depth)?;
        loop {
            match self.peek() {
                Some(b'*') => {
                    self.position += 1;
                    value = value
                        .checked_mul(self.prefix(depth)?)
                        .ok_or(Error::Overflow)?;
                }
                Some(b'/') | Some(b'%') => {
                    let operator = self.bytes[self.position];
                    self.position += 1;
                    let rhs = self.prefix(depth)?;
                    if rhs == 0 {
                        return Err(Error::DivisionByZero);
                    }
                    value = if operator == b'/' {
                        value.checked_div(rhs)
                    } else {
                        value.checked_rem(rhs)
                    }
                    .ok_or(Error::Overflow)?;
                }
                _ => return Ok(value),
            }
        }
    }
    fn prefix(&mut self, depth: usize) -> Result<i64, Error> {
        if depth > MAX_DEPTH {
            return Err(Error::TooComplex);
        }
        match self.peek() {
            Some(b'+') | Some(b'-') => {
                let negative = self.bytes[self.position] == b'-';
                self.position += 1;
                if self.peek().is_some_and(|b| b.is_ascii_digit()) {
                    return self.number(negative);
                }
                let value = self.prefix(depth + 1)?;
                if negative {
                    value.checked_neg().ok_or(Error::Overflow)
                } else {
                    Ok(value)
                }
            }
            Some(b'(') => {
                self.position += 1;
                let value = self.expression(depth + 1)?;
                if self.peek() != Some(b')') {
                    return Err(Error::Syntax);
                }
                self.position += 1;
                Ok(value)
            }
            Some(b'0'..=b'9') => self.number(false),
            _ => Err(Error::Syntax),
        }
    }
    fn number(&mut self, negative: bool) -> Result<i64, Error> {
        let limit = i64::MAX as u64 + u64::from(negative);
        let mut value = 0u64;
        while let Some(&digit) = self.bytes.get(self.position) {
            if !digit.is_ascii_digit() {
                break;
            }
            value = value
                .checked_mul(10)
                .and_then(|v| v.checked_add((digit - b'0') as u64))
                .filter(|v| *v <= limit)
                .ok_or(Error::Overflow)?;
            self.position += 1;
        }
        if negative && value == 1 << 63 {
            Ok(i64::MIN)
        } else if negative {
            Ok(-(value as i64))
        } else {
            Ok(value as i64)
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn precedence_parentheses_signed_values_and_remainders() {
        for (input, expected) in [
            ("2+3*4", 14),
            ("(2+3)*4", 20),
            ("-10 / 3", -3),
            ("-10 % 3", -1),
            ("1--2", 3),
            (" -( -4 + +2 ) ", 2),
            ("-9223372036854775808", i64::MIN),
        ] {
            assert_eq!(evaluate(input), Ok(expected));
        }
    }
    #[test]
    fn invalid_input_overflow_and_division_do_not_execute() {
        for input in ["1/0", "3%(2-2)"] {
            assert_eq!(evaluate(input), Err(Error::DivisionByZero));
        }
        for input in [
            "9223372036854775807+1",
            "-9223372036854775808/-1",
            "-(-9223372036854775808)",
            "999999999999999999999999999",
        ] {
            assert_eq!(evaluate(input), Err(Error::Overflow));
        }
        for input in [
            "1 2",
            "1+",
            "()",
            "(1+2",
            "1)",
            "hello",
            "1;run spin",
            "2^3",
            "é",
        ] {
            assert_eq!(evaluate(input), Err(Error::Syntax));
        }
        assert_eq!(evaluate(" \t"), Err(Error::Empty));
    }
    #[test]
    fn bounded_recursion_and_input() {
        assert_eq!(
            evaluate("((((((((((((((((((1))))))))))))))))))"),
            Err(Error::TooComplex)
        );
        let bytes = [b'1'; 97];
        assert_eq!(
            evaluate(core::str::from_utf8(&bytes).unwrap()),
            Err(Error::TooComplex)
        );
    }
}
