use clap::Parser;
use std::{fs, io};

use mgas::{Metadata, assemble_object, dump_metadata, parse_assembly};

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
