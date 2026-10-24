#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum TableId {
    Module = 0x00,
    TypeRef = 0x01,
    TypeDef = 0x02,
    FieldPtr = 0x03,
    Field = 0x04,
    MethodPtr = 0x05,
    MethodDef = 0x06,
    ParamPtr = 0x07,
    Param = 0x08,
    InterfaceImpl = 0x09,
    MemberRef = 0x0A,
    Constant = 0x0B,
    CustomAttribute = 0x0C,
    FieldMarshal = 0x0D,
    DeclSecurity = 0x0E,
    ClassLayout = 0x0F,
    FieldLayout = 0x10,
    StandAloneSig = 0x11,
    EventMap = 0x12,
    EventPtr = 0x13,
    Event = 0x14,
    PropertyMap = 0x15,
    PropertyPtr = 0x16,
    Property = 0x17,
    MethodSemantics = 0x18,
    MethodImpl = 0x19,
    ModuleRef = 0x1A,
    TypeSpec = 0x1B,
    ImplMap = 0x1C,
    FieldRva = 0x1D,
    EncLog = 0x1E,
    EncMap = 0x1F,
    Assembly = 0x20,
    AssemblyProcessor = 0x21,
    AssemblyOs = 0x22,
    AssemblyRef = 0x23,
    AssemblyRefProcessor = 0x24,
    AssemblyRefOs = 0x25,
    File = 0x26,
    ExportedType = 0x27,
    ManifestResource = 0x28,
    NestedClass = 0x29,
    GenericParam = 0x2A,
    MethodSpec = 0x2B,
    GenericParamConstraint = 0x2C,
}

pub const TABLE_COUNT: usize = 0x2D;

impl TableId {
    pub const ALL: [TableId; TABLE_COUNT] = [
        TableId::Module,
        TableId::TypeRef,
        TableId::TypeDef,
        TableId::FieldPtr,
        TableId::Field,
        TableId::MethodPtr,
        TableId::MethodDef,
        TableId::ParamPtr,
        TableId::Param,
        TableId::InterfaceImpl,
        TableId::MemberRef,
        TableId::Constant,
        TableId::CustomAttribute,
        TableId::FieldMarshal,
        TableId::DeclSecurity,
        TableId::ClassLayout,
        TableId::FieldLayout,
        TableId::StandAloneSig,
        TableId::EventMap,
        TableId::EventPtr,
        TableId::Event,
        TableId::PropertyMap,
        TableId::PropertyPtr,
        TableId::Property,
        TableId::MethodSemantics,
        TableId::MethodImpl,
        TableId::ModuleRef,
        TableId::TypeSpec,
        TableId::ImplMap,
        TableId::FieldRva,
        TableId::EncLog,
        TableId::EncMap,
        TableId::Assembly,
        TableId::AssemblyProcessor,
        TableId::AssemblyOs,
        TableId::AssemblyRef,
        TableId::AssemblyRefProcessor,
        TableId::AssemblyRefOs,
        TableId::File,
        TableId::ExportedType,
        TableId::ManifestResource,
        TableId::NestedClass,
        TableId::GenericParam,
        TableId::MethodSpec,
        TableId::GenericParamConstraint,
    ];

    pub fn from_raw(raw: u8) -> Option<Self> {
        Self::ALL.get(raw as usize).copied()
    }

    pub fn name(self) -> &'static str {
        match self {
            TableId::Module => "Module",
            TableId::TypeRef => "TypeRef",
            TableId::TypeDef => "TypeDef",
            TableId::FieldPtr => "FieldPtr",
            TableId::Field => "Field",
            TableId::MethodPtr => "MethodPtr",
            TableId::MethodDef => "MethodDef",
            TableId::ParamPtr => "ParamPtr",
            TableId::Param => "Param",
            TableId::InterfaceImpl => "InterfaceImpl",
            TableId::MemberRef => "MemberRef",
            TableId::Constant => "Constant",
            TableId::CustomAttribute => "CustomAttribute",
            TableId::FieldMarshal => "FieldMarshal",
            TableId::DeclSecurity => "DeclSecurity",
            TableId::ClassLayout => "ClassLayout",
            TableId::FieldLayout => "FieldLayout",
            TableId::StandAloneSig => "StandAloneSig",
            TableId::EventMap => "EventMap",
            TableId::EventPtr => "EventPtr",
            TableId::Event => "Event",
            TableId::PropertyMap => "PropertyMap",
            TableId::PropertyPtr => "PropertyPtr",
            TableId::Property => "Property",
            TableId::MethodSemantics => "MethodSemantics",
            TableId::MethodImpl => "MethodImpl",
            TableId::ModuleRef => "ModuleRef",
            TableId::TypeSpec => "TypeSpec",
            TableId::ImplMap => "ImplMap",
            TableId::FieldRva => "FieldRVA",
            TableId::EncLog => "EncLog",
            TableId::EncMap => "EncMap",
            TableId::Assembly => "Assembly",
            TableId::AssemblyProcessor => "AssemblyProcessor",
            TableId::AssemblyOs => "AssemblyOS",
            TableId::AssemblyRef => "AssemblyRef",
            TableId::AssemblyRefProcessor => "AssemblyRefProcessor",
            TableId::AssemblyRefOs => "AssemblyRefOS",
            TableId::File => "File",
            TableId::ExportedType => "ExportedType",
            TableId::ManifestResource => "ManifestResource",
            TableId::NestedClass => "NestedClass",
            TableId::GenericParam => "GenericParam",
            TableId::MethodSpec => "MethodSpec",
            TableId::GenericParamConstraint => "GenericParamConstraint",
        }
    }

    pub fn columns(self) -> &'static [Column] {
        use CodedIndex as C;
        use Column::*;
        use TableId as T;
        match self {
            T::Module => &[U16, Str, Guid, Guid, Guid],
            T::TypeRef => &[Coded(C::ResolutionScope), Str, Str],
            T::TypeDef => &[
                U32,
                Str,
                Str,
                Coded(C::TypeDefOrRef),
                Table(T::Field),
                Table(T::MethodDef),
            ],
            T::FieldPtr => &[Table(T::Field)],
            T::Field => &[U16, Str, Blob],
            T::MethodPtr => &[Table(T::MethodDef)],
            T::MethodDef => &[U32, U16, U16, Str, Blob, Table(T::Param)],
            T::ParamPtr => &[Table(T::Param)],
            T::Param => &[U16, U16, Str],
            T::InterfaceImpl => &[Table(T::TypeDef), Coded(C::TypeDefOrRef)],
            T::MemberRef => &[Coded(C::MemberRefParent), Str, Blob],
            T::Constant => &[U16, Coded(C::HasConstant), Blob],
            T::CustomAttribute => &[
                Coded(C::HasCustomAttribute),
                Coded(C::CustomAttributeType),
                Blob,
            ],
            T::FieldMarshal => &[Coded(C::HasFieldMarshal), Blob],
            T::DeclSecurity => &[U16, Coded(C::HasDeclSecurity), Blob],
            T::ClassLayout => &[U16, U32, Table(T::TypeDef)],
            T::FieldLayout => &[U32, Table(T::Field)],
            T::StandAloneSig => &[Blob],
            T::EventMap => &[Table(T::TypeDef), Table(T::Event)],
            T::EventPtr => &[Table(T::Event)],
            T::Event => &[U16, Str, Coded(C::TypeDefOrRef)],
            T::PropertyMap => &[Table(T::TypeDef), Table(T::Property)],
            T::PropertyPtr => &[Table(T::Property)],
            T::Property => &[U16, Str, Blob],
            T::MethodSemantics => &[U16, Table(T::MethodDef), Coded(C::HasSemantics)],
            T::MethodImpl => &[
                Table(T::TypeDef),
                Coded(C::MethodDefOrRef),
                Coded(C::MethodDefOrRef),
            ],
            T::ModuleRef => &[Str],
            T::TypeSpec => &[Blob],
            T::ImplMap => &[U16, Coded(C::MemberForwarded), Str, Table(T::ModuleRef)],
            T::FieldRva => &[U32, Table(T::Field)],
            T::EncLog => &[U32, U32],
            T::EncMap => &[U32],
            T::Assembly => &[U32, U16, U16, U16, U16, U32, Blob, Str, Str],
            T::AssemblyProcessor => &[U32],
            T::AssemblyOs => &[U32, U32, U32],
            T::AssemblyRef => &[U16, U16, U16, U16, U32, Blob, Str, Str, Blob],
            T::AssemblyRefProcessor => &[U32, Table(T::AssemblyRef)],
            T::AssemblyRefOs => &[U32, U32, U32, Table(T::AssemblyRef)],
            T::File => &[U32, Str, Blob],
            T::ExportedType => &[U32, U32, Str, Str, Coded(C::Implementation)],
            T::ManifestResource => &[U32, U32, Str, Coded(C::Implementation)],
            T::NestedClass => &[Table(T::TypeDef), Table(T::TypeDef)],
            T::GenericParam => &[U16, U16, Coded(C::TypeOrMethodDef), Str],
            T::MethodSpec => &[Coded(C::MethodDefOrRef), Blob],
            T::GenericParamConstraint => &[Table(T::GenericParam), Coded(C::TypeDefOrRef)],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    U16,
    U32,
    Str,
    Guid,
    Blob,
    Table(TableId),
    Coded(CodedIndex),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodedIndex {
    TypeDefOrRef,
    HasConstant,
    HasCustomAttribute,
    HasFieldMarshal,
    HasDeclSecurity,
    MemberRefParent,
    HasSemantics,
    MethodDefOrRef,
    MemberForwarded,
    Implementation,
    CustomAttributeType,
    ResolutionScope,
    TypeOrMethodDef,
}

impl CodedIndex {
    pub fn tables(self) -> &'static [Option<TableId>] {
        use TableId as T;
        match self {
            CodedIndex::TypeDefOrRef => &[Some(T::TypeDef), Some(T::TypeRef), Some(T::TypeSpec)],
            CodedIndex::HasConstant => &[Some(T::Field), Some(T::Param), Some(T::Property)],
            CodedIndex::HasCustomAttribute => &[
                Some(T::MethodDef),
                Some(T::Field),
                Some(T::TypeRef),
                Some(T::TypeDef),
                Some(T::Param),
                Some(T::InterfaceImpl),
                Some(T::MemberRef),
                Some(T::Module),
                Some(T::DeclSecurity),
                Some(T::Property),
                Some(T::Event),
                Some(T::StandAloneSig),
                Some(T::ModuleRef),
                Some(T::TypeSpec),
                Some(T::Assembly),
                Some(T::AssemblyRef),
                Some(T::File),
                Some(T::ExportedType),
                Some(T::ManifestResource),
                Some(T::GenericParam),
                Some(T::GenericParamConstraint),
                Some(T::MethodSpec),
            ],
            CodedIndex::HasFieldMarshal => &[Some(T::Field), Some(T::Param)],
            CodedIndex::HasDeclSecurity => {
                &[Some(T::TypeDef), Some(T::MethodDef), Some(T::Assembly)]
            }
            CodedIndex::MemberRefParent => &[
                Some(T::TypeDef),
                Some(T::TypeRef),
                Some(T::ModuleRef),
                Some(T::MethodDef),
                Some(T::TypeSpec),
            ],
            CodedIndex::HasSemantics => &[Some(T::Event), Some(T::Property)],
            CodedIndex::MethodDefOrRef => &[Some(T::MethodDef), Some(T::MemberRef)],
            CodedIndex::MemberForwarded => &[Some(T::Field), Some(T::MethodDef)],
            CodedIndex::Implementation => {
                &[Some(T::File), Some(T::AssemblyRef), Some(T::ExportedType)]
            }
            CodedIndex::CustomAttributeType => {
                &[None, None, Some(T::MethodDef), Some(T::MemberRef), None]
            }
            CodedIndex::ResolutionScope => &[
                Some(T::Module),
                Some(T::ModuleRef),
                Some(T::AssemblyRef),
                Some(T::TypeRef),
            ],
            CodedIndex::TypeOrMethodDef => &[Some(T::TypeDef), Some(T::MethodDef)],
        }
    }

    pub fn tag_bits(self) -> u32 {
        let count = self.tables().len() as u32;
        u32::BITS - (count - 1).leading_zeros()
    }
}
