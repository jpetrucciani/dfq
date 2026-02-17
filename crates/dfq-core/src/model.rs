use std::collections::BTreeMap;

pub type ArgDefaults = BTreeMap<String, Option<String>>;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DockerfileModel {
    pub global_args: ArgDefaults,
    pub stages: Vec<Stage>,
    pub raw_instructions: Vec<Instruction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    pub index: usize,
    pub name: Option<String>,
    pub parent: Parent,
    pub arg_defaults: ArgDefaults,
    pub instructions: Vec<Instruction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parent {
    Image(String),
    Scratch,
    StageRef(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instruction {
    pub keyword: String,
    pub raw: String,
    pub start_line: usize,
    pub end_line: usize,
}
