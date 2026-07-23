use std::{collections::HashMap, fs};

use crate::error::{AssemblerError, AssemblerResult};

#[derive(Debug, Default)]
pub struct StringPool {
    strings: Vec<String>,
    offsets: HashMap<String, u16>,
    next_offset: u16,
}

impl StringPool {
    pub fn new() -> Self {
        Self::default()
    }

    /// Interns a string and returns its byte offset in the serialized pool.
    pub fn intern<S: AsRef<str>>(&mut self, s: S) -> u16 {
        let s = s.as_ref();

        if let Some(&offset) = self.offsets.get(s) {
            return offset;
        }

        let offset = self.next_offset;

        self.strings.push(s.to_owned());
        self.offsets.insert(s.to_owned(), offset);

        self.next_offset += s.len() as u16 + 1; // account for '\0'

        offset
    }

    /// Returns the serialized string pool.
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.next_offset as usize);

        for s in &self.strings {
            out.extend_from_slice(s.as_bytes());
            out.push(0);
        }

        out
    }

    /// Returns the offset if the string has already been interned.
    pub fn get<S: AsRef<str>>(&self, s: S) -> Option<u16> {
        self.offsets.get(s.as_ref()).copied()
    }

    pub fn len(&self) -> usize {
        self.strings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }
}

#[derive(Debug)]
pub struct Export {
    pub name_ofst: u16,
    pub first_param_ofst: u16,
    pub param_count: u16,
    pub ret_ofst: u16,
}

#[derive(Debug)]
pub struct Param {
    pub name_ofst: u16,
    pub type_ofst: u16,
}

#[derive(Debug)]
pub struct Metadata {
    pub name_ofst: u16,
    pub dependency_count: u16,
    pub export_count: u16,
    pub dependency_table: Vec<u16>,
    pub export_table: Vec<Export>,
    pub param_table: Vec<Param>,
    pub string_pool: StringPool,
}

impl Metadata {
    pub fn from_mif(filename: String) -> AssemblerResult<Self> {
        let contents = fs::read_to_string(&filename)
            .map_err(|_| AssemblerError::new(format!("could not open {}", filename)))?;

        let mut lines = contents.lines();

        let first = lines.next().ok_or_else(|| {
            AssemblerError::new(format!("could not read module name in {}", filename))
        })?;

        let module_name = first
            .strip_prefix("module ")
            .ok_or_else(|| AssemblerError::new("expected `module <name>`".into()))?;

        let mut pool = StringPool::new();

        let name_ofst = pool.intern(module_name);

        let mut dependency_table = Vec::new();
        let mut export_table = Vec::new();
        let mut param_table = Vec::new();

        for (linenum, line) in lines.enumerate() {
            let line = line.trim();

            if line.is_empty() {
                continue;
            }

            if let Some(dep) = line.strip_prefix("depends ") {
                dependency_table.push(pool.intern(dep.trim()));
                continue;
            }

            if let Some(sig) = line.strip_prefix("fn ") {
                let (head, ret) = sig
                    .split_once("->")
                    .ok_or_else(|| AssemblerError::new(format!("invalid function '{}'", line)))?;

                let ret_ofst = pool.intern(ret.trim());

                let open = head
                    .find('(')
                    .ok_or_else(|| AssemblerError::new("expected '('".into()))?;

                let close = head
                    .rfind(')')
                    .ok_or_else(|| AssemblerError::new("expected ')'".into()))?;

                let name = head[..open].trim();
                let params = &head[open + 1..close];

                let first_param = param_table.len() as u16;

                let mut param_count = 0;

                if !params.trim().is_empty() {
                    for param in params.split(',') {
                        let (pname, ptype) = param.trim().split_once(':').ok_or_else(|| {
                            AssemblerError::new(format!("invalid parameter '{}'", param))
                        })?;

                        param_table.push(Param {
                            name_ofst: pool.intern(pname.trim()),
                            type_ofst: pool.intern(ptype.trim()),
                        });

                        param_count += 1;
                    }
                }

                export_table.push(Export {
                    name_ofst: pool.intern(name),
                    first_param_ofst: first_param,
                    param_count,
                    ret_ofst,
                });

                continue;
            }

            return Err(AssemblerError::new_with_line(
                "unexpected line".into(),
                linenum,
            ));
        }

        Ok(Self {
            name_ofst,
            dependency_count: dependency_table.len() as u16,
            export_count: export_table.len() as u16,
            dependency_table,
            export_table,
            param_table,
            string_pool: pool,
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();

        out.extend(self.name_ofst.to_le_bytes());

        out.extend(self.dependency_count.to_le_bytes());

        out.extend(self.export_count.to_le_bytes());

        out.extend((self.param_table.len() as u16).to_le_bytes());

        // dependencies

        for dep in &self.dependency_table {
            out.extend(dep.to_le_bytes());
        }

        // exports

        for export in &self.export_table {
            out.extend(export.name_ofst.to_le_bytes());

            out.extend(export.ret_ofst.to_le_bytes());

            out.extend(export.first_param_ofst.to_le_bytes());

            out.extend(export.param_count.to_le_bytes());
        }

        // params

        for param in &self.param_table {
            out.extend(param.name_ofst.to_le_bytes());

            out.extend(param.type_ofst.to_le_bytes());
        }

        // string pool

        out.extend(self.string_pool.serialize());

        out
    }
}
