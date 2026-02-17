use std::collections::{BTreeMap, BTreeSet};

use crate::error::Error;
use crate::model::{DockerfileModel, Instruction, Parent, Stage};
use crate::query::{Arg, Index, Query, Segment};
use crate::value::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    Global,
    Stage(usize),
    StageWildcard,
}

impl Scope {
    pub fn as_str(&self) -> String {
        match self {
            Self::Global => "global".to_string(),
            Self::Stage(index) => format!("stage[{index}]"),
            Self::StageWildcard => "stage[*]".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalMeta {
    pub scope: Scope,
    pub missing_vars: BTreeSet<String>,
    pub used_vars: BTreeSet<String>,
    pub missing_paths: Vec<String>,
}

impl EvalMeta {
    pub fn new(scope: Scope) -> Self {
        Self {
            scope,
            missing_vars: BTreeSet::new(),
            used_vars: BTreeSet::new(),
            missing_paths: Vec::new(),
        }
    }

    pub fn to_value(&self, include_var_details: bool) -> Value {
        let mut map = BTreeMap::new();
        map.insert("scope".to_string(), Value::String(self.scope.as_str()));

        if include_var_details {
            map.insert(
                "missing_vars".to_string(),
                Value::Array(
                    self.missing_vars
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            );
            map.insert(
                "used_vars".to_string(),
                Value::Array(self.used_vars.iter().cloned().map(Value::String).collect()),
            );
        }

        if !self.missing_paths.is_empty() {
            map.insert(
                "missing_paths".to_string(),
                Value::Array(
                    self.missing_paths
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            );
        }

        Value::Object(map)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalResult {
    pub value: Value,
    pub meta: EvalMeta,
}

pub struct Evaluator<'a> {
    model: &'a DockerfileModel,
    overrides: &'a BTreeMap<String, String>,
    strict: bool,
}

impl<'a> Evaluator<'a> {
    pub fn new(
        model: &'a DockerfileModel,
        overrides: &'a BTreeMap<String, String>,
        strict: bool,
    ) -> Self {
        Self {
            model,
            overrides,
            strict,
        }
    }

    pub fn evaluate(&self, query: &Query) -> Result<EvalResult, Error> {
        if query.segments.is_empty() {
            return Err(Error::query_parse("empty query", None));
        }

        match &query.segments[0] {
            Segment::Ident(ident) if ident == "ARG" => {
                self.eval_arg(&query.segments[1..], &query.source)
            }
            Segment::Ident(ident) if ident == "FROM" => {
                self.eval_from_root(&query.segments[1..], &query.source)
            }
            Segment::Indexed { ident, index } if ident == "FROM" => {
                self.eval_from_index(index, &query.segments[1..], &query.source)
            }
            Segment::Ident(ident) if ident == "RUN" => {
                self.eval_run_root(&query.segments[1..], &query.source)
            }
            Segment::Indexed { ident, index } if ident == "RUN" => {
                self.eval_run_index(index, &query.segments[1..], &query.source)
            }
            Segment::Ident(ident) if ident == "STAGE" => {
                self.eval_stage_root(&query.segments[1..], &query.source)
            }
            Segment::Indexed { ident, index } if ident == "STAGE" => {
                self.eval_stage_index(index, &query.segments[1..], &query.source)
            }
            Segment::Function { ident, args } if ident == "RESOLVE" => {
                if query.segments.len() > 1 {
                    return Err(Error::not_found(query.source.clone()));
                }
                self.eval_resolve(args, ResolveScope::Global, Scope::Global)
            }
            _ => Err(Error::not_found(query.source.clone())),
        }
    }

    fn eval_arg(&self, rest: &[Segment], path: &str) -> Result<EvalResult, Error> {
        let mut meta = EvalMeta::new(Scope::Global);

        if rest.is_empty() {
            let mut out = BTreeMap::new();
            for (key, value) in &self.model.global_args {
                let mapped = value
                    .as_ref()
                    .map_or(Value::Null, |value| Value::String(value.clone()));
                out.insert(key.clone(), mapped);
            }
            return Ok(EvalResult {
                value: Value::Object(out),
                meta,
            });
        }

        let name = match &rest[0] {
            Segment::Ident(name) => name.clone(),
            _ => return Err(Error::not_found(path.to_string())),
        };

        if rest.len() == 1 {
            let mut stack = Vec::new();
            let value = self.resolve_var(ResolveScope::Global, &name, &mut meta, &mut stack)?;
            return value
                .map(|value| EvalResult {
                    value: Value::String(value),
                    meta,
                })
                .ok_or_else(|| Error::not_found(path.to_string()));
        }

        if rest.len() == 2
            && let Segment::Ident(field) = &rest[1]
        {
            return match field.as_str() {
                "DEFAULT" => {
                    let default = self
                        .model
                        .global_args
                        .get(&name)
                        .ok_or_else(|| Error::not_found(path.to_string()))?;
                    let value = default
                        .as_ref()
                        .map_or(Value::Null, |value| Value::String(value.clone()));
                    Ok(EvalResult { value, meta })
                }
                "SET" => {
                    let mut stack = Vec::new();
                    let set = self
                        .resolve_var(ResolveScope::Global, &name, &mut meta, &mut stack)?
                        .is_some();
                    Ok(EvalResult {
                        value: Value::Bool(set),
                        meta,
                    })
                }
                _ => Err(Error::not_found(path.to_string())),
            };
        }

        Err(Error::not_found(path.to_string()))
    }

    fn eval_from_root(&self, rest: &[Segment], path: &str) -> Result<EvalResult, Error> {
        if !rest.is_empty() {
            return Err(Error::not_found(path.to_string()));
        }

        let mut meta = EvalMeta::new(Scope::Global);
        let mut values = Vec::with_capacity(self.model.stages.len());
        for stage in &self.model.stages {
            values.push(Value::String(self.resolve_parent(stage, &mut meta)?));
        }

        Ok(EvalResult {
            value: Value::Array(values),
            meta,
        })
    }

    fn eval_from_index(
        &self,
        index: &Index,
        rest: &[Segment],
        path: &str,
    ) -> Result<EvalResult, Error> {
        match index {
            Index::Position(position) => {
                let Some(stage) = self.model.stages.get(*position) else {
                    return Err(Error::not_found(path.to_string()));
                };
                let mut meta = EvalMeta::new(Scope::Global);
                let value = self.eval_from_stage(*position, stage, rest, &mut meta, path)?;
                Ok(EvalResult { value, meta })
            }
            Index::Wildcard => {
                let mut meta = EvalMeta::new(Scope::Global);
                let mut values = Vec::with_capacity(self.model.stages.len());
                for (index, stage) in self.model.stages.iter().enumerate() {
                    values.push(self.eval_from_stage(index, stage, rest, &mut meta, path)?);
                }
                Ok(EvalResult {
                    value: Value::Array(values),
                    meta,
                })
            }
            Index::Key(_) => Err(Error::not_found(path.to_string())),
        }
    }

    fn eval_from_stage(
        &self,
        _stage_index: usize,
        stage: &Stage,
        rest: &[Segment],
        meta: &mut EvalMeta,
        path: &str,
    ) -> Result<Value, Error> {
        if rest.is_empty() {
            return self.resolve_parent(stage, meta).map(Value::String);
        }

        if rest.len() != 1 {
            return Err(Error::not_found(path.to_string()));
        }

        let Segment::Ident(field) = &rest[0] else {
            return Err(Error::not_found(path.to_string()));
        };
        match field.as_str() {
            "RAW" => Ok(Value::String(parent_raw(&stage.parent))),
            "RESOLVED" => self.resolve_parent(stage, meta).map(Value::String),
            "KIND" => Ok(Value::String(parent_kind(&stage.parent).to_string())),
            "STAGE" => match &stage.parent {
                Parent::StageRef(target) => Ok(Value::String(target.clone())),
                _ => Err(Error::not_found(path.to_string())),
            },
            _ => Err(Error::not_found(path.to_string())),
        }
    }

    fn eval_run_root(&self, rest: &[Segment], path: &str) -> Result<EvalResult, Error> {
        let mut meta = EvalMeta::new(Scope::Global);
        let entries = self.collect_run_entries();
        let value = self.eval_run_collection(&entries, rest, path, &mut meta)?;
        Ok(EvalResult { value, meta })
    }

    fn eval_run_index(
        &self,
        index: &Index,
        rest: &[Segment],
        path: &str,
    ) -> Result<EvalResult, Error> {
        let entries = self.collect_run_entries();

        match index {
            Index::Position(position) => {
                let Some(entry) = entries.get(*position) else {
                    return Err(Error::not_found(path.to_string()));
                };
                let meta = EvalMeta::new(Scope::Global);
                let value = self.eval_run_entry(entry, rest, path)?;
                Ok(EvalResult { value, meta })
            }
            Index::Wildcard => {
                let mut meta = EvalMeta::new(Scope::Global);
                let value = self.eval_run_collection(&entries, rest, path, &mut meta)?;
                Ok(EvalResult { value, meta })
            }
            Index::Key(_) => Err(Error::not_found(path.to_string())),
        }
    }

    fn eval_run_collection(
        &self,
        entries: &[RunEntry<'a>],
        rest: &[Segment],
        path: &str,
        meta: &mut EvalMeta,
    ) -> Result<Value, Error> {
        if rest.is_empty() {
            return Ok(Value::Array(
                entries
                    .iter()
                    .map(|entry| Value::String(entry.instruction.raw.clone()))
                    .collect(),
            ));
        }

        if let Segment::Ident(field) = &rest[0] {
            if field == "COUNT" && rest.len() == 1 {
                return Ok(Value::Number(entries.len() as i64));
            }
            if field == "RAW" && rest.len() == 1 {
                return Ok(Value::Array(
                    entries
                        .iter()
                        .map(|entry| Value::String(entry.instruction.raw.clone()))
                        .collect(),
                ));
            }
        }

        if let Segment::Function { ident, args } = &rest[0] {
            match ident.as_str() {
                "GREP" => {
                    let needle = function_single_string_arg(args, "GREP")?;
                    let filtered: Vec<RunEntry<'a>> = entries
                        .iter()
                        .filter(|entry| entry.instruction.raw.contains(needle))
                        .copied()
                        .collect();
                    return self.eval_run_collection(&filtered, &rest[1..], path, meta);
                }
                "CONTAINS" => {
                    if rest.len() != 1 {
                        return Err(Error::not_found(path.to_string()));
                    }
                    let needle = function_single_string_arg(args, "CONTAINS")?;
                    let contains = entries
                        .iter()
                        .any(|entry| entry.instruction.raw.contains(needle));
                    return Ok(Value::Bool(contains));
                }
                _ => return Err(Error::not_found(path.to_string())),
            }
        }

        if entries.is_empty() {
            return Ok(Value::Array(Vec::new()));
        }

        let probe_path = run_path(entries[0].run_index, rest);
        if matches!(
            self.eval_run_entry(&entries[0], rest, &probe_path),
            Err(Error::NotFound { .. })
        ) {
            return Err(Error::not_found(path.to_string()));
        }

        let mut values = Vec::with_capacity(entries.len());
        for entry in entries {
            let item_path = run_path(entry.run_index, rest);
            match self.eval_run_entry(entry, rest, &item_path) {
                Ok(value) => values.push(value),
                Err(Error::NotFound { .. }) => {
                    values.push(Value::Null);
                    meta.missing_paths.push(item_path);
                }
                Err(other) => return Err(other),
            }
        }
        Ok(Value::Array(values))
    }

    fn eval_run_entry(
        &self,
        entry: &RunEntry<'a>,
        rest: &[Segment],
        path: &str,
    ) -> Result<Value, Error> {
        if rest.is_empty() {
            return Ok(Value::String(entry.instruction.raw.clone()));
        }
        if rest.len() != 1 {
            return Err(Error::not_found(path.to_string()));
        }

        match &rest[0] {
            Segment::Ident(field) => match field.as_str() {
                "RAW" => Ok(Value::String(entry.instruction.raw.clone())),
                "COMMAND" => Ok(Value::String(run_command(&entry.instruction.raw))),
                "KEYWORD" => Ok(Value::String("RUN".to_string())),
                "INDEX" => Ok(Value::Number(entry.run_index as i64)),
                "STAGE" => Ok(Value::Number(entry.stage_index as i64)),
                "STAGE_NAME" => Ok(entry
                    .stage_name
                    .map_or(Value::Null, |name| Value::String(name.to_string()))),
                "SPAN" => {
                    let mut span = BTreeMap::new();
                    span.insert(
                        "start".to_string(),
                        Value::Number(entry.instruction.start_line as i64),
                    );
                    span.insert(
                        "end".to_string(),
                        Value::Number(entry.instruction.end_line as i64),
                    );
                    Ok(Value::Object(span))
                }
                _ => Err(Error::not_found(path.to_string())),
            },
            _ => Err(Error::not_found(path.to_string())),
        }
    }

    fn eval_stage_root(&self, rest: &[Segment], path: &str) -> Result<EvalResult, Error> {
        if !rest.is_empty() {
            return Err(Error::not_found(path.to_string()));
        }

        let mut meta = EvalMeta::new(Scope::Global);
        let mut values = Vec::with_capacity(self.model.stages.len());
        for (index, stage) in self.model.stages.iter().enumerate() {
            values.push(self.stage_object(index, stage, &mut meta)?);
        }

        Ok(EvalResult {
            value: Value::Array(values),
            meta,
        })
    }

    fn eval_stage_index(
        &self,
        index: &Index,
        rest: &[Segment],
        path: &str,
    ) -> Result<EvalResult, Error> {
        let selected = self.select_stage_indices(index, path)?;

        match index {
            Index::Wildcard => {
                let mut meta = EvalMeta::new(Scope::StageWildcard);
                let mut values = Vec::with_capacity(selected.len());

                for stage_index in selected {
                    let stage_path = stage_path(stage_index, rest);
                    match self.eval_stage_path(stage_index, rest, &stage_path, &mut meta) {
                        Ok(value) => values.push(value),
                        Err(Error::NotFound { .. }) => {
                            values.push(Value::Null);
                            meta.missing_paths.push(stage_path);
                        }
                        Err(other) => return Err(other),
                    }
                }

                Ok(EvalResult {
                    value: Value::Array(values),
                    meta,
                })
            }
            Index::Position(index) => {
                let mut meta = EvalMeta::new(Scope::Stage(*index));
                let value = self.eval_stage_path(*index, rest, path, &mut meta)?;
                Ok(EvalResult { value, meta })
            }
            Index::Key(_) => {
                let stage_index = selected[0];
                let mut meta = EvalMeta::new(Scope::Stage(stage_index));
                let value = self.eval_stage_path(stage_index, rest, path, &mut meta)?;
                Ok(EvalResult { value, meta })
            }
        }
    }

    fn eval_stage_path(
        &self,
        stage_index: usize,
        rest: &[Segment],
        path: &str,
        meta: &mut EvalMeta,
    ) -> Result<Value, Error> {
        let Some(stage) = self.model.stages.get(stage_index) else {
            return Err(Error::not_found(path.to_string()));
        };

        if rest.is_empty() {
            return self.stage_object(stage_index, stage, meta);
        }

        match &rest[0] {
            Segment::Ident(field) if field == "NAME" => {
                if rest.len() != 1 {
                    return Err(Error::not_found(path.to_string()));
                }
                Ok(stage.name.clone().map_or(Value::Null, Value::String))
            }
            Segment::Ident(field) if field == "ARG" => {
                self.eval_stage_arg(stage_index, stage, &rest[1..], path, meta)
            }
            Segment::Ident(field) if field == "PARENT" => {
                if rest.len() != 2 {
                    return Err(Error::not_found(path.to_string()));
                }
                let Segment::Ident(parent_field) = &rest[1] else {
                    return Err(Error::not_found(path.to_string()));
                };
                match parent_field.as_str() {
                    "RAW" => Ok(Value::String(parent_raw(&stage.parent))),
                    "RESOLVED" => self.resolve_parent(stage, meta).map(Value::String),
                    "KIND" => Ok(Value::String(parent_kind(&stage.parent).to_string())),
                    "STAGE" => match &stage.parent {
                        Parent::StageRef(target) => Ok(Value::String(target.clone())),
                        _ => Err(Error::not_found(path.to_string())),
                    },
                    _ => Err(Error::not_found(path.to_string())),
                }
            }
            Segment::Ident(field) if field == "INSTRUCTIONS" => {
                self.eval_stage_instructions(stage, &rest[1..], path)
            }
            Segment::Indexed { ident, index } if ident == "INSTRUCTIONS" => {
                self.eval_stage_instruction_index(stage, index, &rest[1..], path)
            }
            Segment::Function { ident, args } if ident == "RESOLVE" => {
                if rest.len() != 1 {
                    return Err(Error::not_found(path.to_string()));
                }
                if args.len() != 1 {
                    return Err(Error::eval("RESOLVE requires exactly one argument"));
                }
                let Arg::String(input) = &args[0] else {
                    return Err(Error::eval("RESOLVE argument must be a string literal"));
                };

                let mut stack = Vec::new();
                let resolved =
                    self.resolve_text(ResolveScope::Stage(stage_index), input, meta, &mut stack)?;
                Ok(Value::String(resolved))
            }
            _ => Err(Error::not_found(path.to_string())),
        }
    }

    fn eval_stage_arg(
        &self,
        stage_index: usize,
        stage: &Stage,
        rest: &[Segment],
        path: &str,
        meta: &mut EvalMeta,
    ) -> Result<Value, Error> {
        if rest.is_empty() {
            let mut out = BTreeMap::new();
            for (key, value) in &stage.arg_defaults {
                out.insert(
                    key.clone(),
                    value
                        .as_ref()
                        .map_or(Value::Null, |value| Value::String(value.clone())),
                );
            }
            return Ok(Value::Object(out));
        }

        let name = match &rest[0] {
            Segment::Ident(name) => name.clone(),
            _ => return Err(Error::not_found(path.to_string())),
        };

        if rest.len() == 1 {
            let mut stack = Vec::new();
            let value =
                self.resolve_var(ResolveScope::Stage(stage_index), &name, meta, &mut stack)?;
            return value
                .map(Value::String)
                .ok_or_else(|| Error::not_found(path.to_string()));
        }

        if rest.len() == 2
            && let Segment::Ident(field) = &rest[1]
        {
            return match field.as_str() {
                "DEFAULT" => {
                    let default = stage
                        .arg_defaults
                        .get(&name)
                        .ok_or_else(|| Error::not_found(path.to_string()))?;
                    Ok(default
                        .as_ref()
                        .map_or(Value::Null, |value| Value::String(value.clone())))
                }
                "SET" => {
                    let mut stack = Vec::new();
                    let set = self
                        .resolve_var(ResolveScope::Stage(stage_index), &name, meta, &mut stack)?
                        .is_some();
                    Ok(Value::Bool(set))
                }
                _ => Err(Error::not_found(path.to_string())),
            };
        }

        Err(Error::not_found(path.to_string()))
    }

    fn eval_stage_instructions(
        &self,
        stage: &Stage,
        rest: &[Segment],
        path: &str,
    ) -> Result<Value, Error> {
        if rest.len() != 1 {
            return Err(Error::not_found(path.to_string()));
        }

        let Segment::Ident(field) = &rest[0] else {
            return Err(Error::not_found(path.to_string()));
        };

        if field == "COUNT" {
            return Ok(Value::Number(stage.instructions.len() as i64));
        }

        Err(Error::not_found(path.to_string()))
    }

    fn eval_stage_instruction_index(
        &self,
        stage: &Stage,
        index: &Index,
        rest: &[Segment],
        path: &str,
    ) -> Result<Value, Error> {
        let instruction_index = match index {
            Index::Position(index) => *index,
            _ => return Err(Error::not_found(path.to_string())),
        };
        let Some(instruction) = stage.instructions.get(instruction_index) else {
            return Err(Error::not_found(path.to_string()));
        };

        if rest.len() != 1 {
            return Err(Error::not_found(path.to_string()));
        }

        let Segment::Ident(field) = &rest[0] else {
            return Err(Error::not_found(path.to_string()));
        };
        match field.as_str() {
            "RAW" => Ok(Value::String(instruction.raw.clone())),
            "KEYWORD" => Ok(Value::String(instruction.keyword.clone())),
            "SPAN" => {
                let mut span = BTreeMap::new();
                span.insert(
                    "start".to_string(),
                    Value::Number(instruction.start_line as i64),
                );
                span.insert(
                    "end".to_string(),
                    Value::Number(instruction.end_line as i64),
                );
                Ok(Value::Object(span))
            }
            _ => Err(Error::not_found(path.to_string())),
        }
    }

    fn eval_resolve(
        &self,
        args: &[Arg],
        scope: ResolveScope,
        meta_scope: Scope,
    ) -> Result<EvalResult, Error> {
        if args.len() != 1 {
            return Err(Error::eval("RESOLVE requires exactly one argument"));
        }
        let Arg::String(input) = &args[0] else {
            return Err(Error::eval("RESOLVE argument must be a string literal"));
        };

        let mut meta = EvalMeta::new(meta_scope);
        let mut stack = Vec::new();
        let resolved = self.resolve_text(scope, input, &mut meta, &mut stack)?;
        Ok(EvalResult {
            value: Value::String(resolved),
            meta,
        })
    }

    fn stage_object(
        &self,
        index: usize,
        stage: &Stage,
        meta: &mut EvalMeta,
    ) -> Result<Value, Error> {
        let mut stage_map = BTreeMap::new();
        stage_map.insert("index".to_string(), Value::Number(index as i64));
        stage_map.insert(
            "name".to_string(),
            stage.name.clone().map_or(Value::Null, Value::String),
        );

        let mut parent_map = BTreeMap::new();
        parent_map.insert("raw".to_string(), Value::String(parent_raw(&stage.parent)));
        parent_map.insert(
            "resolved".to_string(),
            Value::String(self.resolve_parent(stage, meta)?),
        );
        parent_map.insert(
            "kind".to_string(),
            Value::String(parent_kind(&stage.parent).to_string()),
        );
        if let Parent::StageRef(target) = &stage.parent {
            parent_map.insert("stage".to_string(), Value::String(target.clone()));
        }
        stage_map.insert("parent".to_string(), Value::Object(parent_map));

        let mut args_map = BTreeMap::new();
        for (key, value) in &stage.arg_defaults {
            args_map.insert(
                key.clone(),
                value
                    .as_ref()
                    .map_or(Value::Null, |value| Value::String(value.clone())),
            );
        }
        stage_map.insert("arg".to_string(), Value::Object(args_map));

        let mut instruction_meta = BTreeMap::new();
        instruction_meta.insert(
            "count".to_string(),
            Value::Number(stage.instructions.len() as i64),
        );
        stage_map.insert("instructions".to_string(), Value::Object(instruction_meta));

        Ok(Value::Object(stage_map))
    }

    fn resolve_parent(&self, stage: &Stage, meta: &mut EvalMeta) -> Result<String, Error> {
        match &stage.parent {
            Parent::Image(raw) => {
                let mut stack = Vec::new();
                self.resolve_text(ResolveScope::Global, raw, meta, &mut stack)
            }
            Parent::Scratch => Ok("scratch".to_string()),
            Parent::StageRef(target) => Ok(target.clone()),
        }
    }

    fn resolve_text(
        &self,
        scope: ResolveScope,
        input: &str,
        meta: &mut EvalMeta,
        stack: &mut Vec<ResolveKey>,
    ) -> Result<String, Error> {
        let bytes = input.as_bytes();
        let mut pos = 0;
        let mut out = String::with_capacity(input.len());

        while pos < bytes.len() {
            if bytes[pos] != b'$' {
                out.push(bytes[pos] as char);
                pos += 1;
                continue;
            }

            if pos + 1 >= bytes.len() {
                out.push('$');
                pos += 1;
                continue;
            }

            if bytes[pos + 1] == b'{' {
                let mut cursor = pos + 2;
                while cursor < bytes.len() && bytes[cursor] != b'}' {
                    cursor += 1;
                }
                if cursor >= bytes.len() {
                    return Err(Error::eval("unterminated ${...} interpolation"));
                }
                let name = &input[pos + 2..cursor];
                if !is_valid_var_name(name) {
                    return Err(Error::eval(format!(
                        "unsupported interpolation form '${{{name}}}'"
                    )));
                }
                meta.used_vars.insert(name.to_string());
                let value = self.resolve_var(scope, name, meta, stack)?;
                if let Some(value) = value {
                    out.push_str(&value);
                } else if self.strict {
                    return Err(Error::eval(format!("missing variable '{name}'")));
                } else {
                    meta.missing_vars.insert(name.to_string());
                }
                pos = cursor + 1;
                continue;
            }

            let name_start = pos + 1;
            if !is_var_start(bytes[name_start]) {
                out.push('$');
                pos += 1;
                continue;
            }

            let mut cursor = name_start + 1;
            while cursor < bytes.len() && is_var_continue(bytes[cursor]) {
                cursor += 1;
            }
            let name = &input[name_start..cursor];
            meta.used_vars.insert(name.to_string());

            let value = self.resolve_var(scope, name, meta, stack)?;
            if let Some(value) = value {
                out.push_str(&value);
            } else if self.strict {
                return Err(Error::eval(format!("missing variable '{name}'")));
            } else {
                meta.missing_vars.insert(name.to_string());
            }

            pos = cursor;
        }

        Ok(out)
    }

    fn resolve_var(
        &self,
        scope: ResolveScope,
        name: &str,
        meta: &mut EvalMeta,
        stack: &mut Vec<ResolveKey>,
    ) -> Result<Option<String>, Error> {
        if let Some(value) = self.overrides.get(name) {
            return Ok(Some(value.clone()));
        }

        if let ResolveScope::Stage(stage_index) = scope
            && let Some(stage) = self.model.stages.get(stage_index)
            && let Some(Some(default)) = stage.arg_defaults.get(name)
        {
            let key = ResolveKey::new(scope, name);
            if stack.contains(&key) {
                return Err(Error::eval(format!(
                    "cycle detected while resolving '{name}'"
                )));
            }
            stack.push(key);
            let resolved = self.resolve_text(scope, default, meta, stack)?;
            stack.pop();
            return Ok(Some(resolved));
        }

        if let Some(Some(default)) = self.model.global_args.get(name) {
            let key = ResolveKey::new(ResolveScope::Global, name);
            if stack.contains(&key) {
                return Err(Error::eval(format!(
                    "cycle detected while resolving '{name}'"
                )));
            }
            stack.push(key);
            let resolved = self.resolve_text(scope, default, meta, stack)?;
            stack.pop();
            return Ok(Some(resolved));
        }

        Ok(None)
    }

    fn collect_run_entries(&self) -> Vec<RunEntry<'a>> {
        let mut entries = Vec::new();
        for (stage_index, stage) in self.model.stages.iter().enumerate() {
            for instruction in &stage.instructions {
                if instruction.keyword == "RUN" {
                    let run_index = entries.len();
                    entries.push(RunEntry {
                        run_index,
                        stage_index,
                        stage_name: stage.name.as_deref(),
                        instruction,
                    });
                }
            }
        }
        entries
    }

    fn select_stage_indices(&self, selector: &Index, path: &str) -> Result<Vec<usize>, Error> {
        match selector {
            Index::Position(index) => {
                if *index >= self.model.stages.len() {
                    return Err(Error::not_found(path.to_string()));
                }
                Ok(vec![*index])
            }
            Index::Wildcard => Ok((0..self.model.stages.len()).collect()),
            Index::Key(name) => {
                let matches: Vec<usize> = self
                    .model
                    .stages
                    .iter()
                    .enumerate()
                    .filter_map(|(index, stage)| {
                        stage
                            .name
                            .as_ref()
                            .filter(|stage_name| *stage_name == name)
                            .map(|_| index)
                    })
                    .collect();

                if matches.is_empty() {
                    return Err(Error::not_found(path.to_string()));
                }
                if matches.len() > 1 {
                    return Err(Error::eval(format!(
                        "stage selector \"{name}\" is ambiguous ({} matches)",
                        matches.len()
                    )));
                }
                Ok(matches)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ResolveScope {
    Global,
    Stage(usize),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ResolveKey {
    scope: ResolveScope,
    name: String,
}

impl ResolveKey {
    fn new(scope: ResolveScope, name: &str) -> Self {
        Self {
            scope,
            name: name.to_string(),
        }
    }
}

#[derive(Clone, Copy)]
struct RunEntry<'a> {
    run_index: usize,
    stage_index: usize,
    stage_name: Option<&'a str>,
    instruction: &'a Instruction,
}

fn is_var_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_var_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn is_valid_var_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !is_var_start(first) {
        return false;
    }
    bytes.all(is_var_continue)
}

fn parent_raw(parent: &Parent) -> String {
    match parent {
        Parent::Image(raw) => raw.clone(),
        Parent::Scratch => "scratch".to_string(),
        Parent::StageRef(target) => target.clone(),
    }
}

fn parent_kind(parent: &Parent) -> &'static str {
    match parent {
        Parent::Image(_) => "image",
        Parent::Scratch => "scratch",
        Parent::StageRef(_) => "stage",
    }
}

fn function_single_string_arg<'a>(args: &'a [Arg], name: &str) -> Result<&'a str, Error> {
    match args {
        [Arg::String(value)] => Ok(value.as_str()),
        _ => Err(Error::eval(format!(
            "{name} requires exactly one string argument"
        ))),
    }
}

fn stage_path(stage_index: usize, rest: &[Segment]) -> String {
    let mut out = format!("STAGE[{stage_index}]");
    for segment in rest {
        out.push('.');
        out.push_str(&segment_to_string(segment));
    }
    out
}

fn run_path(run_index: usize, rest: &[Segment]) -> String {
    let mut out = format!("RUN[{run_index}]");
    for segment in rest {
        out.push('.');
        out.push_str(&segment_to_string(segment));
    }
    out
}

fn run_command(raw: &str) -> String {
    let trimmed = raw.trim_start();
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 4
        && bytes[0].eq_ignore_ascii_case(&b'r')
        && bytes[1].eq_ignore_ascii_case(&b'u')
        && bytes[2].eq_ignore_ascii_case(&b'n')
        && bytes[3].is_ascii_whitespace()
    {
        return trimmed[3..].trim_start().to_string();
    }
    trimmed.to_string()
}

fn segment_to_string(segment: &Segment) -> String {
    match segment {
        Segment::Ident(ident) => ident.clone(),
        Segment::Indexed { ident, index } => match index {
            Index::Position(index) => format!("{ident}[{index}]"),
            Index::Wildcard => format!("{ident}[*]"),
            Index::Key(name) => format!("{ident}[\"{name}\"]"),
        },
        Segment::Function { ident, args } => {
            let rendered_args: Vec<String> = args
                .iter()
                .map(|arg| match arg {
                    Arg::String(value) => format!("\"{value}\""),
                    Arg::Ident(value) => value.clone(),
                    Arg::Number(value) => value.to_string(),
                })
                .collect();
            format!("{ident}({})", rendered_args.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::eval::Evaluator;
    use crate::parser::parse_dockerfile;
    use crate::query::parse_query;

    #[test]
    fn resolves_from_with_override() {
        let dockerfile = "ARG VERSION=1.0.0\nFROM alpine:${VERSION}\n";
        let model = parse_dockerfile(dockerfile).expect("dockerfile should parse");
        let mut overrides = BTreeMap::new();
        overrides.insert("VERSION".to_string(), "2.0.0".to_string());
        let evaluator = Evaluator::new(&model, &overrides, false);

        let query = parse_query("FROM[0].RESOLVED").expect("query should parse");
        let result = evaluator.evaluate(&query).expect("query should evaluate");
        assert_eq!(
            result.value.render_scalar(),
            Some("alpine:2.0.0".to_string())
        );
    }

    #[test]
    fn stage_wildcard_returns_null_for_missing() {
        let dockerfile = "FROM alpine AS a\nFROM alpine AS b\nARG ONLY_B=1\n";
        let model = parse_dockerfile(dockerfile).expect("dockerfile should parse");
        let overrides = BTreeMap::new();
        let evaluator = Evaluator::new(&model, &overrides, false);

        let query = parse_query("STAGE[*].ARG.DOES_NOT_EXIST").expect("query should parse");
        let result = evaluator.evaluate(&query).expect("query should evaluate");
        let json = result.value.to_json_string();
        assert_eq!(json, "[null,null]");
        assert_eq!(result.meta.missing_paths.len(), 2);
    }

    #[test]
    fn run_queries_work_for_wildcard_and_index() {
        let dockerfile = "FROM alpine AS base\nRUN echo one\nRUN echo two\n";
        let model = parse_dockerfile(dockerfile).expect("dockerfile should parse");
        let overrides = BTreeMap::new();
        let evaluator = Evaluator::new(&model, &overrides, false);

        let wildcard = parse_query("RUN[*]").expect("query should parse");
        let wildcard_result = evaluator
            .evaluate(&wildcard)
            .expect("query should evaluate");
        assert_eq!(
            wildcard_result.value.to_json_string(),
            "[\"RUN echo one\",\"RUN echo two\"]"
        );

        let indexed = parse_query("RUN[1].COMMAND").expect("query should parse");
        let indexed_result = evaluator.evaluate(&indexed).expect("query should evaluate");
        assert_eq!(
            indexed_result.value.render_scalar(),
            Some("echo two".to_string())
        );
    }

    #[test]
    fn run_directives_filter_and_count() {
        let dockerfile = "FROM alpine\nRUN apk add curl\nRUN echo done\n";
        let model = parse_dockerfile(dockerfile).expect("dockerfile should parse");
        let overrides = BTreeMap::new();
        let evaluator = Evaluator::new(&model, &overrides, false);

        let grep = parse_query("RUN.GREP(\"apk\")").expect("query should parse");
        let grep_result = evaluator.evaluate(&grep).expect("query should evaluate");
        assert_eq!(grep_result.value.to_json_string(), "[\"RUN apk add curl\"]");

        let grep_count = parse_query("RUN.GREP(\"apk\").COUNT").expect("query should parse");
        let grep_count_result = evaluator
            .evaluate(&grep_count)
            .expect("query should evaluate");
        assert_eq!(
            grep_count_result.value.render_scalar(),
            Some("1".to_string())
        );

        let contains = parse_query("RUN.CONTAINS(\"echo\")").expect("query should parse");
        let contains_result = evaluator
            .evaluate(&contains)
            .expect("query should evaluate");
        assert_eq!(
            contains_result.value.render_scalar(),
            Some("true".to_string())
        );

        let wildcard_grep = parse_query("RUN[*].GREP(\"apk\").COUNT").expect("query should parse");
        let wildcard_grep_result = evaluator
            .evaluate(&wildcard_grep)
            .expect("query should evaluate");
        assert_eq!(
            wildcard_grep_result.value.render_scalar(),
            Some("1".to_string())
        );
    }
}
