//! DWARF debug-info backend (ELF, Mach-O dSYM and debug-map objects, PE built with MinGW).
//! `gimli` types stay inside this crate (ADR-0018).

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use gimli::{AttributeValue, DwTag, EndianArcSlice, Reader as _, RunTimeEndian, UnitOffset, constants};
use libstratum_core::SpiError;
use libstratum_core::host::DebugOpenContext;
use libstratum_core::ir::{
    AggregateKind, BinaryId, DebugLocation, DiscardEvidence, Endian, FunctionInfo, InlineTree, LineTable, RawLayout,
    RawLayoutEntry, SourceLanguage, SourceLoc, UnitId, UnitInfo,
};
use libstratum_core::spi::{DebugInfoBackend, DebugReader, Image};

type R = EndianArcSlice<RunTimeEndian>;

#[derive(Debug, Clone, Copy, Default)]
pub struct Dwarf;

impl DebugInfoBackend for Dwarf {
    fn id(&self) -> &'static str {
        "dwarf"
    }

    fn accepts(&self, location: &DebugLocation) -> bool {
        matches!(
            location,
            DebugLocation::Embedded
                | DebugLocation::Dsym { .. }
                | DebugLocation::MachOObject { .. }
                | DebugLocation::DebugLink { .. }
                | DebugLocation::BuildId { .. }
        )
    }

    fn open(
        &self,
        location: &DebugLocation,
        image: &dyn Image,
        context: &DebugOpenContext<'_>,
    ) -> Result<Box<dyn DebugReader>, SpiError> {
        let reader = match location {
            DebugLocation::Embedded => DwarfReader::load(image, image.binary_id())?,
            _ => {
                let source =
                    context.locate(location)?.ok_or_else(|| SpiError::NotFound(format!("{location:?} not found")))?;
                let companion = context.open_image(source)?;
                // Debug-map objects have no identity of their own; they belong to the image by construction.
                let id = match location {
                    DebugLocation::MachOObject { .. } => BinaryId::None,
                    _ => companion.binary_id(),
                };
                let mut reader = DwarfReader::load(companion.as_ref(), id)?;
                // Debug-map objects hold pre-link addresses until the debug map is applied (M4).
                reader.final_addresses = !matches!(location, DebugLocation::MachOObject { .. });
                reader
            }
        };
        Ok(Box::new(reader))
    }
}

/// Maps a DWARF section name to its name in `image`: `.debug_info` in ELF/PE,
/// `__debug_info` (truncated to 16 bytes) in Mach-O.
fn section_bytes(image: &dyn Image, name: &str) -> Result<Option<Arc<[u8]>>, SpiError> {
    let macho: String = format!("__{}", &name[1..]).chars().take(16).collect();
    let Some(section) = image.sections().iter().find(|s| s.name == name || s.name == macho) else { return Ok(None) };
    let data = image.section_data(section.id)?;
    Ok(Some(Arc::from(match data {
        Cow::Borrowed(b) => b.to_vec(),
        Cow::Owned(v) => v,
    })))
}

/// A named aggregate definition found while indexing.
#[derive(Debug, Clone)]
struct TypeEntry {
    unit: usize,
    offset: UnitOffset,
}

pub struct DwarfReader {
    dwarf: gimli::Dwarf<R>,
    units: Vec<gimli::Unit<R>>,
    /// Type units by signature: `DW_FORM_ref_sig8` references resolve through this.
    signatures: HashMap<u64, (usize, UnitOffset)>,
    address_size: u8,
    endian: Endian,
    binary_id: BinaryId,
    /// Addresses in this DWARF are final image addresses (false for unrelocated debug-map objects).
    final_addresses: bool,
    /// Qualified name → definitions (and names seen only as declarations). Built on the first type query.
    type_index: OnceLock<Result<TypeIndex, String>>,
}

#[derive(Debug, Default)]
struct TypeIndex {
    definitions: HashMap<String, Vec<TypeEntry>>,
    declarations: std::collections::HashSet<String>,
}

impl std::fmt::Debug for DwarfReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DwarfReader").field("units", &self.units.len()).field("binary_id", &self.binary_id).finish()
    }
}

fn gimli_err(err: gimli::Error) -> SpiError {
    SpiError::Malformed(format!("DWARF: {err}"))
}

impl DwarfReader {
    fn load(image: &dyn Image, binary_id: BinaryId) -> Result<Self, SpiError> {
        let endian = match image.endian() {
            Endian::Little => RunTimeEndian::Little,
            Endian::Big => RunTimeEndian::Big,
        };
        if section_bytes(image, ".debug_info")?.is_none() {
            return Err(SpiError::NotFound("no .debug_info section".into()));
        }
        let dwarf = gimli::Dwarf::load(|id| -> Result<R, SpiError> {
            let data = section_bytes(image, id.name())?.unwrap_or_else(|| Arc::from(Vec::new()));
            Ok(EndianArcSlice::new(data, endian))
        })?;
        let mut units = Vec::new();
        let mut headers = dwarf.units();
        while let Some(header) = headers.next().map_err(gimli_err)? {
            units.push(dwarf.unit(header).map_err(gimli_err)?);
        }
        // DWARF 4 type units live in `.debug_types` (DWARF 5 puts them in `.debug_info`, above).
        let mut type_headers = dwarf.type_units();
        while let Some(header) = type_headers.next().map_err(gimli_err)? {
            units.push(dwarf.unit(header).map_err(gimli_err)?);
        }
        let signatures = units
            .iter()
            .enumerate()
            .filter_map(|(i, unit)| match unit.header.type_() {
                gimli::UnitType::Type { type_signature, type_offset }
                | gimli::UnitType::SplitType { type_signature, type_offset } => {
                    Some((type_signature.0, (i, type_offset)))
                }
                _ => None,
            })
            .collect();
        Ok(Self {
            dwarf,
            units,
            signatures,
            address_size: image.address_size(),
            endian: image.endian(),
            binary_id,
            final_addresses: true,
            type_index: OnceLock::new(),
        })
    }

    fn unit_ref(&self, unit: usize) -> gimli::UnitRef<'_, R> {
        self.units[unit].unit_ref(&self.dwarf)
    }

    fn name_of(&self, unit: usize, entry: &gimli::DebuggingInformationEntry<R>) -> Option<String> {
        let value = entry.attr_value(constants::DW_AT_name)?;
        let name = self.unit_ref(unit).attr_string(value).ok()?;
        name.to_string_lossy().ok().map(Cow::into_owned)
    }

    /// Walks every unit once, recording complete named aggregates under their qualified names.
    fn build_index(&self) -> Result<TypeIndex, SpiError> {
        let mut index = TypeIndex::default();
        for unit in 0..self.units.len() {
            let unit_ref = self.unit_ref(unit);
            let mut cursor = unit_ref.entries();
            // (depth, name) of enclosing namespaces and types.
            let mut scopes: Vec<(isize, Option<String>)> = Vec::new();
            while let Some(entry) = cursor.next_dfs().map_err(gimli_err)? {
                let depth = entry.depth();
                while scopes.last().is_some_and(|(d, _)| *d >= depth) {
                    scopes.pop();
                }
                let tag = entry.tag();
                let scoping = matches!(
                    tag,
                    constants::DW_TAG_namespace
                        | constants::DW_TAG_structure_type
                        | constants::DW_TAG_class_type
                        | constants::DW_TAG_union_type
                );
                if !scoping {
                    continue;
                }
                // Type-unit scopes can be nameless declaration stubs that only carry DW_AT_signature
                // (Clang emits `Outer` this way around `Inner`); the name lives in the referenced unit.
                let name = self.name_of(unit, entry).or_else(|| {
                    let signature = entry.attr_value(constants::DW_AT_signature)?;
                    let (target_unit, target) = self.resolve_once(unit, &signature)?;
                    self.name_of(target_unit, &target)
                });
                if tag != constants::DW_TAG_namespace
                    && let Some(name) = &name
                {
                    let mut qualified: Vec<&str> = scopes.iter().filter_map(|(_, n)| n.as_deref()).collect();
                    qualified.push(name);
                    let qualified = qualified.join("::");
                    if flag(entry, constants::DW_AT_declaration) {
                        index.declarations.insert(qualified);
                    } else {
                        index
                            .definitions
                            .entry(qualified)
                            .or_default()
                            .push(TypeEntry { unit, offset: entry.offset() });
                    }
                }
                if entry.has_children() {
                    scopes.push((
                        depth,
                        name.or_else(|| (tag == constants::DW_TAG_namespace).then(|| "(anonymous namespace)".into())),
                    ));
                }
            }
        }
        Ok(index)
    }

    fn layout(&self, name: &str, entry: &TypeEntry) -> Result<RawLayout, SpiError> {
        let unit_ref = self.unit_ref(entry.unit);
        let mut cursor = unit_ref.entries_at_offset(entry.offset).map_err(gimli_err)?;
        let die = cursor
            .next_dfs()
            .map_err(gimli_err)?
            .ok_or_else(|| SpiError::Malformed("missing type DIE".into()))?
            .clone();
        let kind = match die.tag() {
            constants::DW_TAG_class_type => AggregateKind::Class,
            constants::DW_TAG_union_type => AggregateKind::Union,
            _ => AggregateKind::Struct,
        };
        let byte_size = die.attr(constants::DW_AT_byte_size).and_then(|a| a.udata_value()).unwrap_or(0);
        let alignment = die.attr(constants::DW_AT_alignment).and_then(|a| a.udata_value());
        let decl = self.decl(entry.unit, &die);

        let mut entries = Vec::new();
        if die.has_children() {
            let base_depth = die.depth();
            while let Some(child) = cursor.next_dfs().map_err(gimli_err)? {
                if child.depth() <= base_depth {
                    break;
                }
                if child.depth() != base_depth + 1 {
                    continue;
                }
                match child.tag() {
                    constants::DW_TAG_member => {
                        if let Some(e) = self.member(entry.unit, child) {
                            entries.push(e);
                        }
                    }
                    constants::DW_TAG_inheritance => {
                        let ty = child.attr_value(constants::DW_AT_type);
                        let offset = member_offset_bytes(child).map(|b| b * 8);
                        let is_virtual = child
                            .attr_value(constants::DW_AT_virtuality)
                            .is_some_and(|v| !matches!(v, AttributeValue::Virtuality(constants::DW_VIRTUALITY_none)));
                        entries.push(RawLayoutEntry::Base {
                            type_name: ty.as_ref().map_or_else(|| "?".into(), |t| self.type_name(entry.unit, t, 0)),
                            offset_bits: offset,
                            size_bits: ty.as_ref().and_then(|t| self.type_size(entry.unit, t, 0)).unwrap_or(0) * 8,
                            is_virtual,
                        });
                    }
                    _ => {}
                }
            }
        }
        let has_virtual_bases = self.has_virtual_bases(entry.unit, &die, 0);
        Ok(RawLayout {
            name: name.into(),
            kind,
            byte_size,
            alignment,
            decl,
            is_declaration: false,
            has_virtual_bases,
            entries,
        })
    }

    fn member(&self, unit: usize, child: &gimli::DebuggingInformationEntry<R>) -> Option<RawLayoutEntry> {
        let name = self.name_of(unit, child);
        // Static data members: DWARF 4 marks them as external declarations without a location.
        if child.attr(constants::DW_AT_data_member_location).is_none()
            && child.attr(constants::DW_AT_data_bit_offset).is_none()
            && flag(child, constants::DW_AT_declaration)
        {
            return None;
        }
        let ty = child.attr_value(constants::DW_AT_type);
        let type_name = ty.as_ref().map_or_else(|| "?".into(), |t| self.type_name(unit, t, 0));
        let offset_bytes = member_offset_bytes(child).unwrap_or(0);

        if flag(child, constants::DW_AT_artificial) && name.as_deref().is_some_and(|n| n.starts_with("_vptr")) {
            return Some(RawLayoutEntry::VtablePtr {
                offset_bits: offset_bytes * 8,
                size_bits: u64::from(self.address_size) * 8,
            });
        }

        if let Some(width) = child.attr(constants::DW_AT_bit_size).and_then(|a| a.udata_value()) {
            let offset_bits =
                if let Some(bits) = child.attr(constants::DW_AT_data_bit_offset).and_then(|a| a.udata_value()) {
                    bits
                } else {
                    // DWARF 2/3: DW_AT_bit_offset counts from the most significant bit of the storage unit.
                    let storage = child
                        .attr(constants::DW_AT_byte_size)
                        .and_then(|a| a.udata_value())
                        .or_else(|| ty.as_ref().and_then(|t| self.type_size(unit, t, 0)))
                        .unwrap_or(0);
                    let bit_offset = child.attr(constants::DW_AT_bit_offset).and_then(|a| a.udata_value()).unwrap_or(0);
                    match self.endian {
                        Endian::Little => (offset_bytes * 8 + storage * 8).saturating_sub(bit_offset + width),
                        Endian::Big => offset_bytes * 8 + bit_offset,
                    }
                };
            return Some(RawLayoutEntry::Bitfield { name, type_name, offset_bits, width_bits: width as u32 });
        }

        let size_bytes = ty.as_ref().and_then(|t| self.type_size(unit, t, 0)).unwrap_or(0);
        Some(RawLayoutEntry::Field {
            name,
            type_name,
            offset_bits: offset_bytes * 8,
            size_bits: size_bytes * 8,
            align_bytes: ty.as_ref().and_then(|t| self.type_align(unit, t, 0)),
            empty_type: ty.as_ref().is_some_and(|t| self.type_is_empty(unit, t, 0)),
        })
    }

    /// Whether a type has no data: a class/struct without non-static data members, vtable pointer or
    /// virtual bases, whose bases are all empty too (followed through typedefs and cv-qualifiers).
    fn type_is_empty(&self, unit: usize, value: &AttributeValue<R>, depth: usize) -> bool {
        if depth > Self::MAX_DEPTH {
            return false;
        }
        let Some((unit, die)) = self.resolve(unit, value) else { return false };
        match die.tag() {
            constants::DW_TAG_typedef | constants::DW_TAG_const_type | constants::DW_TAG_volatile_type => {
                die.attr_value(constants::DW_AT_type).is_some_and(|t| self.type_is_empty(unit, &t, depth + 1))
            }
            constants::DW_TAG_structure_type | constants::DW_TAG_class_type => {
                if flag(&die, constants::DW_AT_declaration) {
                    return false;
                }
                let Ok(mut tree) = self.unit_ref(unit).entries_tree(Some(die.offset())) else { return false };
                let Ok(root) = tree.root() else { return false };
                let mut children = root.children();
                while let Ok(Some(child)) = children.next() {
                    let e = child.entry();
                    match e.tag() {
                        constants::DW_TAG_member if !flag(e, constants::DW_AT_declaration) => return false,
                        constants::DW_TAG_inheritance => {
                            let is_virtual = e.attr_value(constants::DW_AT_virtuality).is_some_and(|v| {
                                !matches!(v, AttributeValue::Virtuality(constants::DW_VIRTUALITY_none))
                            });
                            let base_empty = e
                                .attr_value(constants::DW_AT_type)
                                .is_some_and(|t| self.type_is_empty(unit, &t, depth + 1));
                            if is_virtual || !base_empty {
                                return false;
                            }
                        }
                        _ => {}
                    }
                }
                true
            }
            _ => false,
        }
    }

    fn decl(&self, unit: usize, die: &gimli::DebuggingInformationEntry<R>) -> Option<SourceLoc> {
        let line = die.attr(constants::DW_AT_decl_line).and_then(|a| a.udata_value())?;
        let file_index = die.attr(constants::DW_AT_decl_file).and_then(|a| a.udata_value())?;
        let unit_ref = self.unit_ref(unit);
        let program = unit_ref.line_program.as_ref()?;
        let header = program.header();
        let file = header.file(file_index)?;
        let name = unit_ref.attr_string(file.path_name()).ok()?.to_string_lossy().ok()?.into_owned();
        let is_absolute = name.starts_with('/') || name.as_bytes().get(1) == Some(&b':');
        let mut path = String::new();
        if !is_absolute
            && let Some(dir) = file.directory(header)
            && let Ok(dir) = unit_ref.attr_string(dir)
        {
            path.push_str(&dir.to_string_lossy().ok()?);
            path.push('/');
        }
        path.push_str(&name);
        Some(SourceLoc { file: libstratum_core::ir::normalize_source_path(&path), line: line as u32, column: None })
    }

    /// Follows a type reference to its DIE: same-unit, cross-unit, or type-unit signature. A
    /// declaration stub carrying `DW_AT_signature` is followed to the definition in its type unit.
    fn resolve(&self, unit: usize, value: &AttributeValue<R>) -> Option<(usize, gimli::DebuggingInformationEntry<R>)> {
        let (unit, die) = self.resolve_once(unit, value)?;
        if flag(&die, constants::DW_AT_declaration)
            && let Some(signature @ AttributeValue::DebugTypesRef(_)) = die.attr_value(constants::DW_AT_signature)
        {
            return self.resolve_once(unit, &signature).or(Some((unit, die)));
        }
        Some((unit, die))
    }

    fn resolve_once(
        &self,
        unit: usize,
        value: &AttributeValue<R>,
    ) -> Option<(usize, gimli::DebuggingInformationEntry<R>)> {
        match value {
            AttributeValue::UnitRef(offset) => self.unit_ref(unit).entry(*offset).ok().map(|e| (unit, e)),
            AttributeValue::DebugTypesRef(signature) => {
                let (unit, offset) = *self.signatures.get(&signature.0)?;
                self.unit_ref(unit).entry(offset).ok().map(|e| (unit, e))
            }
            AttributeValue::DebugInfoRef(offset) => {
                // Cross-unit reference (e.g. GCC LTO): find the unit containing the offset.
                self.units.iter().enumerate().find_map(|(i, u)| {
                    let local = offset.to_unit_offset(&u.header)?;
                    self.unit_ref(i).entry(local).ok().map(|e| (i, e))
                })
            }
            _ => None,
        }
    }

    const MAX_DEPTH: usize = 32;

    fn type_name(&self, unit: usize, value: &AttributeValue<R>, depth: usize) -> String {
        if depth > Self::MAX_DEPTH {
            return "?".into();
        }
        let Some((unit, die)) = self.resolve(unit, value) else { return "?".into() };
        let inner = || die.attr_value(constants::DW_AT_type).map(|t| self.type_name(unit, &t, depth + 1));
        match die.tag() {
            constants::DW_TAG_pointer_type => format!("{}*", inner().unwrap_or_else(|| "void".into())),
            constants::DW_TAG_reference_type => format!("{}&", inner().unwrap_or_default()),
            constants::DW_TAG_rvalue_reference_type => format!("{}&&", inner().unwrap_or_default()),
            constants::DW_TAG_const_type => format!("const {}", inner().unwrap_or_else(|| "void".into())),
            constants::DW_TAG_volatile_type => format!("volatile {}", inner().unwrap_or_else(|| "void".into())),
            constants::DW_TAG_array_type => {
                let mut dims = String::new();
                if let Ok(mut tree) = self.unit_ref(unit).entries_tree(Some(die.offset()))
                    && let Ok(root) = tree.root()
                {
                    let mut children = root.children();
                    while let Ok(Some(child)) = children.next() {
                        let e = child.entry();
                        if e.tag() == constants::DW_TAG_subrange_type {
                            match subrange_count(e) {
                                Some(n) => dims.push_str(&format!("[{n}]")),
                                None => dims.push_str("[]"),
                            }
                        }
                    }
                }
                format!("{}{dims}", inner().unwrap_or_else(|| "?".into()))
            }
            constants::DW_TAG_subroutine_type => "fn(...)".into(),
            constants::DW_TAG_ptr_to_member_type => "member pointer".into(),
            tag => self.name_of(unit, &die).unwrap_or_else(|| anonymous_name(tag)),
        }
    }

    fn type_size(&self, unit: usize, value: &AttributeValue<R>, depth: usize) -> Option<u64> {
        if depth > Self::MAX_DEPTH {
            return None;
        }
        let (unit, die) = self.resolve(unit, value)?;
        if let Some(size) = die.attr(constants::DW_AT_byte_size).and_then(|a| a.udata_value()) {
            return Some(size);
        }
        match die.tag() {
            constants::DW_TAG_pointer_type
            | constants::DW_TAG_reference_type
            | constants::DW_TAG_rvalue_reference_type
            | constants::DW_TAG_ptr_to_member_type => Some(u64::from(self.address_size)),
            constants::DW_TAG_array_type => {
                let elem = self.type_size(unit, &die.attr_value(constants::DW_AT_type)?, depth + 1)?;
                Some(elem * self.array_elements(unit, &die)?)
            }
            _ => self.type_size(unit, &die.attr_value(constants::DW_AT_type)?, depth + 1),
        }
    }

    fn type_align(&self, unit: usize, value: &AttributeValue<R>, depth: usize) -> Option<u64> {
        if depth > Self::MAX_DEPTH {
            return None;
        }
        let (unit, die) = self.resolve(unit, value)?;
        if let Some(align) = die.attr(constants::DW_AT_alignment).and_then(|a| a.udata_value()) {
            return Some(align);
        }
        match die.tag() {
            constants::DW_TAG_base_type | constants::DW_TAG_enumeration_type => {
                // Scalars align to their size on every supported ABI, except 64-bit integers and
                // doubles on 32-bit x86 (not a Tier 1 target) and long double, which may be larger.
                die.attr(constants::DW_AT_byte_size).and_then(|a| a.udata_value()).map(|s| s.clamp(1, 16))
            }
            constants::DW_TAG_pointer_type
            | constants::DW_TAG_reference_type
            | constants::DW_TAG_rvalue_reference_type
            | constants::DW_TAG_ptr_to_member_type => Some(u64::from(self.address_size)),
            constants::DW_TAG_structure_type | constants::DW_TAG_class_type | constants::DW_TAG_union_type => {
                let mut tree = self.unit_ref(unit).entries_tree(Some(die.offset())).ok()?;
                let root = tree.root().ok()?;
                let mut children = root.children();
                let mut align = 1u64;
                while let Ok(Some(child)) = children.next() {
                    let e = child.entry();
                    if matches!(e.tag(), constants::DW_TAG_member | constants::DW_TAG_inheritance)
                        && !flag(e, constants::DW_AT_declaration)
                    {
                        if flag(e, constants::DW_AT_artificial) {
                            align = align.max(u64::from(self.address_size));
                        } else if let Some(t) = e.attr_value(constants::DW_AT_type) {
                            align = align.max(self.type_align(unit, &t, depth + 1)?);
                        }
                    }
                }
                Some(align)
            }
            _ => self.type_align(unit, &die.attr_value(constants::DW_AT_type)?, depth + 1),
        }
    }

    /// Whether `die` or any of its bases (recursively) has a virtual base.
    fn has_virtual_bases(&self, unit: usize, die: &gimli::DebuggingInformationEntry<R>, depth: usize) -> bool {
        if depth > Self::MAX_DEPTH {
            return false;
        }
        let Ok(mut tree) = self.unit_ref(unit).entries_tree(Some(die.offset())) else { return false };
        let Ok(root) = tree.root() else { return false };
        let mut children = root.children();
        while let Ok(Some(child)) = children.next() {
            let e = child.entry();
            if e.tag() != constants::DW_TAG_inheritance {
                continue;
            }
            if e.attr_value(constants::DW_AT_virtuality)
                .is_some_and(|v| !matches!(v, AttributeValue::Virtuality(constants::DW_VIRTUALITY_none)))
            {
                return true;
            }
            if let Some((base_unit, base)) = e.attr_value(constants::DW_AT_type).and_then(|t| self.resolve(unit, &t))
                && self.has_virtual_bases(base_unit, &base, depth + 1)
            {
                return true;
            }
        }
        false
    }

    fn array_elements(&self, unit: usize, die: &gimli::DebuggingInformationEntry<R>) -> Option<u64> {
        let mut tree = self.unit_ref(unit).entries_tree(Some(die.offset())).ok()?;
        let root = tree.root().ok()?;
        let mut children = root.children();
        let mut total = 1u64;
        let mut any = false;
        while let Ok(Some(child)) = children.next() {
            let e = child.entry();
            if e.tag() == constants::DW_TAG_subrange_type {
                total = total.checked_mul(subrange_count(e)?)?;
                any = true;
            }
        }
        any.then_some(total)
    }
}

fn flag(entry: &gimli::DebuggingInformationEntry<R>, name: constants::DwAt) -> bool {
    matches!(entry.attr_value(name), Some(AttributeValue::Flag(true)))
}

fn subrange_count(e: &gimli::DebuggingInformationEntry<R>) -> Option<u64> {
    if let Some(count) = e.attr(constants::DW_AT_count).and_then(|a| a.udata_value()) {
        return Some(count);
    }
    let upper = e.attr(constants::DW_AT_upper_bound).and_then(|a| a.udata_value())?;
    let lower = e.attr(constants::DW_AT_lower_bound).and_then(|a| a.udata_value()).unwrap_or(0);
    upper.checked_sub(lower)?.checked_add(1)
}

/// `DW_AT_data_member_location` as a constant, or a `DW_OP_plus_uconst` expression.
fn member_offset_bytes(entry: &gimli::DebuggingInformationEntry<R>) -> Option<u64> {
    let attr = entry.attr(constants::DW_AT_data_member_location)?;
    if let Some(value) = attr.udata_value() {
        return Some(value);
    }
    let expr = attr.exprloc_value()?;
    let mut reader = expr.0;
    let op = reader.read_u8().ok()?;
    if op == constants::DW_OP_plus_uconst.0 {
        let value = reader.read_uleb128().ok()?;
        return reader.is_empty().then_some(value);
    }
    None
}

fn anonymous_name(tag: DwTag) -> String {
    match tag {
        constants::DW_TAG_union_type => "(anonymous union)".into(),
        constants::DW_TAG_structure_type | constants::DW_TAG_class_type => "(anonymous struct)".into(),
        constants::DW_TAG_enumeration_type => "(anonymous enum)".into(),
        _ => "?".into(),
    }
}

impl DebugReader for DwarfReader {
    fn binary_id(&self) -> BinaryId {
        self.binary_id.clone()
    }

    fn units(&self) -> Result<Vec<UnitInfo>, SpiError> {
        Ok(self
            .units
            .iter()
            .enumerate()
            .map(|(i, u)| UnitInfo {
                id: UnitId(i as u32),
                name: u.name.as_ref().and_then(|n| n.to_string_lossy().ok().map(Cow::into_owned)).unwrap_or_default(),
                comp_dir: u.comp_dir.as_ref().and_then(|n| n.to_string_lossy().ok().map(Cow::into_owned)),
                language: SourceLanguage::Unknown,
                producer: None,
                ranges: Vec::new(),
            })
            .collect())
    }

    fn unit_for_address(&self, _address: u64) -> Result<Option<UnitId>, SpiError> {
        Err(SpiError::Unimplemented("address → unit (M4)"))
    }

    fn find_types(&self, matches: &dyn Fn(&str) -> bool) -> Result<Vec<RawLayout>, SpiError> {
        let index = self.type_index.get_or_init(|| self.build_index().map_err(|e| e.to_string()));
        let index = index.as_ref().map_err(|e| SpiError::Malformed(e.clone()))?;
        let mut out = Vec::new();
        for (name, entries) in index.definitions.iter().filter(|(name, _)| matches(name)) {
            for entry in entries {
                out.push(self.layout(name, entry)?);
            }
        }
        // Names that exist only as declarations: the definition is elsewhere or was omitted.
        for name in index.declarations.iter().filter(|n| matches(n) && !index.definitions.contains_key(*n)) {
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

    fn functions(&self, unit: UnitId) -> Result<Vec<FunctionInfo>, SpiError> {
        if !self.final_addresses {
            return Err(SpiError::Unimplemented("function addresses in debug-map objects (M4)"));
        }
        let index = unit.0 as usize;
        if index >= self.units.len() {
            return Ok(Vec::new());
        }
        let unit_ref = self.unit_ref(index);
        let mut cursor = unit_ref.entries();
        let mut out = Vec::new();
        while let Some(entry) = cursor.next_dfs().map_err(gimli_err)? {
            if entry.tag() != constants::DW_TAG_subprogram || flag(entry, constants::DW_AT_declaration) {
                continue;
            }
            let mut ranges = Vec::new();
            let mut iter = unit_ref.die_ranges(entry).map_err(gimli_err)?;
            while let Some(range) = iter.next().map_err(gimli_err)? {
                // Discarded or folded-away functions keep tombstoned addresses (M0 S8).
                if range.begin == 0 || range.begin >= u64::MAX - 1 || range.end <= range.begin {
                    continue;
                }
                ranges.push(libstratum_core::ir::AddrRange { start: range.begin, size: range.end - range.begin });
            }
            if ranges.is_empty() {
                continue;
            }
            let linkage_name = entry
                .attr_value(constants::DW_AT_linkage_name)
                .or_else(|| entry.attr_value(constants::DW_AT_MIPS_linkage_name))
                .and_then(|v| unit_ref.attr_string(v).ok())
                .and_then(|n| n.to_string_lossy().ok().map(Cow::into_owned));
            out.push(FunctionInfo {
                reference: libstratum_core::ir::DebugRef { unit: Some(unit), key: entry.offset().0 as u64 },
                name: self.name_of(index, entry).unwrap_or_default(),
                linkage_name,
                ranges,
                decl: self.decl(index, entry),
            });
        }
        Ok(out)
    }

    fn variables(&self) -> Result<Vec<libstratum_core::ir::VariableInfo>, SpiError> {
        if !self.final_addresses {
            return Err(SpiError::Unimplemented("variable addresses in debug-map objects (M4)"));
        }
        let mut out = Vec::new();
        for index in 0..self.units.len() {
            let unit_ref = self.unit_ref(index);
            let mut cursor = unit_ref.entries();
            while let Some(entry) = cursor.next_dfs().map_err(gimli_err)? {
                if entry.tag() != constants::DW_TAG_variable || flag(entry, constants::DW_AT_declaration) {
                    continue;
                }
                // Only statically allocated variables: a location that is exactly `DW_OP_addr <address>`.
                let Some(expr) = entry.attr(constants::DW_AT_location).and_then(|a| a.exprloc_value()) else {
                    continue;
                };
                let mut reader = expr.0;
                if reader.read_u8().ok() != Some(constants::DW_OP_addr.0) {
                    continue;
                }
                let address = match self.address_size {
                    8 => reader.read_u64().ok(),
                    4 => reader.read_u32().ok().map(u64::from),
                    _ => None,
                };
                let Some(address) = address.filter(|&a| a != 0 && reader.is_empty()) else { continue };
                let Some(size) = entry.attr_value(constants::DW_AT_type).and_then(|t| self.type_size(index, &t, 0))
                else {
                    continue;
                };
                let linkage_name = entry
                    .attr_value(constants::DW_AT_linkage_name)
                    .and_then(|v| unit_ref.attr_string(v).ok())
                    .and_then(|n| n.to_string_lossy().ok().map(Cow::into_owned));
                out.push(libstratum_core::ir::VariableInfo {
                    name: self.name_of(index, entry).unwrap_or_default(),
                    linkage_name,
                    address,
                    size,
                });
            }
        }
        Ok(out)
    }

    fn inline_trees(&self, _unit: UnitId) -> Result<Vec<InlineTree>, SpiError> {
        Err(SpiError::Unimplemented("inline trees (M4)"))
    }

    fn line_table(&self, _unit: UnitId) -> Result<LineTable, SpiError> {
        Err(SpiError::Unimplemented("line tables (M4)"))
    }

    fn discarded_functions(&self, _unit: UnitId) -> Result<Vec<DiscardEvidence>, SpiError> {
        Err(SpiError::Unimplemented("discard evidence (M4)"))
    }
}
