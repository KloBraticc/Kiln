use crate::error::{Error, Result};
use crate::reader::Reader;

const CLI_HEADER_DIRECTORY: usize = 14;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Machine {
    I386,
    Amd64,
    Arm64,
    Other(u16),
}

impl Machine {
    fn from_raw(raw: u16) -> Self {
        match raw {
            0x014C => Machine::I386,
            0x8664 => Machine::Amd64,
            0xAA64 => Machine::Arm64,
            other => Machine::Other(other),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DataDirectory {
    pub rva: u32,
    pub size: u32,
}

impl DataDirectory {
    fn read(reader: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            rva: reader.u32()?,
            size: reader.u32()?,
        })
    }

    pub fn is_present(&self) -> bool {
        self.rva != 0 && self.size != 0
    }
}

#[derive(Debug, Clone)]
pub struct Section {
    pub name: String,
    pub virtual_address: u32,
    pub virtual_size: u32,
    pub raw_offset: u32,
    pub raw_size: u32,
}

impl Section {
    fn contains(&self, rva: u32) -> bool {
        let extent = self.virtual_size.max(self.raw_size);
        rva >= self.virtual_address && rva - self.virtual_address < extent
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CliHeader {
    pub runtime_major: u16,
    pub runtime_minor: u16,
    pub metadata: DataDirectory,
    pub flags: u32,
    pub entry_point: u32,
    pub resources: DataDirectory,
    pub strong_name: DataDirectory,
    pub vtable_fixups: DataDirectory,
    pub managed_native_header: DataDirectory,
}

impl CliHeader {
    pub const IL_ONLY: u32 = 0x1;
    pub const REQUIRES_32BIT: u32 = 0x2;
    pub const STRONG_NAME_SIGNED: u32 = 0x8;
    pub const NATIVE_ENTRY_POINT: u32 = 0x10;

    pub fn is_ready_to_run(&self) -> bool {
        self.managed_native_header.is_present()
    }
}

pub struct PeImage<'a> {
    data: &'a [u8],
    pub machine: Machine,
    pub is_pe32_plus: bool,
    pub sections: Vec<Section>,
    pub cli: CliHeader,
}

impl<'a> PeImage<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let mut reader = Reader::new(data, "PE image");
        if reader.u16()? != 0x5A4D {
            return Err(Error::BadMagic("DOS header"));
        }
        reader.seek(0x3C)?;
        let pe_offset = reader.u32()? as usize;
        reader.seek(pe_offset)?;
        if reader.u32()? != 0x0000_4550 {
            return Err(Error::BadMagic("PE header"));
        }

        let machine = Machine::from_raw(reader.u16()?);
        let section_count = reader.u16()? as usize;
        reader.skip(12)?;
        let optional_size = reader.u16()? as usize;
        reader.skip(2)?;

        let optional_start = reader.position();
        let is_pe32_plus = match reader.u16()? {
            0x10B => false,
            0x20B => true,
            _ => return Err(Error::BadMagic("optional header")),
        };
        reader.seek(optional_start + if is_pe32_plus { 108 } else { 92 })?;
        let directory_count = reader.u32()? as usize;
        let mut directories = Vec::with_capacity(directory_count.min(16));
        for _ in 0..directory_count.min(16) {
            directories.push(DataDirectory::read(&mut reader)?);
        }

        reader.seek(optional_start + optional_size)?;
        let mut sections = Vec::with_capacity(section_count);
        for _ in 0..section_count {
            let raw_name = reader.bytes(8)?;
            let name_length = raw_name.iter().position(|&b| b == 0).unwrap_or(8);
            let virtual_size = reader.u32()?;
            let virtual_address = reader.u32()?;
            let raw_size = reader.u32()?;
            let raw_offset = reader.u32()?;
            reader.skip(16)?;
            sections.push(Section {
                name: String::from_utf8_lossy(&raw_name[..name_length]).into_owned(),
                virtual_address,
                virtual_size,
                raw_offset,
                raw_size,
            });
        }

        let cli_directory = directories
            .get(CLI_HEADER_DIRECTORY)
            .copied()
            .filter(DataDirectory::is_present)
            .ok_or(Error::NotManaged)?;
        let mut image = Self {
            data,
            machine,
            is_pe32_plus,
            sections,
            cli: CliHeader {
                runtime_major: 0,
                runtime_minor: 0,
                metadata: DataDirectory { rva: 0, size: 0 },
                flags: 0,
                entry_point: 0,
                resources: DataDirectory { rva: 0, size: 0 },
                strong_name: DataDirectory { rva: 0, size: 0 },
                vtable_fixups: DataDirectory { rva: 0, size: 0 },
                managed_native_header: DataDirectory { rva: 0, size: 0 },
            },
        };

        let mut cli = Reader::at(data, image.rva_to_offset(cli_directory.rva)?, "CLI header")?;
        cli.skip(4)?;
        let runtime_major = cli.u16()?;
        let runtime_minor = cli.u16()?;
        let metadata = DataDirectory::read(&mut cli)?;
        let flags = cli.u32()?;
        let entry_point = cli.u32()?;
        let resources = DataDirectory::read(&mut cli)?;
        let strong_name = DataDirectory::read(&mut cli)?;
        cli.skip(8)?;
        let vtable_fixups = DataDirectory::read(&mut cli)?;
        cli.skip(8)?;
        let managed_native_header = DataDirectory::read(&mut cli)?;
        image.cli = CliHeader {
            runtime_major,
            runtime_minor,
            metadata,
            flags,
            entry_point,
            resources,
            strong_name,
            vtable_fixups,
            managed_native_header,
        };
        Ok(image)
    }

    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    pub fn rva_to_offset(&self, rva: u32) -> Result<usize> {
        let section = self
            .sections
            .iter()
            .find(|s| s.contains(rva))
            .ok_or(Error::BadRva(rva))?;
        let offset = (rva - section.virtual_address) as usize + section.raw_offset as usize;
        if offset > self.data.len() {
            return Err(Error::BadRva(rva));
        }
        Ok(offset)
    }

    pub fn slice_at_rva(&self, rva: u32, size: usize) -> Result<&'a [u8]> {
        let start = self.rva_to_offset(rva)?;
        self.data.get(start..start + size).ok_or(Error::BadRva(rva))
    }

    pub fn tail_at_rva(&self, rva: u32) -> Result<&'a [u8]> {
        let section = self
            .sections
            .iter()
            .find(|s| s.contains(rva))
            .ok_or(Error::BadRva(rva))?;
        let start = self.rva_to_offset(rva)?;
        let end = (section.raw_offset as usize + section.raw_size as usize).min(self.data.len());
        self.data
            .get(start..end.max(start))
            .ok_or(Error::BadRva(rva))
    }

    pub fn metadata(&self) -> Result<&'a [u8]> {
        self.slice_at_rva(self.cli.metadata.rva, self.cli.metadata.size as usize)
    }
}
