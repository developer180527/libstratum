//! PDB (CodeView) debug-info backend for PE images built by MSVC or clang-cl.
//! Pure Rust via `pdb2` (Q14); no dependency on the Windows-only DIA SDK. `pdb2` types stay
//! inside this crate (ADR-0018).

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;

use libstratum_core::SpiError;
use libstratum_core::host::DebugOpenContext;
use libstratum_core::ir::{
    AggregateKind, BinaryId, DebugLocation, DiscardEvidence, FunctionInfo, InlineTree, LineTable, RawLayout,
    RawLayoutEntry, UnitId, UnitInfo,
};
use libstratum_core::spi::{DebugInfoBackend, DebugReader, Image};
use pdb2::{FallibleIterator, PrimitiveKind, TypeData, TypeFinder, TypeIndex};

#[derive(Debug, Clone, Copy, Default)]
pub struct Pdb;

impl DebugInfoBackend for Pdb {
    fn id(&self) -> &'static str {
        "pdb"
    }

    fn accepts(&self, location: &DebugLocation) -> bool {
        matches!(location, DebugLocation::Pdb { .. })
    }

    fn open(
        &self,
        location: &DebugLocation,
        image: &dyn Image,
        context: &DebugOpenContext<'_>,
    ) -> Result<Box<dyn DebugReader>, SpiError> {
        let source = context.locate(location)?.ok_or_else(|| SpiError::NotFound(format!("{location:?} not found")))?;
        let bytes: Arc<[u8]> = Arc::from(source.bytes()?);
        let reader = PdbReader { bytes, address_size: image.address_size() };
        // Validate eagerly so a corrupt or mismatched file is reported at open.
        let _ = reader.identity()?;
        Ok(Box::new(reader))
    }
}

fn pdb_err(err: pdb2::Error) -> SpiError {
    SpiError::Malformed(format!("PDB: {err}"))
}

/// Holds the PDB bytes; the `pdb2` parser is re-created per query, which is cheap (it maps the
/// MSF stream directory only) and keeps the reader `Send + Sync`.
pub struct PdbReader {
    bytes: Arc<[u8]>,
    address_size: u8,
}

impl std::fmt::Debug for PdbReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PdbReader").field("bytes", &self.bytes.len()).finish()
    }
}

type Parser = pdb2::PDB<'static, Cursor<Arc<[u8]>>>;

/// A class or union definition found in the TPI stream.
struct Candidate {
    index: Option<TypeIndex>,
    name: String,
    kind: AggregateKind,
    size: u64,
    fields: Option<TypeIndex>,
}

impl PdbReader {
    fn parser(&self) -> Result<Parser, SpiError> {
        pdb2::PDB::open(Cursor::new(self.bytes.clone())).map_err(pdb_err)
    }

    fn identity(&self) -> Result<BinaryId, SpiError> {
        let mut pdb = self.parser()?;
        let info = pdb.pdb_information().map_err(pdb_err)?;
        // The executable's RSDS age matches the DBI stream age (M1 finding).
        let age = pdb.debug_information().map_err(pdb_err)?.age().unwrap_or(info.age);
        Ok(BinaryId::PdbGuidAge { guid: info.guid.to_bytes_le(), age })
    }
}

struct TypeContext<'a, 't> {
    finder: &'a TypeFinder<'t>,
    /// Definitions by name, to resolve forward references used in member types.
    definitions: &'a HashMap<String, (u64, Option<TypeIndex>)>,
    address_size: u8,
}

const MAX_DEPTH: usize = 32;

impl TypeContext<'_, '_> {
    fn data(&self, index: TypeIndex) -> Option<TypeData<'_>> {
        self.finder.find(index).ok()?.parse().ok()
    }

    fn name(&self, index: TypeIndex, depth: usize) -> String {
        if depth > MAX_DEPTH {
            return "?".into();
        }
        match self.data(index) {
            Some(TypeData::Primitive(p)) => {
                let base = primitive_name(p.kind);
                if p.indirection.is_some() { format!("{base}*") } else { base.into() }
            }
            Some(TypeData::Class(c)) => c.name.to_string().into_owned(),
            Some(TypeData::Union(u)) => u.name.to_string().into_owned(),
            Some(TypeData::Enumeration(e)) => e.name.to_string().into_owned(),
            Some(TypeData::Pointer(p)) => {
                let suffix = if p.attributes.is_reference() { "&" } else { "*" };
                format!("{}{suffix}", self.name(p.underlying_type, depth + 1))
            }
            Some(TypeData::Modifier(m)) => {
                let mut prefix = String::new();
                if m.constant {
                    prefix.push_str("const ");
                }
                if m.volatile {
                    prefix.push_str("volatile ");
                }
                format!("{prefix}{}", self.name(m.underlying_type, depth + 1))
            }
            Some(TypeData::Array(a)) => {
                let elem = self.size(a.element_type, depth + 1).unwrap_or(0);
                let bytes = u64::from(a.dimensions.last().copied().unwrap_or(0));
                let count = bytes.checked_div(elem).unwrap_or(0);
                format!("{}[{count}]", self.name(a.element_type, depth + 1))
            }
            Some(TypeData::Procedure(_)) | Some(TypeData::MemberFunction(_)) => "fn(...)".into(),
            _ => "?".into(),
        }
    }

    fn size(&self, index: TypeIndex, depth: usize) -> Option<u64> {
        if depth > MAX_DEPTH {
            return None;
        }
        match self.data(index)? {
            TypeData::Primitive(p) if p.indirection.is_some() => Some(u64::from(self.address_size)),
            TypeData::Primitive(p) => primitive_size(p.kind),
            TypeData::Class(c) if c.properties.forward_reference() => {
                self.definitions.get(c.name.to_string().as_ref()).map(|(size, _)| *size)
            }
            TypeData::Class(c) => Some(c.size),
            TypeData::Union(u) if u.properties.forward_reference() => {
                self.definitions.get(u.name.to_string().as_ref()).map(|(size, _)| *size)
            }
            TypeData::Union(u) => Some(u.size),
            TypeData::Enumeration(e) => self.size(e.underlying_type, depth + 1),
            TypeData::Pointer(_) => Some(u64::from(self.address_size)),
            TypeData::Modifier(m) => self.size(m.underlying_type, depth + 1),
            TypeData::Array(a) => a.dimensions.last().map(|&d| u64::from(d)),
            TypeData::Bitfield(b) => self.size(b.underlying_type, depth + 1),
            _ => None,
        }
    }

    fn align(&self, index: TypeIndex, depth: usize) -> Option<u64> {
        if depth > MAX_DEPTH {
            return None;
        }
        match self.data(index)? {
            TypeData::Primitive(p) if p.indirection.is_some() => Some(u64::from(self.address_size)),
            TypeData::Primitive(p) => primitive_size(p.kind).map(|s| s.clamp(1, 16)),
            TypeData::Pointer(_) => Some(u64::from(self.address_size)),
            TypeData::Modifier(m) => self.align(m.underlying_type, depth + 1),
            TypeData::Enumeration(e) => self.align(e.underlying_type, depth + 1),
            TypeData::Array(a) => self.align(a.element_type, depth + 1),
            TypeData::Class(c) => {
                let fields = if c.properties.forward_reference() {
                    self.definitions.get(c.name.to_string().as_ref())?.1
                } else {
                    c.fields
                };
                self.aggregate_align(fields?, depth + 1)
            }
            TypeData::Union(u) => {
                let fields = if u.properties.forward_reference() {
                    self.definitions.get(u.name.to_string().as_ref())?.1?
                } else {
                    u.fields
                };
                self.aggregate_align(fields, depth + 1)
            }
            _ => None,
        }
    }

    /// Whether a type has no data: a class without data members, vfptr or virtual bases, whose bases
    /// are empty too (followed through cv-modifiers and forward references).
    fn is_empty(&self, index: TypeIndex, depth: usize) -> bool {
        if depth > MAX_DEPTH {
            return false;
        }
        let fields = match self.data(index) {
            Some(TypeData::Modifier(m)) => return self.is_empty(m.underlying_type, depth + 1),
            Some(TypeData::Class(c)) if c.properties.forward_reference() => {
                match self.definitions.get(c.name.to_string().as_ref()) {
                    Some((_, fields)) => *fields,
                    None => return false,
                }
            }
            Some(TypeData::Class(c)) => c.fields,
            _ => return false,
        };
        let Some(fields) = fields else { return true };
        self.fields(fields).iter().all(|field| match field {
            TypeData::Member(_) | TypeData::VirtualFunctionTablePointer(_) | TypeData::VirtualBaseClass(_) => false,
            TypeData::BaseClass(b) => self.is_empty(b.base_class, depth + 1),
            _ => true,
        })
    }

    fn aggregate_align(&self, fields: TypeIndex, depth: usize) -> Option<u64> {
        let mut align = 1u64;
        for field in self.fields(fields) {
            match field {
                TypeData::Member(m) => align = align.max(self.align(m.field_type, depth + 1)?),
                TypeData::BaseClass(b) => align = align.max(self.align(b.base_class, depth + 1)?),
                TypeData::VirtualBaseClass(_) | TypeData::VirtualFunctionTablePointer(_) => {
                    align = align.max(u64::from(self.address_size))
                }
                _ => {}
            }
        }
        Some(align)
    }

    /// All field records of a field list, following continuations.
    fn fields(&self, start: TypeIndex) -> Vec<TypeData<'_>> {
        let mut out = Vec::new();
        let mut next = Some(start);
        let mut guard = 0;
        while let Some(index) = next.take() {
            guard += 1;
            if guard > 1024 {
                break;
            }
            let Some(TypeData::FieldList(list)) = self.data(index) else { break };
            next = list.continuation;
            out.extend(list.fields);
        }
        out
    }
}

fn primitive_size(kind: PrimitiveKind) -> Option<u64> {
    use PrimitiveKind::*;
    Some(match kind {
        Char | UChar | RChar | I8 | U8 | Bool8 | Char8 => 1,
        WChar | RChar16 | Short | UShort | I16 | U16 | Bool16 | F16 => 2,
        RChar32 | Long | ULong | I32 | U32 | F32 | F32PP | Bool32 | HRESULT => 4,
        F48 => 6,
        Quad | UQuad | I64 | U64 | F64 | Bool64 | Complex32 => 8,
        F80 => 10,
        Octa | UOcta | I128 | U128 | F128 | Complex64 => 16,
        _ => return None,
    })
}

fn primitive_name(kind: PrimitiveKind) -> &'static str {
    use PrimitiveKind::*;
    match kind {
        Void => "void",
        Char | RChar => "char",
        UChar => "unsigned char",
        I8 => "signed char",
        U8 => "unsigned char",
        WChar => "wchar_t",
        RChar16 => "char16_t",
        RChar32 => "char32_t",
        Char8 => "char8_t",
        Short | I16 => "short",
        UShort | U16 => "unsigned short",
        Long => "long",
        ULong => "unsigned long",
        I32 => "int",
        U32 => "unsigned int",
        Quad | I64 => "long long",
        UQuad | U64 => "unsigned long long",
        F32 => "float",
        F64 => "double",
        F80 => "long double",
        Bool8 => "bool",
        HRESULT => "HRESULT",
        _ => "?",
    }
}

impl PdbReader {
    /// `LF_UDT_SRC_LINE` / `LF_UDT_MOD_SRC_LINE` records: where each type was defined.
    fn declaration_lines(
        &self,
        pdb: &mut Parser,
    ) -> Result<HashMap<TypeIndex, libstratum_core::ir::SourceLoc>, SpiError> {
        let ids = pdb.id_information().map_err(pdb_err)?;
        let strings = pdb.string_table().ok();
        let mut finder = ids.finder();
        let mut iter = ids.iter();
        let mut sources = Vec::new();
        while let Some(item) = iter.next().map_err(pdb_err)? {
            finder.update(&iter);
            if let Ok(pdb2::IdData::UserDefinedTypeSource(source)) = item.parse() {
                sources.push(source);
            }
        }
        let mut out = HashMap::new();
        for source in sources {
            let file = match source.source_file {
                pdb2::UserDefinedTypeSourceFileRef::Local(id) => match finder.find(id).and_then(|i| i.parse()) {
                    Ok(pdb2::IdData::String(s)) => s.name.to_string().into_owned(),
                    _ => continue,
                },
                pdb2::UserDefinedTypeSourceFileRef::Remote(_, reference) => {
                    match strings.as_ref().and_then(|t| reference.to_string_lossy(t).ok()) {
                        Some(name) => name.into_owned(),
                        None => continue,
                    }
                }
            };
            out.insert(
                source.udt,
                libstratum_core::ir::SourceLoc {
                    file: libstratum_core::ir::normalize_source_path(&file),
                    line: source.line,
                    column: None,
                },
            );
        }
        Ok(out)
    }
}

impl DebugReader for PdbReader {
    fn binary_id(&self) -> BinaryId {
        self.identity().unwrap_or(BinaryId::None)
    }

    fn units(&self) -> Result<Vec<UnitInfo>, SpiError> {
        let pdb = &mut self.parser()?;
        let dbi = pdb.debug_information().map_err(pdb_err)?;
        let mut modules = dbi.modules().map_err(pdb_err)?;
        let mut units = Vec::new();
        while let Some(module) = modules.next().map_err(pdb_err)? {
            units.push(UnitInfo {
                id: UnitId(units.len() as u32),
                name: module.module_name().to_string(),
                comp_dir: None,
                language: libstratum_core::ir::SourceLanguage::Unknown,
                producer: None,
                ranges: Vec::new(),
            });
        }
        Ok(units)
    }

    fn unit_for_address(&self, _address: u64) -> Result<Option<UnitId>, SpiError> {
        Err(SpiError::Unimplemented("address → module (M4)"))
    }

    fn find_types(&self, matches: &dyn Fn(&str) -> bool) -> Result<Vec<RawLayout>, SpiError> {
        let mut pdb = self.parser()?;
        let info = pdb.type_information().map_err(pdb_err)?;
        let mut finder = info.finder();
        let mut iter = info.iter();
        let mut candidates = Vec::new();
        let mut forward_names = std::collections::HashSet::new();
        let mut definitions: HashMap<String, (u64, Option<TypeIndex>)> = HashMap::new();
        while let Some(item) = iter.next().map_err(pdb_err)? {
            finder.update(&iter);
            let candidate = match item.parse() {
                Ok(TypeData::Class(c)) if !c.properties.forward_reference() => Candidate {
                    index: Some(item.index()),
                    name: c.name.to_string().into_owned(),
                    kind: if matches!(c.kind, pdb2::ClassKind::Class) {
                        AggregateKind::Class
                    } else {
                        AggregateKind::Struct
                    },
                    size: c.size,
                    fields: c.fields,
                },
                Ok(TypeData::Union(u)) if !u.properties.forward_reference() => Candidate {
                    index: Some(item.index()),
                    name: u.name.to_string().into_owned(),
                    kind: AggregateKind::Union,
                    size: u.size,
                    fields: Some(u.fields),
                },
                Ok(TypeData::Class(c)) => {
                    forward_names.insert(c.name.to_string().into_owned());
                    continue;
                }
                Ok(TypeData::Union(u)) => {
                    forward_names.insert(u.name.to_string().into_owned());
                    continue;
                }
                _ => continue,
            };
            definitions.entry(candidate.name.clone()).or_insert((candidate.size, candidate.fields));
            candidates.push(candidate);
        }

        let decls = self.declaration_lines(&mut pdb).unwrap_or_default();
        let context = TypeContext { finder: &finder, definitions: &definitions, address_size: self.address_size };
        let mut out = Vec::new();
        for candidate in candidates.iter().filter(|c| matches(&c.name)) {
            let mut entries = Vec::new();
            let mut has_virtual_bases = false;
            for field in candidate.fields.map(|f| context.fields(f)).unwrap_or_default() {
                match field {
                    TypeData::Member(m) => {
                        let name = Some(m.name.to_string().into_owned());
                        match context.data(m.field_type) {
                            Some(TypeData::Bitfield(b)) => entries.push(RawLayoutEntry::Bitfield {
                                name,
                                type_name: context.name(b.underlying_type, 0),
                                offset_bits: m.offset * 8 + u64::from(b.position),
                                width_bits: u32::from(b.length),
                            }),
                            _ => entries.push(RawLayoutEntry::Field {
                                name,
                                type_name: context.name(m.field_type, 0),
                                offset_bits: m.offset * 8,
                                size_bits: context.size(m.field_type, 0).unwrap_or(0) * 8,
                                align_bytes: context.align(m.field_type, 0),
                                empty_type: context.is_empty(m.field_type, 0),
                            }),
                        }
                    }
                    TypeData::BaseClass(b) => entries.push(RawLayoutEntry::Base {
                        type_name: context.name(b.base_class, 0),
                        offset_bits: Some(u64::from(b.offset) * 8),
                        size_bits: context.size(b.base_class, 0).unwrap_or(0) * 8,
                        is_virtual: false,
                    }),
                    TypeData::VirtualBaseClass(v) => {
                        has_virtual_bases = true;
                        if v.direct {
                            entries.push(RawLayoutEntry::Base {
                                type_name: context.name(v.base_class, 0),
                                offset_bits: None,
                                size_bits: context.size(v.base_class, 0).unwrap_or(0) * 8,
                                is_virtual: true,
                            });
                        }
                    }
                    // MSVC places the vfptr at offset 0 of the class that introduces it.
                    TypeData::VirtualFunctionTablePointer(_) => entries.push(RawLayoutEntry::VtablePtr {
                        offset_bits: 0,
                        size_bits: u64::from(self.address_size) * 8,
                    }),
                    _ => {}
                }
            }
            // Bases that themselves have virtual bases make this type's layout depend on hidden members too.
            if !has_virtual_bases {
                has_virtual_bases = entries.iter().any(|e| matches!(e, RawLayoutEntry::Base { type_name, .. }
                    if definitions.get(type_name).and_then(|(_, f)| *f).is_some_and(|f| context.fields(f).iter().any(|x| matches!(x, TypeData::VirtualBaseClass(_))))));
            }
            out.push(RawLayout {
                name: candidate.name.clone(),
                kind: candidate.kind,
                byte_size: candidate.size,
                alignment: None,
                decl: candidate.index.and_then(|i| decls.get(&i).cloned()),
                is_declaration: false,
                has_virtual_bases,
                entries,
            });
        }
        for name in forward_names.iter().filter(|n| matches(n) && !definitions.contains_key(*n)) {
            out.push(RawLayout {
                name: name.clone(),
                kind: AggregateKind::Struct,
                byte_size: 0,
                alignment: None,
                decl: None,
                is_declaration: true,
                has_virtual_bases: false,
                entries: Vec::new(),
            });
        }
        Ok(out)
    }

    fn functions(&self, _unit: UnitId) -> Result<Vec<FunctionInfo>, SpiError> {
        Err(SpiError::Unimplemented("functions (M4)"))
    }

    fn inline_trees(&self, _unit: UnitId) -> Result<Vec<InlineTree>, SpiError> {
        Err(SpiError::Unimplemented("inline sites (M4)"))
    }

    fn line_table(&self, _unit: UnitId) -> Result<LineTable, SpiError> {
        Err(SpiError::Unimplemented("C13 lines (M4)"))
    }

    fn discarded_functions(&self, _unit: UnitId) -> Result<Vec<DiscardEvidence>, SpiError> {
        Err(SpiError::Unimplemented("discard evidence (M4)"))
    }
}
