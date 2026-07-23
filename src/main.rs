mod error;
mod mif;
mod parser;

use clap::Parser;
use computils::instr::Instr;
use mif::Metadata;
use std::{collections::HashMap, fs, io};

use crate::{
    error::{AssemblerError, AssemblerResult},
    parser::{Assembly, parse_assembly},
};

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum RelocType {
    Abs16 = 0,
    Rel16 = 1, // relative 8-bit displacement (e.g. jmp/jlt/jgt/jeq)
    Data = 2,
}

#[derive(Debug, Clone)]
pub struct Reloc {
    pub offset: u16,      // byte offset into emitted code where operand bytes start
    pub sym_name: String, // symbolic name to resolve at link time
    pub kind: RelocType,
}

#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub val: u16,
}

type Symbols = Vec<Symbol>;
type Relocs = Vec<Reloc>;

#[derive(Debug)]
pub struct Object {
    pub instrs: Vec<Instr>,
    pub data: Vec<(String, String)>,
    pub symbols: Symbols,
    pub relocs: Relocs, // assembler-time relocations with byte offsets
    pub metadata: Option<Metadata>,
}

const OBJ_FILE_VERSION: u16 = 3;

impl Object {
    // Serialize object into binary:
    // [ "MOBJ" (4) ][version u16]
    // [instr_bytes_len u16][data_bytes_len u16]
    // [symtable_len u16][reloctable_len u16]
    // [meta_len u16]
    // [instr_bytes...][data_bytes...]
    // [symtable...][reloctable...][meta_bytes...]
    //
    // symtable: repeated (name bytes, 0u8, u16 addr)
    // reloctable: repeated (u16 offset, u16 sym_index, u8 kind)
    //
    // [mod_name_ofst u16][dependency_count u16]
    // [export_count u16][param_count u16]
    // [dependency_table][export_table]
    // [param_table][string_pool]
    //
    // dependency_table:
    //     repeated (name_ofst u16)
    //
    // export_table:
    //     repeated (
    //         name_ofst u16,
    //         return_type_ofst u16,
    //         first_param u16,
    //         param_count u16
    //     )
    //
    // param_table:
    //     repeated (
    //         name_ofst u16,
    //         type_ofst u16
    //     )
    //
    // string_pool:
    //     repeated (
    //         bytes,
    //         0u8
    //     )

    pub fn to_bin(&self) -> Vec<u8> {
        let mut out = Vec::new();

        // header: magic + version
        out.extend([b'M', b'O', b'B', b'J']);
        out.extend(OBJ_FILE_VERSION.to_le_bytes());

        // instructions bytes
        let instr_bytes = gen_bin(&self.instrs);
        out.extend((instr_bytes.len() as u16).to_le_bytes());

        // data bytes
        let mut data_bytes: Vec<u8> = Vec::new();
        for (_, value) in &self.data {
            data_bytes.extend(value.bytes());
        }
        out.extend((data_bytes.len() as u16).to_le_bytes());

        // symtable bytes (name\0 + u16 addr)
        let mut symtab_bytes = Vec::new();
        for Symbol { name, val } in &self.symbols {
            symtab_bytes.extend(name.bytes());
            symtab_bytes.push(0u8);
            symtab_bytes.extend(val.to_le_bytes());
        }
        out.extend((symtab_bytes.len() as u16).to_le_bytes());

        // reloctable bytes: need to convert sym_name -> sym_index
        let mut reloctab_bytes = Vec::new();
        for Reloc {
            offset,
            sym_name,
            kind,
        } in &self.relocs
        {
            let sym_index = self
                .symbols
                .iter()
                .position(|s| s.name == *sym_name)
                .expect("symbol referenced in reloc missing from symbol table")
                as u16;
            reloctab_bytes.extend(offset.to_le_bytes());
            reloctab_bytes.extend(sym_index.to_le_bytes());
            reloctab_bytes.push(*kind as u8);
        }
        out.extend((reloctab_bytes.len() as u16).to_le_bytes());

        // interface metadata
        let metadata_bytes = self
            .metadata
            .as_ref()
            .map(Metadata::to_bytes)
            .unwrap_or_default();
        out.extend((metadata_bytes.len() as u16).to_le_bytes());

        // payload
        out.extend(instr_bytes);
        out.extend(data_bytes);
        out.extend(symtab_bytes);
        out.extend(reloctab_bytes);
        out.extend(metadata_bytes);

        out
    }
}

pub fn resolve_conv_instrs(instrs: Vec<Instr>) -> Vec<Instr> {
    let mut out = Vec::new();
    for instr in instrs {
        out.extend(match instr {
            Instr::Ldr(rs, imm) => vec![
                Instr::Pushr(rs),
                Instr::Push(imm as i16 as u16),
                Instr::Add,
                Instr::Ldw,
            ],
            Instr::Str(rd, imm) => vec![
                Instr::Pushr(rd),
                Instr::Push(imm as i16 as u16),
                Instr::Add,
                Instr::Stw,
            ],
            Instr::Sub => vec![Instr::Not, Instr::Push(1), Instr::Add, Instr::Add],
            Instr::Mul => vec![
                Instr::Popr(2),
                Instr::Popr(1),
                Instr::CallLbl("__imul".to_string()),
                Instr::Pushr(0),
            ],
            Instr::Div => vec![
                Instr::Popr(3),
                Instr::Popr(2),
                Instr::CallLbl("__idivmod".to_string()),
                Instr::Pushr(0),
            ],
            Instr::Mod => vec![
                Instr::Popr(3),
                Instr::Popr(2),
                Instr::CallLbl("__idivmod".to_string()),
                Instr::Pushr(1),
            ],
            Instr::Neg => vec![Instr::Not, Instr::Push(1), Instr::Add],

            // immediate versions (no convenience instructions)
            Instr::AddI(imm) => vec![Instr::Push(imm), Instr::Add],
            Instr::SubI(imm) => vec![
                Instr::Push(imm),
                Instr::Not,
                Instr::Push(1),
                Instr::Add,
                Instr::Add,
            ],
            Instr::MulI(imm) => vec![
                Instr::Push(imm),
                Instr::Popr(2),
                Instr::Popr(1),
                Instr::CallLbl("__imul".to_string()),
                Instr::Pushr(0),
            ],
            Instr::DivI(imm) => vec![
                Instr::Push(imm),
                Instr::Popr(3),
                Instr::Popr(2),
                Instr::CallLbl("__idivmod".to_string()),
                Instr::Pushr(0),
            ],
            Instr::ModI(imm) => vec![
                Instr::Push(imm),
                Instr::Popr(3),
                Instr::Popr(2),
                Instr::CallLbl("__idivmod".to_string()),
                Instr::Pushr(1),
            ],
            Instr::NegI(imm) => vec![Instr::Push(imm), Instr::Not, Instr::Push(1), Instr::Add],
            Instr::CmpI(imm) => vec![Instr::Push(imm), Instr::Cmp],
            Instr::NotI(imm) => vec![Instr::Push(imm), Instr::Not],
            Instr::AndI(imm) => vec![Instr::Push(imm), Instr::And],
            Instr::OrI(imm) => vec![Instr::Push(imm), Instr::Or],
            Instr::XorI(imm) => vec![Instr::Push(imm), Instr::Xor],
            Instr::ShlI(imm) => vec![Instr::Push(imm), Instr::Shl],
            Instr::ShrI(imm) => vec![Instr::Push(imm), Instr::Shr],

            _ => vec![instr],
        })
    }
    out
}

pub fn gen_bin(instrs: &Vec<Instr>) -> Vec<u8> {
    let mut code = Vec::new();

    for instr in instrs {
        code.extend(match instr {
            Instr::Push(val) => vec![0x01, (val & 0xFF) as u8, (val >> 8) as u8],
            Instr::Halt => vec![0x0F],
            Instr::Ldw => vec![0x12],
            Instr::Stw => vec![0x13],
            Instr::Ldb => vec![0x16],
            Instr::Stb => vec![0x17],
            Instr::Call(addr) => vec![0x24, (*addr & 0xFF) as u8, (*addr >> 8) as u8],
            Instr::Ret => vec![0x25],
            Instr::Jmp(displ) => vec![0x26, (*displ & 0xFF) as u8, (*displ >> 8) as u8],
            Instr::Jlt(displ) => vec![0x27, (*displ & 0xFF) as u8, (*displ >> 8) as u8],
            Instr::Jgt(displ) => vec![0x28, (*displ & 0xFF) as u8, (*displ >> 8) as u8],
            Instr::Jeq(displ) => vec![0x29, (*displ & 0xFF) as u8, (*displ >> 8) as u8],
            Instr::Add => vec![0x30],
            Instr::Cmp => vec![0x35],
            Instr::Not => vec![0x40],
            Instr::And => vec![0x41],
            Instr::Or => vec![0x42],
            Instr::Xor => vec![0x43],
            Instr::Shl => vec![0x44],
            Instr::Shr => vec![0x45],
            Instr::Mov(rd, rs) => vec![0x50, (rd << 4) | rs],
            Instr::Pushr(rs) => vec![0x51, *rs],
            Instr::Popr(rd) => vec![0x52, *rd],
            Instr::Lbl(_)
            | Instr::CallLbl(_)
            | Instr::JmpLbl(_)
            | Instr::JltLbl(_)
            | Instr::JgtLbl(_)
            | Instr::JeqLbl(_)
            | Instr::Data(_) => {
                unreachable!("gen_bin called on unresolved symbolic instruction")
            }
            Instr::CmpI(_)
            | Instr::AddI(_)
            | Instr::SubI(_)
            | Instr::MulI(_)
            | Instr::DivI(_)
            | Instr::NegI(_)
            | Instr::ModI(_)
            | Instr::NotI(_)
            | Instr::AndI(_)
            | Instr::OrI(_)
            | Instr::XorI(_)
            | Instr::ShlI(_)
            | Instr::ShrI(_)
            | Instr::Ldr(_, _)
            | Instr::Str(_, _)
            | Instr::Sub
            | Instr::Mul
            | Instr::Div
            | Instr::Neg
            | Instr::Mod => {
                unreachable!(
                    "gen_bin called on unresolved convenience instructions {:?}",
                    &instr
                )
            }
            Instr::Int(int) => vec![0x70, *int],
            Instr::Iret => vec![0x71],
            Instr::Bkpt => vec![0x72],
        });
    }

    code
}

pub fn assemble_object(
    assembly: &Assembly,
    metadata: Option<Metadata>,
) -> AssemblerResult<Vec<u8>> {
    let mut out_instr = Vec::new();
    let mut byte_pos: u16 = 0;
    let mut symbols = HashMap::new();
    let mut relocs = Vec::new();

    for instr in &assembly.text {
        match instr {
            Instr::Lbl(name) => {
                let rec = symbols.insert(name.clone(), byte_pos);
                if rec.is_some() && rec.unwrap() != 0xFFFF {
                    return Err(AssemblerError {
                        msg: format!("symbol {} is defined multiple times", name),
                        line: None,
                    });
                }
            }

            Instr::JmpLbl(name)
            | Instr::JltLbl(name)
            | Instr::JgtLbl(name)
            | Instr::JeqLbl(name) => {
                let target_addr = match symbols.get(name) {
                    // Back reference
                    Some(addr) => {
                        (*addr as isize - (byte_pos as isize + instr.clone().byte_len() as isize))
                            as i16
                    }
                    // Forward reference
                    None => {
                        symbols.insert(name.to_string(), 0xFFFF);
                        relocs.push(Reloc {
                            offset: byte_pos + 1,
                            sym_name: name.to_string(),
                            kind: RelocType::Rel16,
                        });
                        -1 // 0xFFFF
                    }
                };

                let unlabelled = match instr {
                    Instr::JmpLbl(_) => Instr::Jmp(target_addr),
                    Instr::JltLbl(_) => Instr::Jlt(target_addr),
                    Instr::JgtLbl(_) => Instr::Jgt(target_addr),
                    Instr::JeqLbl(_) => Instr::Jeq(target_addr),
                    _ => unreachable!(),
                };

                out_instr.push(unlabelled);
            }

            Instr::CallLbl(name) => {
                let target_addr = match symbols.get(name) {
                    Some(addr) => addr,
                    None => {
                        symbols.insert(name.to_string(), 0xFFFF);
                        &0xFFFF
                    }
                };
                relocs.push(Reloc {
                    offset: byte_pos + 1,
                    sym_name: name.to_string(),
                    kind: RelocType::Abs16,
                });
                out_instr.push(Instr::Call(*target_addr));
            }

            Instr::Data(name) => {
                relocs.push(Reloc {
                    offset: byte_pos + 1,
                    sym_name: name.to_string(),
                    kind: RelocType::Data,
                });
                out_instr.push(Instr::Push(0xFFFF));
            }

            concrete => out_instr.push(concrete.clone()),
        }
        byte_pos += instr.byte_len() as u16;
    }

    for (name, value) in &assembly.data {
        let rec = symbols.insert(name.clone(), byte_pos);
        if rec.is_some() && rec.unwrap() != 0xFFFF {
            return Err(AssemblerError {
                msg: format!("symbol {} is defined multiple times", name),
                line: None,
            });
        }

        byte_pos += value.len() as u16
    }

    Ok(Object {
        instrs: out_instr,
        data: assembly.data.clone(),
        symbols: symbols
            .iter()
            .map(|(name, val)| Symbol {
                name: name.to_string(),
                val: *val,
            })
            .collect(),
        relocs,
        metadata,
    }
    .to_bin())
}

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Cli {
    /// input assembly file
    #[arg(value_name = "FILE", required_unless_present = "dump_meta")]
    input: Option<String>,

    /// output object file
    #[arg(short, long)]
    output: Option<String>,

    /// optional MIF interface file to embed
    #[arg(long, value_name = "FILE", group = "mode")]
    mif: Option<String>,

    /// dump metadata from an mobj file
    #[arg(long, value_name = "FILE", group = "mode")]
    dump_meta: Option<String>,
}

fn dump_metadata(file: &str, output: &Option<String>) -> io::Result<()> {
    let bytes = fs::read(file)?;

    if bytes.len() < 6 {
        eprintln!("error: file too small to be a valid mobj file");
        std::process::exit(1);
    }

    if &bytes[0..4] != b"MOBJ" {
        eprintln!("error: not a valid mobj file (bad magic)");
        std::process::exit(1);
    }

    let read_u16 = |offset: usize| -> u16 {
        u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
    };

    let version = read_u16(4);

    if version < OBJ_FILE_VERSION {
        eprintln!(
            "error: object file version {} does not contain metadata (requires version {})",
            version, OBJ_FILE_VERSION
        );
        std::process::exit(1);
    }

    if bytes.len() < 16 {
        eprintln!("error: file too small for a version {} header", version);
        std::process::exit(1);
    }

    let instr_bytes_len = read_u16(6) as usize;
    let data_bytes_len = read_u16(8) as usize;
    let symtable_len = read_u16(10) as usize;
    let reloctable_len = read_u16(12) as usize;
    let meta_len = read_u16(14) as usize;

    if meta_len == 0 {
        eprintln!("file has no metadata section");
        return Ok(());
    }

    let meta_start = 16 + instr_bytes_len + data_bytes_len + symtable_len + reloctable_len;
    let meta_end = meta_start + meta_len;

    if meta_end > bytes.len() {
        eprintln!("error: metadata section extends beyond file");
        std::process::exit(1);
    }

    let metadata = Metadata::from_bytes(&bytes[meta_start..meta_end]).unwrap_or_else(|e| {
        eprintln!("error parsing metadata: {}", e);
        std::process::exit(1);
    });

    match output {
        Some(path) => {
            fs::write(path, metadata.to_string())?;
            println!("wrote metadata to {}", path);
        }
        None => {
            print!("{}", metadata);
        }
    }

    Ok(())
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    if let Some(ref file) = cli.dump_meta {
        dump_metadata(file, &cli.output)?;
        return Ok(());
    }

    let input = cli.input.as_deref().unwrap_or_else(|| {
        eprintln!("error: input file required for assembly mode");
        std::process::exit(1);
    });

    let asm_code = fs::read_to_string(input)?;

    // parse assembly → instrs
    let assembly = parse_assembly(&asm_code).unwrap_or_else(|e| {
        eprintln!("Assembly parse error: {:?}", e);
        std::process::exit(1);
    });

    // assemble into object bytes
    let metadata = cli
        .mif
        .map(Metadata::from_mif)
        .transpose()
        .unwrap_or_else(|e| {
            eprintln!("MIF error: {:?}", e);
            std::process::exit(1);
        });
    let object_bytes = assemble_object(&assembly, metadata).unwrap_or_else(|e| {
        eprintln!("Assembly error: {:?}", e);
        std::process::exit(1);
    });

    // decide output filename
    let output = cli.output.unwrap_or_else(|| {
        let stem = std::path::Path::new(input)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        format!("{}.mobj", stem)
    });

    fs::write(&output, object_bytes)?;
    println!("wrote object file to {}", output);

    Ok(())
}
