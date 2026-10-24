mod schema;

pub use schema::{CodedIndex, Column, TABLE_COUNT, TableId};

use crate::error::{Error, Result};
use crate::reader::Reader;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Token(pub u32);

impl Token {
    pub const USER_STRING: u8 = 0x70;

    pub fn new(table: TableId, row: u32) -> Self {
        Self(((table as u32) << 24) | row)
    }

    pub fn kind(self) -> u8 {
        (self.0 >> 24) as u8
    }

    pub fn row(self) -> u32 {
        self.0 & 0x00FF_FFFF
    }

    pub fn table(self) -> Option<TableId> {
        TableId::from_raw(self.kind())
    }

    pub fn is_null(self) -> bool {
        self.row() == 0
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct ColumnLayout {
    offset: usize,
    wide: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Table {
    rows: u32,
    row_size: usize,
    start: usize,
    columns: Vec<ColumnLayout>,
}

impl Table {
    pub fn rows(&self) -> u32 {
        self.rows
    }

    pub fn row_size(&self) -> usize {
        self.row_size
    }
}

pub struct Metadata<'a> {
    pub version: String,
    strings: &'a [u8],
    user_strings: &'a [u8],
    blobs: &'a [u8],
    guids: &'a [u8],
    tables_data: &'a [u8],
    tables: [Table; TABLE_COUNT],
    pub uncompressed: bool,
}

impl<'a> Metadata<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let mut root = Reader::new(data, "metadata root");
        if root.u32()? != 0x424A_5342 {
            return Err(Error::BadMagic("metadata root"));
        }
        root.skip(8)?;
        let version_length = root.u32()? as usize;
        let raw_version = root.bytes(version_length)?;
        let version_end = raw_version
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(raw_version.len());
        let version = String::from_utf8_lossy(&raw_version[..version_end]).into_owned();
        root.skip(2)?;
        let stream_count = root.u16()?;

        let mut strings: &[u8] = &[];
        let mut user_strings: &[u8] = &[];
        let mut blobs: &[u8] = &[];
        let mut guids: &[u8] = &[];
        let mut tables_stream: Option<&[u8]> = None;
        let mut uncompressed = false;
        for _ in 0..stream_count {
            let offset = root.u32()? as usize;
            let size = root.u32()? as usize;
            let name = root.zero_terminated()?;
            root.align(4)?;
            let stream = data.get(offset..offset + size).ok_or(Error::Truncated {
                what: "metadata stream",
                offset,
            })?;
            match name {
                b"#Strings" => strings = stream,
                b"#US" => user_strings = stream,
                b"#Blob" => blobs = stream,
                b"#GUID" => guids = stream,
                b"#~" => tables_stream = Some(stream),
                b"#-" => {
                    tables_stream = Some(stream);
                    uncompressed = true;
                }
                _ => {}
            }
        }

        let tables_stream = tables_stream.ok_or(Error::MissingStream("#~"))?;
        let mut header = Reader::new(tables_stream, "table stream");
        header.skip(6)?;
        let heap_sizes = header.u8()?;
        header.skip(1)?;
        let valid = header.u64()?;
        header.skip(8)?;

        let mut row_counts = [0u32; TABLE_COUNT];
        for bit in 0..64u8 {
            if valid & (1u64 << bit) == 0 {
                continue;
            }
            let id = TableId::from_raw(bit).ok_or(Error::BadTable(bit))?;
            row_counts[id as usize] = header.u32()?;
        }
        if heap_sizes & 0x40 != 0 {
            header.skip(4)?;
        }

        let wide_strings = heap_sizes & 0x01 != 0;
        let wide_guids = heap_sizes & 0x02 != 0;
        let wide_blobs = heap_sizes & 0x04 != 0;
        let table_wide = |id: TableId| row_counts[id as usize] > 0xFFFF;
        let coded_wide = |coded: CodedIndex| {
            let largest = coded
                .tables()
                .iter()
                .flatten()
                .map(|&id| row_counts[id as usize])
                .max()
                .unwrap_or(0);
            largest >= (1u32 << (16 - coded.tag_bits()))
        };

        let mut tables: [Table; TABLE_COUNT] = std::array::from_fn(|_| Table::default());
        let mut cursor = header.position();
        for id in TableId::ALL {
            let mut offset = 0;
            let mut columns = Vec::with_capacity(id.columns().len());
            for column in id.columns() {
                let (wide, width) = match *column {
                    Column::U16 => (false, 2),
                    Column::U32 => (true, 4),
                    Column::Str => (wide_strings, if wide_strings { 4 } else { 2 }),
                    Column::Guid => (wide_guids, if wide_guids { 4 } else { 2 }),
                    Column::Blob => (wide_blobs, if wide_blobs { 4 } else { 2 }),
                    Column::Table(target) => {
                        (table_wide(target), if table_wide(target) { 4 } else { 2 })
                    }
                    Column::Coded(coded) => {
                        (coded_wide(coded), if coded_wide(coded) { 4 } else { 2 })
                    }
                };
                columns.push(ColumnLayout { offset, wide });
                offset += width;
            }
            let rows = row_counts[id as usize];
            tables[id as usize] = Table {
                rows,
                row_size: offset,
                start: cursor,
                columns,
            };
            cursor += offset * rows as usize;
        }
        if cursor > tables_stream.len() {
            return Err(Error::Truncated {
                what: "table stream",
                offset: cursor,
            });
        }

        Ok(Self {
            version,
            strings,
            user_strings,
            blobs,
            guids,
            tables_data: tables_stream,
            tables,
            uncompressed,
        })
    }

    pub fn table(&self, id: TableId) -> &Table {
        &self.tables[id as usize]
    }

    pub fn rows(&self, id: TableId) -> u32 {
        self.tables[id as usize].rows
    }

    pub fn cell(&self, id: TableId, row: u32, column: usize) -> Result<u32> {
        let table = &self.tables[id as usize];
        if row == 0 || row > table.rows {
            return Err(Error::BadIndex {
                table: id.name(),
                index: row,
            });
        }
        let layout = table.columns.get(column).ok_or(Error::BadIndex {
            table: id.name(),
            index: column as u32,
        })?;
        let at = table.start + (row as usize - 1) * table.row_size + layout.offset;
        let mut reader = Reader::at(self.tables_data, at, "table row")?;
        reader.index(layout.wide)
    }

    pub fn coded(&self, id: TableId, row: u32, column: usize) -> Result<Option<Token>> {
        let Column::Coded(coded) = id.columns()[column] else {
            return Err(Error::BadIndex {
                table: id.name(),
                index: column as u32,
            });
        };
        let raw = self.cell(id, row, column)?;
        let bits = coded.tag_bits();
        let tag = (raw & ((1 << bits) - 1)) as usize;
        let target_row = raw >> bits;
        let target = coded
            .tables()
            .get(tag)
            .copied()
            .flatten()
            .ok_or(Error::BadIndex {
                table: id.name(),
                index: raw,
            })?;
        Ok((target_row != 0).then(|| Token::new(target, target_row)))
    }

    pub fn string(&self, offset: u32) -> Result<&'a str> {
        let mut reader = Reader::at(self.strings, offset as usize, "#Strings heap")?;
        let bytes = reader.zero_terminated()?;
        std::str::from_utf8(bytes).map_err(|_| Error::BadIndex {
            table: "#Strings",
            index: offset,
        })
    }

    pub fn string_cell(&self, id: TableId, row: u32, column: usize) -> Result<&'a str> {
        self.string(self.cell(id, row, column)?)
    }

    pub fn blob(&self, offset: u32) -> Result<&'a [u8]> {
        let mut reader = Reader::at(self.blobs, offset as usize, "#Blob heap")?;
        let length = reader.compressed_u32()? as usize;
        reader.bytes(length)
    }

    pub fn guid(&self, index: u32) -> Result<Option<[u8; 16]>> {
        if index == 0 {
            return Ok(None);
        }
        let start = (index as usize - 1) * 16;
        let bytes = self.guids.get(start..start + 16).ok_or(Error::BadIndex {
            table: "#GUID",
            index,
        })?;
        Ok(Some(bytes.try_into().expect("slice is 16 bytes")))
    }

    pub fn user_string(&self, offset: u32) -> Result<String> {
        let mut reader = Reader::at(self.user_strings, offset as usize, "#US heap")?;
        let length = reader.compressed_u32()? as usize;
        let bytes = reader.bytes(length)?;
        let units: Vec<u16> = bytes[..length & !1]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        Ok(String::from_utf16_lossy(&units))
    }

    pub fn has_user_string(&self, offset: u32) -> bool {
        (offset as usize) < self.user_strings.len()
    }

    pub fn is_valid_token(&self, token: Token) -> bool {
        if token.kind() == Token::USER_STRING {
            return self.has_user_string(token.row());
        }
        match token.table() {
            Some(id) => token.row() != 0 && token.row() <= self.rows(id),
            None => false,
        }
    }

    pub fn type_name(&self, token: Token) -> Result<String> {
        match token.table() {
            Some(TableId::TypeDef) => self.qualified(TableId::TypeDef, token.row(), 2, 1),
            Some(TableId::TypeRef) => self.qualified(TableId::TypeRef, token.row(), 2, 1),
            Some(TableId::TypeSpec) => Ok(format!("TypeSpec#{}", token.row())),
            _ => Err(Error::BadIndex {
                table: "TypeDefOrRef",
                index: token.0,
            }),
        }
    }

    fn qualified(
        &self,
        id: TableId,
        row: u32,
        namespace_column: usize,
        name_column: usize,
    ) -> Result<String> {
        let namespace = self.string_cell(id, row, namespace_column)?;
        let name = self.string_cell(id, row, name_column)?;
        Ok(if namespace.is_empty() {
            name.to_owned()
        } else {
            format!("{namespace}.{name}")
        })
    }

    pub fn assembly_name(&self) -> Result<Option<String>> {
        if self.rows(TableId::Assembly) == 0 {
            return Ok(None);
        }
        let version: Vec<String> = (1..=4)
            .map(|column| {
                self.cell(TableId::Assembly, 1, column)
                    .map(|v| v.to_string())
            })
            .collect::<Result<_>>()?;
        Ok(Some(format!(
            "{}, Version={}",
            self.string_cell(TableId::Assembly, 1, 7)?,
            version.join(".")
        )))
    }

    pub fn method_range(&self, type_row: u32) -> Result<std::ops::Range<u32>> {
        self.list_range(TableId::TypeDef, type_row, 5, TableId::MethodDef)
    }

    pub fn field_range(&self, type_row: u32) -> Result<std::ops::Range<u32>> {
        self.list_range(TableId::TypeDef, type_row, 4, TableId::Field)
    }

    fn list_range(
        &self,
        owner: TableId,
        row: u32,
        column: usize,
        target: TableId,
    ) -> Result<std::ops::Range<u32>> {
        let start = self.cell(owner, row, column)?;
        let end = if row < self.rows(owner) {
            self.cell(owner, row + 1, column)?
        } else {
            self.rows(target) + 1
        };
        Ok(start..end.max(start))
    }
}
