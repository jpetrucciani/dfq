use std::collections::BTreeMap;
use std::io::{Read, Write};

use clap::{CommandFactory, Parser, Subcommand, ValueEnum, error::ErrorKind};
use clap_complete::{
    generate,
    shells::{Bash, Fish, Zsh},
};

use dfq_core::Error;
use dfq_core::eval::Evaluator;
use dfq_core::exit_code::ExitCode;
use dfq_core::parser::parse_dockerfile;
use dfq_core::query::parse_query;
use dfq_core::value::Value;

const LONG_ABOUT: &str = "Parse a Dockerfile and query resolved values such as ARGs, FROM parents, STAGE metadata, and RUN commands.\n\nUse --json for structured queries. In text mode, scalar arrays are streamed one item per line, so queries like RUN[*] can be piped to grep.";

const AFTER_HELP: &str = "Examples:\n  dfq ARG.VERSION\n  dfq --build-arg VERSION=1.2.3 FROM[0].RESOLVED\n  dfq --json STAGE\n  dfq 'STAGE[\"builder\"].ARG.VERSION'\n  dfq RUN[*] | grep apt-get\n  dfq 'RUN.GREP(\"apt-get\")'\n  dfq 'RUN[*].GREP(\"apt-get\")'\n  dfq 'RUN.GREP(\"apt-get\").COUNT'\n  dfq --json 'RUN[*].SPAN'\n  dfq 'RESOLVE(\"img:${VERSION}\")'";

fn main() {
    let code = match run() {
        Ok(()) => ExitCode::Success,
        Err(app_error) => {
            if !app_error.message.is_empty() {
                eprintln!("{}", app_error.message);
            }
            app_error.code
        }
    };
    std::process::exit(code.as_i32());
}

#[derive(Debug)]
struct AppError {
    code: ExitCode,
    message: String,
}

impl AppError {
    fn usage(message: impl Into<String>) -> Self {
        Self {
            code: ExitCode::Usage,
            message: message.into(),
        }
    }
}

impl From<Error> for AppError {
    fn from(value: Error) -> Self {
        Self {
            code: ExitCode::from(&value),
            message: value.to_string(),
        }
    }
}

#[derive(Debug, Subcommand)]
enum Commands {
    #[command(about = "Generate shell completion scripts")]
    Completion {
        #[arg(value_enum, value_name = "SHELL")]
        shell: CompletionShell,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum CompletionShell {
    Bash,
    Zsh,
    Fish,
}

#[derive(Debug, Parser)]
#[command(
    name = "dfq",
    about = "Query Dockerfiles like data",
    long_about = LONG_ABOUT,
    after_help = AFTER_HELP
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    #[arg(
        short = 'f',
        long = "file",
        value_name = "PATH",
        help = "Read Dockerfile from a file path",
        long_help = "Read Dockerfile content from PATH. If omitted, dfq uses ./Dockerfile."
    )]
    file: Option<String>,

    #[arg(
        long = "stdin",
        help = "Read Dockerfile from stdin",
        long_help = "Read Dockerfile content from stdin. This conflicts with --file."
    )]
    stdin: bool,

    #[arg(
        long = "context",
        hide = true,
        value_name = "PATH",
        help = "Reserved compatibility flag",
        long_help = "Reserved compatibility flag for future build-context-aware behavior. Accepted in v1 but ignored."
    )]
    context: Option<String>,

    #[arg(
        long = "build-arg",
        value_name = "K[=V]",
        help = "Override build ARG values",
        long_help = "Override ARG values. Repeat the flag as needed. Use KEY=VALUE or KEY (empty value). Overrides take precedence over stage defaults and global defaults."
    )]
    build_args: Vec<String>,

    #[arg(
        long = "json",
        help = "Emit a JSON envelope",
        long_help = "Emit a JSON envelope with { query, value, type, meta }. Required for object/array results unless the result is an array of scalars."
    )]
    json: bool,

    #[arg(
        long = "raw",
        help = "Disable trailing newline for scalar output",
        long_help = "Disable trailing newline for scalar output. For scalar arrays in text mode, separators remain between items but no final separator is emitted."
    )]
    raw: bool,

    #[arg(
        long = "null",
        help = "Use NUL terminators for scalar output",
        long_help = "Use '\\0' as the scalar terminator. For scalar arrays in text mode, each item is terminated by '\\0'."
    )]
    nul: bool,

    #[arg(
        long = "strict",
        help = "Error on missing interpolation variables",
        long_help = "Error when interpolation references missing variables instead of expanding them to empty strings."
    )]
    strict: bool,

    #[arg(
        long = "show-missing",
        help = "Include interpolation metadata in JSON",
        long_help = "Include missing_vars and used_vars in JSON metadata. Applies to --json output."
    )]
    show_missing: bool,

    #[arg(
        short = 'v',
        long = "verbose",
        help = "Print debug details to stderr",
        long_help = "Print extra debug details to stderr (scope and value type) while keeping stdout clean for result output."
    )]
    verbose: bool,

    #[arg(
        value_name = "QUERY",
        help = "Query expression",
        long_help = "Query expression to evaluate. Examples: ARG.VERSION, FROM[0].RESOLVED, STAGE[\"builder\"].ARG.VERSION, RUN[*], RUN.GREP(\"apt-get\"), RUN[*].GREP(\"apt-get\")."
    )]
    query: Option<String>,
}

fn run() -> Result<(), AppError> {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            if matches!(
                err.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                print!("{err}");
                return Ok(());
            }
            return Err(AppError::usage(err.to_string()));
        }
    };

    execute(cli)
}

fn execute(cli: Cli) -> Result<(), AppError> {
    if let Some(command) = cli.command {
        return execute_command(command);
    }

    validate_cli(&cli)?;
    let _reserved_context = &cli.context;

    let dockerfile = read_dockerfile(&cli)?;
    let model = parse_dockerfile(&dockerfile)?;
    let query_source = cli.query.as_deref().ok_or_else(|| {
        AppError::usage("the following required arguments were not provided:\n  <QUERY>")
    })?;
    let query = parse_query(query_source)?;

    let overrides = cli
        .build_args
        .iter()
        .map(|arg| parse_build_arg(arg))
        .collect::<Result<BTreeMap<_, _>, _>>()?;

    let evaluator = Evaluator::new(&model, &overrides, cli.strict);
    let result = evaluator.evaluate(&query)?;

    if cli.verbose {
        eprintln!(
            "scope={} type={}",
            result.meta.scope.as_str(),
            result.value.kind()
        );
    }

    if cli.json {
        let include_var_details = cli.show_missing || cli.verbose;
        let payload = json_envelope(
            &query.source,
            result.value,
            result.meta.to_value(include_var_details),
        );
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(payload.as_bytes()).map_err(Error::from)?;
        stdout.write_all(b"\n").map_err(Error::from)?;
        return Ok(());
    }

    if let Some(text) = result.value.render_scalar() {
        write_scalar_text(&text, cli.raw, cli.nul)?;
        return Ok(());
    }

    if let Some(items) = result.value.render_scalar_array() {
        write_scalar_items(&items, cli.raw, cli.nul)?;
        return Ok(());
    }

    Err(AppError::usage("structured result requires --json"))
}

fn execute_command(command: Commands) -> Result<(), AppError> {
    match command {
        Commands::Completion { shell } => write_completion(shell),
    }
}

fn write_completion(shell: CompletionShell) -> Result<(), AppError> {
    let mut command = Cli::command();
    let mut stdout = std::io::stdout().lock();
    match shell {
        CompletionShell::Bash => generate(Bash, &mut command, "dfq", &mut stdout),
        CompletionShell::Zsh => generate(Zsh, &mut command, "dfq", &mut stdout),
        CompletionShell::Fish => generate(Fish, &mut command, "dfq", &mut stdout),
    }
    stdout.flush().map_err(Error::from).map_err(AppError::from)
}

fn write_scalar_text(text: &str, raw: bool, nul: bool) -> Result<(), AppError> {
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(text.as_bytes()).map_err(Error::from)?;
    if nul {
        stdout.write_all(&[0]).map_err(Error::from)?;
    } else if !raw {
        stdout.write_all(b"\n").map_err(Error::from)?;
    }
    Ok(())
}

fn write_scalar_items(items: &[String], raw: bool, nul: bool) -> Result<(), AppError> {
    let mut stdout = std::io::stdout().lock();

    if nul {
        for item in items {
            stdout.write_all(item.as_bytes()).map_err(Error::from)?;
            stdout.write_all(&[0]).map_err(Error::from)?;
        }
        return Ok(());
    }

    for (index, item) in items.iter().enumerate() {
        stdout.write_all(item.as_bytes()).map_err(Error::from)?;
        let is_last = index + 1 == items.len();
        if !is_last || !raw {
            stdout.write_all(b"\n").map_err(Error::from)?;
        }
    }

    Ok(())
}

fn json_envelope(query: &str, value: Value, meta: Value) -> String {
    let mut out = BTreeMap::new();
    out.insert("query".to_string(), Value::String(query.to_string()));
    out.insert("value".to_string(), value.clone());
    out.insert("type".to_string(), Value::String(value.kind().to_string()));
    out.insert("meta".to_string(), meta);
    Value::Object(out).to_json_string()
}

fn validate_cli(cli: &Cli) -> Result<(), AppError> {
    if cli.command.is_none() && cli.query.is_none() {
        return Err(AppError::usage(
            "the following required arguments were not provided:\n  <QUERY>",
        ));
    }
    if cli.stdin && cli.file.is_some() {
        return Err(AppError::usage("--stdin is mutually exclusive with --file"));
    }
    if cli.json && cli.raw {
        return Err(AppError::usage("--json is mutually exclusive with --raw"));
    }
    if cli.json && cli.nul {
        return Err(AppError::usage("--json is mutually exclusive with --null"));
    }
    if cli.raw && cli.nul {
        return Err(AppError::usage("--raw is mutually exclusive with --null"));
    }
    Ok(())
}

fn read_dockerfile(cli: &Cli) -> Result<String, AppError> {
    if cli.stdin {
        let mut input = String::new();
        std::io::stdin()
            .read_to_string(&mut input)
            .map_err(Error::from)?;
        return Ok(input);
    }

    let path = cli.file.as_deref().unwrap_or("Dockerfile");
    std::fs::read_to_string(path)
        .map_err(Error::from)
        .map_err(AppError::from)
}

fn parse_build_arg(input: &str) -> Result<(String, String), AppError> {
    if let Some((key, value)) = input.split_once('=') {
        let key = key.trim();
        if key.is_empty() {
            return Err(AppError::usage("build-arg key cannot be empty"));
        }
        return Ok((key.to_string(), value.to_string()));
    }

    let key = input.trim();
    if key.is_empty() {
        return Err(AppError::usage("build-arg key cannot be empty"));
    }
    Ok((key.to_string(), String::new()))
}
