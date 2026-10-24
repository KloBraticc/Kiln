pub mod error;
pub mod il;
pub mod metadata;
pub mod pe;
pub mod reader;

pub use error::{Error, Result};

use il::{MethodBody, VerifyStats};
use metadata::{Metadata, TableId};
use pe::PeImage;

#[derive(Debug, Default, Clone)]
pub struct AssemblyReport {
    pub name: Option<String>,
    pub metadata_version: String,
    pub ready_to_run: bool,
    pub types: u32,
    pub methods: u32,
    pub bodies: u32,
    pub native_bodies: u32,
    pub stats: VerifyStats,
    pub failures: Vec<String>,
}

pub fn inspect(data: &[u8]) -> Result<AssemblyReport> {
    let image = PeImage::parse(data)?;
    let metadata = Metadata::parse(image.metadata()?)?;
    let mut report = AssemblyReport {
        name: metadata.assembly_name()?,
        metadata_version: metadata.version.clone(),
        ready_to_run: image.cli.is_ready_to_run(),
        types: metadata.rows(TableId::TypeDef),
        methods: metadata.rows(TableId::MethodDef),
        ..Default::default()
    };
    for row in 1..=report.methods {
        let rva = metadata.cell(TableId::MethodDef, row, 0)?;
        if rva == 0 {
            continue;
        }
        if metadata.cell(TableId::MethodDef, row, 1)? & 0x3 != 0 {
            report.native_bodies += 1;
            continue;
        }
        report.bodies += 1;
        let outcome = image
            .tail_at_rva(rva)
            .and_then(|tail| MethodBody::parse(tail, rva))
            .and_then(|body| il::verify(&metadata, &body, rva));
        match outcome {
            Ok(stats) => {
                report.stats.instructions += stats.instructions;
                report.stats.branches += stats.branches;
                report.stats.tokens += stats.tokens;
            }
            Err(error) => {
                let name = metadata
                    .string_cell(TableId::MethodDef, row, 3)
                    .unwrap_or("?");
                report
                    .failures
                    .push(format!("{name} (MethodDef {row}): {error}"));
            }
        }
    }
    Ok(report)
}
