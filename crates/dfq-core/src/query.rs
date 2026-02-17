use crate::error::{Error, Span};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub source: String,
    pub segments: Vec<Segment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Ident(String),
    Indexed { ident: String, index: Index },
    Function { ident: String, args: Vec<Arg> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Index {
    Position(usize),
    Wildcard,
    Key(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arg {
    String(String),
    Ident(String),
    Number(i64),
}

impl Query {
    pub fn parse(input: &str) -> Result<Self, Error> {
        let mut parser = Parser::new(input);
        let segments = parser.parse_query()?;
        Ok(Self {
            source: input.to_string(),
            segments,
        })
    }
}

pub fn parse_query(input: &str) -> Result<Query, Error> {
    Query::parse(input)
}

struct Parser<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    fn parse_query(&mut self) -> Result<Vec<Segment>, Error> {
        self.skip_ws();
        if self.is_eof() {
            return Err(self.error("query is empty"));
        }

        let mut segments = vec![self.parse_segment()?];
        loop {
            self.skip_ws();
            if self.is_eof() {
                break;
            }

            self.expect_byte(b'.')?;
            self.skip_ws();
            segments.push(self.parse_segment()?);
        }

        Ok(segments)
    }

    fn parse_segment(&mut self) -> Result<Segment, Error> {
        let ident = self.parse_ident()?;
        self.skip_ws();

        match self.peek_byte() {
            Some(b'[') => {
                self.bump_byte();
                self.skip_ws();

                let index = match self.peek_byte() {
                    Some(b'*') => {
                        self.bump_byte();
                        Index::Wildcard
                    }
                    Some(b'"') | Some(b'\'') => Index::Key(self.parse_string()?),
                    Some(byte) if byte.is_ascii_digit() => Index::Position(self.parse_usize()?),
                    _ => return Err(self.error("expected numeric, wildcard, or string index")),
                };

                self.skip_ws();
                self.expect_byte(b']')?;
                Ok(Segment::Indexed { ident, index })
            }
            Some(b'(') => {
                self.bump_byte();
                let mut args = Vec::new();

                self.skip_ws();
                if self.peek_byte() != Some(b')') {
                    loop {
                        args.push(self.parse_arg()?);
                        self.skip_ws();

                        match self.peek_byte() {
                            Some(b',') => {
                                self.bump_byte();
                                self.skip_ws();
                            }
                            Some(b')') => break,
                            _ => return Err(self.error("expected ',' or ')'")),
                        }
                    }
                }

                self.expect_byte(b')')?;
                Ok(Segment::Function { ident, args })
            }
            _ => Ok(Segment::Ident(ident)),
        }
    }

    fn parse_arg(&mut self) -> Result<Arg, Error> {
        self.skip_ws();
        match self.peek_byte() {
            Some(b'"') | Some(b'\'') => self.parse_string().map(Arg::String),
            Some(byte) if byte.is_ascii_digit() || byte == b'-' => {
                self.parse_i64().map(Arg::Number)
            }
            Some(_) => self.parse_ident().map(Arg::Ident),
            None => Err(self.error("expected function argument")),
        }
    }

    fn parse_ident(&mut self) -> Result<String, Error> {
        let start = self.pos;
        let Some(first) = self.peek_byte() else {
            return Err(self.error("expected identifier"));
        };

        if !(first.is_ascii_alphabetic() || first == b'_') {
            return Err(self.error("identifier must start with [A-Za-z_]"));
        }
        self.bump_byte();

        while let Some(byte) = self.peek_byte() {
            if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-' {
                self.bump_byte();
            } else {
                break;
            }
        }

        Ok(self.input[start..self.pos].to_string())
    }

    fn parse_usize(&mut self) -> Result<usize, Error> {
        let start = self.pos;
        while let Some(byte) = self.peek_byte() {
            if byte.is_ascii_digit() {
                self.bump_byte();
            } else {
                break;
            }
        }

        if start == self.pos {
            return Err(self.error("expected numeric index"));
        }

        self.input[start..self.pos]
            .parse::<usize>()
            .map_err(|_| self.error("invalid numeric index"))
    }

    fn parse_i64(&mut self) -> Result<i64, Error> {
        let start = self.pos;
        if self.peek_byte() == Some(b'-') {
            self.bump_byte();
        }
        while let Some(byte) = self.peek_byte() {
            if byte.is_ascii_digit() {
                self.bump_byte();
            } else {
                break;
            }
        }

        if start == self.pos {
            return Err(self.error("expected number"));
        }

        self.input[start..self.pos]
            .parse::<i64>()
            .map_err(|_| self.error("invalid number"))
    }

    fn parse_string(&mut self) -> Result<String, Error> {
        let Some(quote) = self.peek_byte() else {
            return Err(self.error("expected string"));
        };
        if quote != b'"' && quote != b'\'' {
            return Err(self.error("expected string quote"));
        }
        self.bump_byte();

        let mut out = String::new();
        loop {
            let Some(byte) = self.peek_byte() else {
                return Err(self.error("unterminated string"));
            };
            self.bump_byte();

            if byte == quote {
                return Ok(out);
            }

            if byte == b'\\' {
                let Some(escaped) = self.peek_byte() else {
                    return Err(self.error("unterminated escape sequence"));
                };
                self.bump_byte();
                match escaped {
                    b'"' => out.push('"'),
                    b'\'' => out.push('\''),
                    b'\\' => out.push('\\'),
                    b'n' => out.push('\n'),
                    b't' => out.push('\t'),
                    _ => return Err(self.error("unsupported string escape")),
                }
                continue;
            }

            out.push(byte as char);
        }
    }

    fn skip_ws(&mut self) {
        while let Some(byte) = self.peek_byte() {
            if byte.is_ascii_whitespace() {
                self.bump_byte();
            } else {
                break;
            }
        }
    }

    fn expect_byte(&mut self, expected: u8) -> Result<(), Error> {
        match self.peek_byte() {
            Some(byte) if byte == expected => {
                self.bump_byte();
                Ok(())
            }
            _ => Err(self.error(format!("expected '{}'", expected as char))),
        }
    }

    fn peek_byte(&self) -> Option<u8> {
        self.input.as_bytes().get(self.pos).copied()
    }

    fn bump_byte(&mut self) {
        self.pos += 1;
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.input.len()
    }

    fn error(&self, msg: impl Into<String>) -> Error {
        let span = Some(Span::new(self.pos, self.pos.saturating_add(1)));
        Error::query_parse(msg, span)
    }
}

#[cfg(test)]
mod tests {
    use crate::query::{Arg, Index, Segment, parse_query};

    #[test]
    fn parses_stage_name_index() {
        let query = parse_query(r#"STAGE["builder"].ARG.VERSION"#).expect("query should parse");
        assert_eq!(
            query.segments,
            vec![
                Segment::Indexed {
                    ident: "STAGE".to_string(),
                    index: Index::Key("builder".to_string())
                },
                Segment::Ident("ARG".to_string()),
                Segment::Ident("VERSION".to_string())
            ]
        );
    }

    #[test]
    fn parses_wildcard() {
        let query = parse_query("FROM[*].RESOLVED").expect("query should parse");
        assert_eq!(
            query.segments,
            vec![
                Segment::Indexed {
                    ident: "FROM".to_string(),
                    index: Index::Wildcard,
                },
                Segment::Ident("RESOLVED".to_string()),
            ]
        );
    }

    #[test]
    fn parses_resolve_call() {
        let query = parse_query(r#"RESOLVE("x:${VERSION}", 1, foo)"#).expect("query should parse");
        assert_eq!(
            query.segments,
            vec![Segment::Function {
                ident: "RESOLVE".to_string(),
                args: vec![
                    Arg::String("x:${VERSION}".to_string()),
                    Arg::Number(1),
                    Arg::Ident("foo".to_string()),
                ],
            }]
        );
    }

    #[test]
    fn rejects_invalid_query() {
        let error = parse_query("ARG.").expect_err("query should fail");
        let rendered = error.to_string();
        assert!(rendered.contains("query parse error"));
    }
}
