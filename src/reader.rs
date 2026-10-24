use crate::error::{Error, Result};

#[derive(Clone, Copy)]
pub struct Reader<'a> {
    data: &'a [u8],
    position: usize,
    what: &'static str,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8], what: &'static str) -> Self {
        Self {
            data,
            position: 0,
            what,
        }
    }

    pub fn at(data: &'a [u8], position: usize, what: &'static str) -> Result<Self> {
        if position > data.len() {
            return Err(Error::Truncated {
                what,
                offset: position,
            });
        }
        Ok(Self {
            data,
            position,
            what,
        })
    }

    pub fn position(&self) -> usize {
        self.position
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.position
    }

    pub fn is_empty(&self) -> bool {
        self.position >= self.data.len()
    }

    pub fn seek(&mut self, position: usize) -> Result<()> {
        if position > self.data.len() {
            return Err(self.truncated(position));
        }
        self.position = position;
        Ok(())
    }

    pub fn skip(&mut self, count: usize) -> Result<()> {
        self.bytes(count).map(|_| ())
    }

    pub fn align(&mut self, alignment: usize) -> Result<()> {
        let aligned = self.position.next_multiple_of(alignment);
        self.seek(aligned.min(self.data.len()))
    }

    pub fn bytes(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(count)
            .ok_or_else(|| self.truncated(self.position))?;
        let slice = self
            .data
            .get(self.position..end)
            .ok_or_else(|| self.truncated(self.position))?;
        self.position = end;
        Ok(slice)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.bytes(N)?);
        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    pub fn index(&mut self, wide: bool) -> Result<u32> {
        if wide {
            self.u32()
        } else {
            self.u16().map(u32::from)
        }
    }

    pub fn compressed_u32(&mut self) -> Result<u32> {
        let first = self.u8()?;
        if first & 0x80 == 0 {
            Ok(u32::from(first))
        } else if first & 0xC0 == 0x80 {
            Ok((u32::from(first & 0x3F) << 8) | u32::from(self.u8()?))
        } else if first & 0xE0 == 0xC0 {
            let rest = self.array::<3>()?;
            Ok((u32::from(first & 0x1F) << 24)
                | (u32::from(rest[0]) << 16)
                | (u32::from(rest[1]) << 8)
                | u32::from(rest[2]))
        } else {
            Err(self.truncated(self.position))
        }
    }

    pub fn zero_terminated(&mut self) -> Result<&'a [u8]> {
        let rest = &self.data[self.position..];
        let length = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| self.truncated(self.position))?;
        self.position += length + 1;
        Ok(&rest[..length])
    }

    fn truncated(&self, offset: usize) -> Error {
        Error::Truncated {
            what: self.what,
            offset,
        }
    }
}
