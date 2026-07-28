use std::{collections::HashMap, fmt, fs};

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

    pub fn from_bytes(data: &[u8]) -> Self {
        let mut strings = Vec::new();
        let mut offsets = HashMap::new();
        let mut start = 0;

        for (i, &b) in data.iter().enumerate() {
            if b == 0 {
                let s = String::from_utf8_lossy(&data[start..i]).into_owned();
                offsets.insert(s.clone(), start as u16);
                strings.push(s);
                start = i + 1;
            }
        }

        Self {
            strings,
            offsets,
            next_offset: start as u16,
        }
    }

    pub fn resolve(&self, offset: u16) -> &str {
        for s in &self.strings {
            if let Some(&off) = self.offsets.get(s)
                && off == offset
            {
                return s;
            }
        }
        ""
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

        Self::from_mif_str(&contents)
    }

    pub fn from_mif_str(contents: &str) -> AssemblerResult<Self> {
        let mut lines = contents.lines();

        let first = lines.next().ok_or_else(|| {
            AssemblerError::new("could not read module name".into())
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

    pub fn from_bytes(data: &[u8]) -> AssemblerResult<Self> {
        if data.len() < 8 {
            return Err(AssemblerError::new("metadata section too small".into()));
        }

        let read_u16 = |data: &[u8], offset: usize| -> u16 {
            u16::from_le_bytes([data[offset], data[offset + 1]])
        };

        let name_ofst = read_u16(data, 0);
        let dependency_count = read_u16(data, 2);
        let export_count = read_u16(data, 4);
        let param_count = read_u16(data, 6);

        let mut pos = 8;

        let mut dependency_table = Vec::new();
        for _ in 0..dependency_count {
            dependency_table.push(read_u16(data, pos));
            pos += 2;
        }

        let mut export_table = Vec::new();
        for _ in 0..export_count {
            let e = Export {
                name_ofst: read_u16(data, pos),
                ret_ofst: read_u16(data, pos + 2),
                first_param_ofst: read_u16(data, pos + 4),
                param_count: read_u16(data, pos + 6),
            };
            export_table.push(e);
            pos += 8;
        }

        let mut param_table = Vec::new();
        for _ in 0..param_count {
            param_table.push(Param {
                name_ofst: read_u16(data, pos),
                type_ofst: read_u16(data, pos + 2),
            });
            pos += 4;
        }

        let string_pool = StringPool::from_bytes(&data[pos..]);

        Ok(Self {
            name_ofst,
            dependency_count,
            export_count,
            dependency_table,
            export_table,
            param_table,
            string_pool,
        })
    }

    pub fn from_string(s: &str) -> AssemblerResult<Self> {
        Self::from_bytes(s.as_bytes())
    }
}

impl fmt::Display for Metadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "module {}", self.string_pool.resolve(self.name_ofst))?;

        for &dep in &self.dependency_table {
            writeln!(f, "depends {}", self.string_pool.resolve(dep))?;
        }

        for export in &self.export_table {
            let name = self.string_pool.resolve(export.name_ofst);
            let ret = self.string_pool.resolve(export.ret_ofst);

            write!(f, "fn {}(", name)?;

            for i in 0..export.param_count {
                let idx = export.first_param_ofst as usize + i as usize;
                let param = &self.param_table[idx];
                let pname = self.string_pool.resolve(param.name_ofst);
                let ptype = self.string_pool.resolve(param.type_ofst);
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{}: {}", pname, ptype)?;
            }

            writeln!(f, ") -> {}", ret)?;
        }

        Ok(())
    }
}
