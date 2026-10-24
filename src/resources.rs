use crate::error::{Error, Result};
use crate::loader::Module;
use crate::metadata::TableId;
use crate::reader::Reader;

const STREAM: u32 = 0x21;
const BYTES: u32 = 0x20;

pub enum Value<'a> {
    Data(&'a [u8]),
    Text(String),
    Other(u32),
}

pub fn manifest(module: &Module) -> Result<Vec<(&'static str, &'static [u8])>> {
    let metadata = &module.metadata;
    let directory = module.image.cli.resources;
    let mut found = Vec::new();
    if !directory.is_present() {
        return Ok(found);
    }
    let section = module
        .image
        .slice_at_rva(directory.rva, directory.size as usize)?;
    for row in 1..=metadata.rows(TableId::ManifestResource) {
        if metadata.coded(TableId::ManifestResource, row, 3)?.is_some() {
            continue;
        }
        let offset = metadata.cell(TableId::ManifestResource, row, 0)? as usize;
        let mut reader = Reader::at(section, offset, "manifest resource")?;
        let length = reader.u32()? as usize;
        let data = reader.bytes(length)?;
        found.push((
            metadata.string_cell(TableId::ManifestResource, row, 2)?,
            data,
        ));
    }
    Ok(found)
}

pub fn seven_bit(reader: &mut Reader<'_>) -> Result<u32> {
    let mut value = 0u32;
    for shift in (0..35).step_by(7) {
        let byte = reader.u8()?;
        value |= u32::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(Error::BadMagic("seven bit encoded integer"))
}

pub fn entries(data: &[u8]) -> Result<Vec<(String, Value<'_>)>> {
    let mut reader = Reader::new(data, "resources");
    if reader.u32()? != 0xBEEF_CACE {
        return Err(Error::BadMagic("resources"));
    }
    reader.u32()?;
    let skip = reader.u32()? as usize;
    reader.skip(skip)?;
    if reader.u32()? != 2 {
        return Err(Error::BadMagic("resources version"));
    }
    let count = reader.u32()? as usize;
    let types = reader.u32()?;
    for _ in 0..types {
        let length = seven_bit(&mut reader)? as usize;
        reader.skip(length)?;
    }
    while !reader.position().is_multiple_of(8) {
        reader.u8()?;
    }
    reader.skip(4 * count)?;
    let mut positions = Vec::with_capacity(count);
    for _ in 0..count {
        positions.push(reader.u32()? as usize);
    }
    let data_section = reader.u32()? as usize;
    let names = reader.position();
    let mut found = Vec::with_capacity(count);
    for position in positions {
        let mut name_reader = Reader::at(data, names + position, "resource name")?;
        let length = seven_bit(&mut name_reader)? as usize;
        let units: Vec<u16> = name_reader
            .bytes(length)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        let offset = name_reader.u32()? as usize;
        let mut value_reader = Reader::at(data, data_section + offset, "resource value")?;
        let code = seven_bit(&mut value_reader)?;
        let value = if code == 1 {
            let length = seven_bit(&mut value_reader)? as usize;
            Value::Text(String::from_utf8_lossy(value_reader.bytes(length)?).into_owned())
        } else if code == STREAM || code == BYTES {
            let length = value_reader.u32()? as usize;
            Value::Data(value_reader.bytes(length)?)
        } else {
            Value::Other(code)
        };
        found.push((String::from_utf16_lossy(&units), value));
    }
    Ok(found)
}
