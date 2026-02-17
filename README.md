# dfq

[![uses nix](https://img.shields.io/badge/uses-nix-%237EBAE4)](https://nixos.org/)
![rust](https://img.shields.io/badge/Rust-1.95%2B-orange.svg)

`dfq` is a CLI for querying Dockerfiles like structured data.

It parses a Dockerfile once, then evaluates query expressions over global `ARG`s, `FROM` parents, stage metadata, and `RUN` instructions.

## Quickstart

Build:

```bash
cargo build -p dfq
```

Run:

```bash
cargo run -p dfq -- <QUERY>
```

## Example Dockerfile

```dockerfile
ARG VERSION=0.5.13
FROM alpine:${VERSION} AS builder
ARG VERSION=1.0
RUN apk add --no-cache curl
RUN echo "build complete"
FROM builder
RUN echo "runtime"
```

## Query language overview

Forms:

- `IDENT` (namespace or field)
- `IDENT[index]` where index is number, `*`, or string key (`"builder"`)
- `IDENT(args...)` function/directive segments
- Dot chaining: `A.B[0].C("x")`

Built-in namespaces:

- `ARG`
- `FROM`
- `STAGE`
- `RUN`
- `RESOLVE("...")`

## Common examples

Global arg:

```bash
dfq ARG.VERSION
# 0.5.13
```

Override arg during query:

```bash
dfq --build-arg VERSION=1.2.3 FROM[0].RESOLVED
# alpine:1.2.3
```

Stage arg lookup:

```bash
dfq 'STAGE["builder"].ARG.VERSION'
# 1.0
```

Stage dump (JSON):

```bash
dfq --json STAGE
```

Resolve arbitrary string in global scope:

```bash
dfq 'RESOLVE("image:${VERSION}")'
# image:0.5.13
```

Strict mode for missing vars:

```bash
dfq --strict 'RESOLVE("x:${NOPE}")'
# exit 5
```

## RUN queries (grep-friendly)

Text mode streams scalar arrays line-by-line, so this works well:

```bash
dfq RUN[*] | grep apk
# RUN apk add --no-cache curl
```

More RUN examples:

```bash
dfq RUN.COUNT
# 3

dfq RUN[0]
# RUN apk add --no-cache curl

dfq RUN[0].COMMAND
# apk add --no-cache curl

dfq RUN[0].STAGE
# 0

dfq --json 'RUN[*].SPAN'
# [{"start":...,"end":...}, ...]
```

Directive-style RUN helpers:

```bash
dfq 'RUN.GREP("apk")'
# RUN apk add --no-cache curl

dfq 'RUN[*].GREP("apk")'
# RUN apk add --no-cache curl

dfq 'RUN.GREP("apk").COUNT'
# 1

dfq 'RUN.CONTAINS("runtime")'
# true
```

## Output behavior

- Scalars print as plain text by default.
- Scalar arrays print one item per line in text mode.
- `--json` prints a JSON envelope:

```json
{"query":"...","value":...,"type":"...","meta":{...}}
```

Structured non-scalar outputs (objects / object arrays) require `--json`.

## CLI flags

- `-f, --file <PATH>`: read Dockerfile from path, default is `Dockerfile`
- `--stdin`: read Dockerfile content from stdin
- `--build-arg K[=V]`: override build args, repeatable
- `--json`: emit JSON envelope output
- `--raw`: no trailing newline for scalar output
- `--null`: use NUL terminators for scalar output
- `--strict`: fail on missing interpolation variables
- `--show-missing`: include `missing_vars` and `used_vars` in JSON metadata
- `-v, --verbose`: debug info to stderr
- `--context <PATH>`: reserved compatibility flag in v1, accepted but ignored

## Interpolation support

Supported:

- `$VAR`
- `${VAR}`

Not supported:

- `${VAR:-default}`, `${VAR:+alt}`, and similar shell parameter expansion
- command substitution and backticks

## Known limits (v1)

- No execution/evaluation of filesystem context from `RUN`, `COPY`, `ADD`.
- `FROM[n].RESOLVED` follows global meta-arg semantics.
- Duplicate stage names make `STAGE["name"]` ambiguous (exit `5`).

## Exit codes

- `0`: success
- `2`: query parse error
- `3`: Dockerfile parse error
- `4`: query path not found
- `5`: evaluation error
- `6`: IO error
- `64`: usage error
