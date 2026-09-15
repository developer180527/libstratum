use std::fmt::Debug;
use std::sync::Arc;

use crate::error::SpiError;
use crate::host::ByteSource;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FactKind {
    OptimizationRemarks,
    TimeTrace,
    StackSizes,
    Profile,
    HardwareDescription,
}

/// Facts loaded from a non-binary artifact, keyed only by shared join keys (ADR-0013).
pub trait ArtifactFacts: Send + Sync + Debug {
    fn kind(&self) -> FactKind;
}

/// Loads a non-binary artifact: optimization remarks, time traces, stack sizes, SVD, profiles.
pub trait ArtifactProvider: Send + Sync + Debug + 'static {
    fn id(&self) -> &'static str;

    fn sniff(&self, name: &str, head: &[u8]) -> bool;

    fn load(&self, source: Arc<dyn ByteSource>) -> Result<Box<dyn ArtifactFacts>, SpiError>;
}
