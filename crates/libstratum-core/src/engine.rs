//! Engine (plugin registry) and Session (one opened binary).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use libstratum_model::{
    Availability, BinaryId, BinaryInfo, Capabilities, ContainerFormat, DiagCode, Diagnostic, ImageIdentity, Lens,
    LensCapability, MissingInput, SectionInfo, SectionKey, Severity, Size,
};

use crate::error::OpenError;
use crate::host::{ByteSource, HostServices, InMemorySource};
use crate::spi::{
    ArtifactProvider, BinaryFormat, DebugInfoBackend, DebugReader, Demangler, Image, LanguageSupport, MapFileParser,
    OpenOptions, ProbeResult,
};

/// Bytes inspected by [`BinaryFormat::probe`].
const PROBE_LEN: usize = 4096;

/// Input to [`Engine::open`]. Path-based opening lives in the `libstratum` facade.
#[derive(Debug, Clone)]
pub enum Input {
    Bytes(Vec<u8>),
    Source(Arc<dyn ByteSource>),
}

/// Registers plugins explicitly and statically (docs/02 §4.7).
#[derive(Debug, Default)]
pub struct EngineBuilder {
    formats: Vec<Arc<dyn BinaryFormat>>,
    debug_backends: Vec<Arc<dyn DebugInfoBackend>>,
    map_parsers: Vec<Arc<dyn MapFileParser>>,
    languages: Vec<Arc<dyn LanguageSupport>>,
    demanglers: Vec<Arc<dyn Demangler>>,
    artifact_providers: Vec<Arc<dyn ArtifactProvider>>,
    host: HostServices,
}

impl EngineBuilder {
    pub fn format(mut self, format: impl BinaryFormat) -> Self {
        self.formats.push(Arc::new(format));
        self
    }

    pub fn debug_backend(mut self, backend: impl DebugInfoBackend) -> Self {
        self.debug_backends.push(Arc::new(backend));
        self
    }

    pub fn map_parser(mut self, parser: impl MapFileParser) -> Self {
        self.map_parsers.push(Arc::new(parser));
        self
    }

    pub fn language(mut self, language: impl LanguageSupport) -> Self {
        self.languages.push(Arc::new(language));
        self
    }

    pub fn demangler(mut self, demangler: impl Demangler) -> Self {
        self.demanglers.push(Arc::new(demangler));
        self
    }

    pub fn artifact_provider(mut self, provider: impl ArtifactProvider) -> Self {
        self.artifact_providers.push(Arc::new(provider));
        self
    }

    pub fn host(mut self, host: HostServices) -> Self {
        self.host = host;
        self
    }

    pub fn build(self) -> Engine {
        Engine { plugins: Arc::new(self) }
    }
}

/// A configured set of plugins. Cheap to clone; share across threads.
#[derive(Debug, Clone)]
pub struct Engine {
    plugins: Arc<EngineBuilder>,
}

impl Engine {
    pub fn builder() -> EngineBuilder {
        EngineBuilder::default()
    }

    pub fn format_ids(&self) -> Vec<&'static str> {
        self.plugins.formats.iter().map(|f| f.id()).collect()
    }

    pub fn demanglers(&self) -> &[Arc<dyn Demangler>] {
        &self.plugins.demanglers
    }

    pub fn languages(&self) -> &[Arc<dyn LanguageSupport>] {
        &self.plugins.languages
    }

    /// Opens a binary: probe → open → locate debug info → identity check.
    /// Missing or mismatched debug info produces diagnostics, not errors.
    pub fn open(&self, input: Input, options: &OpenOptions) -> Result<Session, OpenError> {
        let source: Arc<dyn ByteSource> = match input {
            Input::Bytes(bytes) => Arc::new(InMemorySource(bytes)),
            Input::Source(source) => source,
        };

        // No panic may escape the public API (docs/02 §6).
        catch_unwind(AssertUnwindSafe(|| self.open_inner(source, options)))
            .unwrap_or_else(|_| Err(OpenError::Internal("panic while opening input".into())))
    }

    fn open_inner(&self, source: Arc<dyn ByteSource>, options: &OpenOptions) -> Result<Session, OpenError> {
        let bytes = source.bytes().map_err(OpenError::Read)?;
        let header = &bytes[..bytes.len().min(PROBE_LEN)];

        let format =
            self.plugins.formats.iter().find(|f| f.probe(header) != ProbeResult::No).ok_or(OpenError::UnknownFormat)?;

        let image =
            format.open(source.clone(), options).map_err(|source| OpenError::Format { format: format.id(), source })?;

        let mut diagnostics = Vec::new();
        let debug = self.open_debug_info(image.as_ref(), &mut diagnostics);

        Ok(Session { inner: Arc::new(SessionInner { image, debug, diagnostics }) })
    }

    fn open_debug_info(&self, image: &dyn Image, diagnostics: &mut Vec<Diagnostic>) -> Vec<Box<dyn DebugReader>> {
        let mut readers = Vec::new();
        for location in image.debug_locations() {
            let Some(backend) = self.plugins.debug_backends.iter().find(|b| b.accepts(&location)) else {
                continue;
            };
            match backend.open(&location, image, &self.plugins.host) {
                Ok(reader) => {
                    if identities_conflict(&image.binary_id(), &reader.binary_id()) {
                        diagnostics.push(diag(
                            Severity::Error,
                            DiagCode::DebugInfoMismatch,
                            format!("{} debug info does not match the image", backend.id()),
                            Some(format!("{location:?}")),
                        ));
                    } else {
                        readers.push(reader);
                    }
                }
                Err(err) => diagnostics.push(diag(
                    Severity::Warning,
                    DiagCode::DebugInfoParseError,
                    format!("{}: {err}", backend.id()),
                    Some(format!("{location:?}")),
                )),
            }
        }
        if readers.is_empty() {
            diagnostics.push(diag(Severity::Warning, DiagCode::NoDebugInfo, "no usable debug info found".into(), None));
        }
        readers
    }
}

fn identities_conflict(image: &BinaryId, debug: &BinaryId) -> bool {
    !matches!(image, BinaryId::None) && !matches!(debug, BinaryId::None) && image != debug
}

fn diag(severity: Severity, code: DiagCode, message: String, subject: Option<String>) -> Diagnostic {
    Diagnostic { severity, code, message, subject }
}

#[derive(Debug)]
struct SessionInner {
    image: Box<dyn Image>,
    debug: Vec<Box<dyn DebugReader>>,
    diagnostics: Vec<Diagnostic>,
}

/// One opened binary. `Send + Sync`; cheap to clone.
#[derive(Debug, Clone)]
pub struct Session {
    inner: Arc<SessionInner>,
}

impl Session {
    pub fn image(&self) -> &dyn Image {
        self.inner.image.as_ref()
    }

    pub fn has_debug_info(&self) -> bool {
        !self.inner.debug.is_empty()
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.inner.diagnostics
    }

    pub fn info(&self) -> BinaryInfo {
        let image = self.image();
        let sections = image
            .sections()
            .iter()
            .map(|s| SectionInfo {
                key: SectionKey { segment: s.segment.clone(), name: s.name.clone() },
                vm_address: s.extent.vm.map(|r| r.start),
                load_address: s.extent.load.map(|r| r.start),
                size: Size {
                    vm: s.extent.vm.map_or(0, |r| r.size),
                    file: s.extent.file.map_or(0, |r| r.size),
                    load: s.extent.load.map_or(0, |r| r.size),
                },
            })
            .collect();
        BinaryInfo {
            identity: ImageIdentity {
                format: container_format(image.format_id()),
                arch: image.arch(),
                id: image.binary_id(),
                // TODO(M1): content hash over image bytes (docs/02 §8).
                content_hash: String::new(),
            },
            sections,
            diagnostics: self.inner.diagnostics.clone(),
        }
    }

    /// Which lenses can answer for this binary (ADR-0014).
    pub fn capabilities(&self) -> Capabilities {
        let lenses = [Lens::Layout, Lens::Symbols, Lens::Correlation, Lens::MemoryRegions]
            .into_iter()
            .map(|lens| LensCapability {
                lens,
                status: Availability::Unavailable,
                missing: vec![MissingInput::NotYetImplemented],
            })
            .collect();
        Capabilities { lenses }
    }
}

fn container_format(id: &str) -> ContainerFormat {
    match id {
        "elf" => ContainerFormat::Elf,
        "macho" => ContainerFormat::MachO,
        "pe" => ContainerFormat::Pe,
        _ => ContainerFormat::Other,
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;
    use crate::error::SpiError;
    use crate::ir::{Arch, DebugLocation, Endian, Section, SectionId, Segment, Symbol};

    #[derive(Debug)]
    struct FakeFormat;

    #[derive(Debug)]
    struct FakeImage;

    impl BinaryFormat for FakeFormat {
        fn id(&self) -> &'static str {
            "fake"
        }
        fn probe(&self, header: &[u8]) -> ProbeResult {
            if header.starts_with(b"FAKE") { ProbeResult::Yes } else { ProbeResult::No }
        }
        fn open(&self, _: Arc<dyn ByteSource>, _: &OpenOptions) -> Result<Box<dyn Image>, SpiError> {
            Ok(Box::new(FakeImage))
        }
    }

    impl Image for FakeImage {
        fn format_id(&self) -> &'static str {
            "fake"
        }
        fn arch(&self) -> Arch {
            Arch::X86_64
        }
        fn endian(&self) -> Endian {
            Endian::Little
        }
        fn address_size(&self) -> u8 {
            8
        }
        fn binary_id(&self) -> BinaryId {
            BinaryId::None
        }
        fn segments(&self) -> &[Segment] {
            &[]
        }
        fn sections(&self) -> &[Section] {
            &[]
        }
        fn symbols(&self) -> &[Symbol] {
            &[]
        }
        fn section_data(&self, _: SectionId) -> Result<Cow<'_, [u8]>, SpiError> {
            Err(SpiError::Unimplemented("fake"))
        }
        fn debug_locations(&self) -> Vec<DebugLocation> {
            vec![DebugLocation::Embedded]
        }
    }

    #[test]
    fn opens_with_registered_format_and_reports_missing_debug_info() {
        let engine = Engine::builder().format(FakeFormat).build();
        let session = engine.open(Input::Bytes(b"FAKE....".to_vec()), &OpenOptions::default()).unwrap();
        assert_eq!(session.image().format_id(), "fake");
        assert!(!session.has_debug_info());
        assert!(session.diagnostics().iter().any(|d| d.code == DiagCode::NoDebugInfo));
    }

    #[test]
    fn unknown_input_is_rejected() {
        let engine = Engine::builder().format(FakeFormat).build();
        let err = engine.open(Input::Bytes(b"nope".to_vec()), &OpenOptions::default()).unwrap_err();
        assert!(matches!(err, OpenError::UnknownFormat));
    }

    #[test]
    fn session_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Session>();
        assert_send_sync::<Engine>();
    }
}
