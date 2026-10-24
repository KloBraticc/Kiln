use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Truncated { what: &'static str, offset: usize },
    BadMagic(&'static str),
    NotManaged,
    BadRva(u32),
    MissingStream(&'static str),
    BadTable(u8),
    BadIndex { table: &'static str, index: u32 },
    BadMethodBody { rva: u32, reason: &'static str },
    BadOpcode { offset: usize, byte: u8 },
    Io(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated { what, offset } => {
                write!(f, "{what} is truncated at offset {offset:#x}")
            }
            Error::BadMagic(what) => write!(f, "{what} has an invalid signature"),
            Error::NotManaged => f.write_str("the image has no CLI header"),
            Error::BadRva(rva) => write!(f, "RVA {rva:#x} is outside every section"),
            Error::MissingStream(name) => write!(f, "the metadata has no {name} stream"),
            Error::BadTable(id) => write!(f, "metadata table {id:#x} is unknown"),
            Error::BadIndex { table, index } => {
                write!(f, "index {index} is outside the {table} table")
            }
            Error::BadMethodBody { rva, reason } => {
                write!(f, "method body at RVA {rva:#x}: {reason}")
            }
            Error::BadOpcode { offset, byte } => {
                write!(f, "invalid opcode {byte:#04x} at IL offset {offset:#x}")
            }
            Error::Io(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
