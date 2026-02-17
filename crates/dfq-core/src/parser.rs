use dockerfile_parser::{Dockerfile, Instruction as ParsedInstruction, StageParent};

use crate::error::Error;
use crate::model::{DockerfileModel, Instruction, Parent, Stage};

pub fn parse_dockerfile(input: &str) -> Result<DockerfileModel, Error> {
    let dockerfile =
        Dockerfile::parse(input).map_err(|err| Error::dockerfile_parse(format!("{err}")))?;

    let line_index = LineIndex::new(&dockerfile.content);

    let mut model = DockerfileModel::default();
    for arg in &dockerfile.global_args {
        model.global_args.insert(
            arg.name.content.clone(),
            arg.value.as_ref().map(|value| value.content.clone()),
        );
    }

    model.raw_instructions = dockerfile
        .instructions
        .iter()
        .map(|instruction| map_instruction(instruction, &dockerfile.content, &line_index))
        .collect::<Result<Vec<_>, _>>()?;

    for parsed_stage in dockerfile.stages().iter() {
        let from = parsed_stage
            .instructions
            .first()
            .and_then(|instruction| instruction.as_from())
            .ok_or_else(|| {
                Error::dockerfile_parse(format!(
                    "stage {} does not start with FROM",
                    parsed_stage.index
                ))
            })?;

        let parent_token = from.image.content.clone();
        let parent = match parsed_stage.parent {
            StageParent::Scratch => Parent::Scratch,
            StageParent::Stage(_) => Parent::StageRef(parent_token),
            StageParent::Image(_) => Parent::Image(parent_token),
        };

        let mut stage = Stage {
            index: parsed_stage.index,
            name: from.alias.as_ref().map(|alias| alias.content.clone()),
            parent,
            arg_defaults: Default::default(),
            instructions: Vec::new(),
        };

        for instruction in parsed_stage.instructions.iter().skip(1) {
            if let Some(arg) = instruction.as_arg() {
                stage.arg_defaults.insert(
                    arg.name.content.clone(),
                    arg.value.as_ref().map(|value| value.content.clone()),
                );
            }
            stage.instructions.push(map_instruction(
                instruction,
                &dockerfile.content,
                &line_index,
            )?);
        }

        model.stages.push(stage);
    }

    Ok(model)
}

fn map_instruction(
    instruction: &ParsedInstruction,
    content: &str,
    line_index: &LineIndex,
) -> Result<Instruction, Error> {
    let span = instruction.span();
    let raw = span_slice(content, span.start, span.end)?;

    let keyword = match instruction {
        ParsedInstruction::From(_) => "FROM".to_string(),
        ParsedInstruction::Arg(_) => "ARG".to_string(),
        ParsedInstruction::Label(_) => "LABEL".to_string(),
        ParsedInstruction::Run(_) => "RUN".to_string(),
        ParsedInstruction::Entrypoint(_) => "ENTRYPOINT".to_string(),
        ParsedInstruction::Cmd(_) => "CMD".to_string(),
        ParsedInstruction::Copy(_) => "COPY".to_string(),
        ParsedInstruction::Env(_) => "ENV".to_string(),
        ParsedInstruction::Misc(misc) => misc.instruction.content.to_ascii_uppercase(),
    };

    let start_line = line_index.line_for_offset(span.start);
    let end_line = if span.end > span.start {
        line_index.line_for_offset(span.end - 1)
    } else {
        start_line
    };

    Ok(Instruction {
        keyword,
        raw,
        start_line,
        end_line,
    })
}

fn span_slice(content: &str, start: usize, end: usize) -> Result<String, Error> {
    let bytes = content.as_bytes();
    if start > end || end > bytes.len() {
        return Err(Error::dockerfile_parse(format!(
            "invalid instruction span {start}..{end}"
        )));
    }

    let slice = &bytes[start..end];
    let text = std::str::from_utf8(slice)
        .map_err(|_| Error::dockerfile_parse(format!("invalid utf-8 in span {start}..{end}")))?;
    Ok(text.to_string())
}

struct LineIndex {
    newline_offsets: Vec<usize>,
}

impl LineIndex {
    fn new(content: &str) -> Self {
        let newline_offsets = content
            .as_bytes()
            .iter()
            .enumerate()
            .filter_map(|(index, byte)| (*byte == b'\n').then_some(index))
            .collect();
        Self { newline_offsets }
    }

    fn line_for_offset(&self, offset: usize) -> usize {
        self.newline_offsets
            .partition_point(|value| *value < offset)
            + 1
    }
}

#[cfg(test)]
mod tests {
    use crate::model::Parent;
    use crate::parser::parse_dockerfile;

    #[test]
    fn parses_global_arg_and_from() {
        let file = "ARG VERSION=1\nFROM alpine:${VERSION}\nUSER root\n";
        let parsed = parse_dockerfile(file).expect("dockerfile should parse");

        assert_eq!(
            parsed
                .global_args
                .get("VERSION")
                .expect("global arg should exist"),
            &Some("1".to_string())
        );
        assert_eq!(parsed.stages.len(), 1);
        assert_eq!(
            parsed.stages[0].parent,
            Parent::Image("alpine:${VERSION}".to_string())
        );
        assert_eq!(parsed.stages[0].instructions.len(), 1);
    }

    #[test]
    fn parses_stage_reference() {
        let file = "FROM alpine AS base\nFROM base\n";
        let parsed = parse_dockerfile(file).expect("dockerfile should parse");
        assert_eq!(parsed.stages.len(), 2);
        assert_eq!(
            parsed.stages[1].parent,
            Parent::StageRef("base".to_string())
        );
    }
}
