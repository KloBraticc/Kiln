pub mod baml;
pub mod codegen;
pub mod error;
pub mod il;
pub mod ir;
pub mod jit;
pub mod layout;
pub mod loader;
pub mod members;
pub mod metadata;
pub mod objects;
pub mod pe;
pub mod reachability;
pub mod reader;
pub mod resources;
pub mod runtime;
pub mod signature;

pub use error::{Error, Result};

use il::{MethodBody, VerifyStats};
use metadata::{Metadata, TableId};
use pe::PeImage;
use signature::Signature;

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
    pub signatures: u64,
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
    verify_signatures(&metadata, &mut report)?;
    Ok(report)
}

const SIGNATURE_COLUMNS: [(TableId, usize, bool); 7] = [
    (TableId::MethodDef, 4, false),
    (TableId::Field, 2, false),
    (TableId::Property, 2, false),
    (TableId::MemberRef, 2, false),
    (TableId::StandAloneSig, 0, false),
    (TableId::MethodSpec, 1, false),
    (TableId::TypeSpec, 0, true),
];

fn verify_signatures(metadata: &Metadata<'_>, report: &mut AssemblyReport) -> Result<()> {
    let mut tokens = Vec::new();
    for (table, column, is_type) in SIGNATURE_COLUMNS {
        for row in 1..=metadata.rows(table) {
            let blob = metadata.blob(metadata.cell(table, row, column)?)?;
            if table == TableId::StandAloneSig && blob.is_empty() {
                continue;
            }
            tokens.clear();
            let decoded = if is_type {
                Signature::parse_type(blob).map(|sig| sig.tokens(&mut tokens))
            } else {
                Signature::parse(blob).map(|sig| sig.tokens(&mut tokens))
            };
            report.signatures += 1;
            let failure = match decoded {
                Err(error) => Some(error.to_string()),
                Ok(()) => tokens
                    .iter()
                    .find(|&&token| !metadata.is_valid_token(token))
                    .map(|token| format!("references missing token {:#010x}", token.0)),
            };
            if let Some(failure) = failure {
                report
                    .failures
                    .push(format!("{} {row} signature: {failure}", table.name()));
            }
        }
    }
    Ok(())
}
