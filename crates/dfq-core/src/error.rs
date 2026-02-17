use core::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

#[derive(Debug)]
pub enum Error {
    QueryParse { msg: String, span: Option<Span> },
    DockerfileParse { msg: String },
    NotFound { path: String },
    Eval { msg: String },
    Io { source: std::io::Error },
}

impl Error {
    pub fn query_parse(msg: impl Into<String>, span: Option<Span>) -> Self {
        Self::QueryParse {
            msg: msg.into(),
            span,
        }
    }

    pub fn dockerfile_parse(msg: impl Into<String>) -> Self {
        Self::DockerfileParse { msg: msg.into() }
    }

    pub fn not_found(path: impl Into<String>) -> Self {
        Self::NotFound { path: path.into() }
    }

    pub fn eval(msg: impl Into<String>) -> Self {
        Self::Eval { msg: msg.into() }
    }

    pub fn io(source: std::io::Error) -> Self {
        Self::Io { source }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::QueryParse { msg, span } => {
                if let Some(span) = span {
                    write!(
                        f,
                        "query parse error at {}..{}: {msg}",
                        span.start, span.end
                    )
                } else {
                    write!(f, "query parse error: {msg}")
                }
            }
            Self::DockerfileParse { msg } => write!(f, "dockerfile parse error: {msg}"),
            Self::NotFound { path } => write!(f, "not found: {path}"),
            Self::Eval { msg } => write!(f, "evaluation error: {msg}"),
            Self::Io { source } => write!(f, "io error: {source}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        if let Self::Io { source } = self {
            Some(source)
        } else {
            None
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::io(value)
    }
}
