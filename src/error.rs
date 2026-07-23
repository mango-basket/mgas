use std::{error::Error, fmt::Display};

#[derive(Debug)]
pub struct AssemblerError {
    pub msg: String,
    pub line: Option<usize>,
}

impl Display for AssemblerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.line {
            Some(l) => {
                write!(f, "Assembler Error at line {}: {}", l, self.msg)
            }
            None => {
                write!(f, "Assembler Error: {}", self.msg)
            }
        }
    }
}

impl Error for AssemblerError {}
pub type AssemblerResult<T> = Result<T, AssemblerError>;

impl AssemblerError {
    pub fn new_with_line(msg: String, line: usize) -> Self {
        Self {
            msg,
            line: Some(line),
        }
    }

    pub fn new(msg: String) -> Self {
        Self { msg, line: None }
    }
}
