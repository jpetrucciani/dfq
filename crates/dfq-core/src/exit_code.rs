use crate::error::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ExitCode {
    Success = 0,
    QueryParse = 2,
    DockerfileParse = 3,
    NotFound = 4,
    Eval = 5,
    Io = 6,
    Usage = 64,
}

impl ExitCode {
    pub const fn as_i32(self) -> i32 {
        self as i32
    }
}

impl From<&Error> for ExitCode {
    fn from(value: &Error) -> Self {
        match value {
            Error::QueryParse { .. } => Self::QueryParse,
            Error::DockerfileParse { .. } => Self::DockerfileParse,
            Error::NotFound { .. } => Self::NotFound,
            Error::Eval { .. } => Self::Eval,
            Error::Io { .. } => Self::Io,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::error::Error;
    use crate::exit_code::ExitCode;

    #[test]
    fn maps_error_variants_to_exit_codes() {
        assert_eq!(
            ExitCode::from(&Error::query_parse("bad query", None)),
            ExitCode::QueryParse
        );
        assert_eq!(
            ExitCode::from(&Error::dockerfile_parse("bad dockerfile")),
            ExitCode::DockerfileParse
        );
        assert_eq!(
            ExitCode::from(&Error::not_found("ARG.NOPE")),
            ExitCode::NotFound
        );
        assert_eq!(ExitCode::from(&Error::eval("cycle")), ExitCode::Eval);
        assert_eq!(
            ExitCode::from(&Error::io(std::io::Error::from(std::io::ErrorKind::Other))),
            ExitCode::Io
        );
    }
}
