pub mod git;
pub mod index;
pub mod lineage;
pub mod store;
pub mod syntax;
pub mod trace;

pub const MAX_BLOB_BYTES: usize = 2 * 1024 * 1024;
pub const ANALYSIS_BUDGET_BYTES: usize = 512 * 1024 * 1024;
pub const CACHE_BUDGET_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SymbolKind {
    Function,
    Method,
    TraitMethod,
    Struct,
    Enum,
    Trait,
}

impl SymbolKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Method => "method",
            Self::TraitMethod => "trait method",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Trait => "trait",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub context: Vec<String>,
    pub start_byte: usize,
    pub end_byte: usize,
    pub line: usize,
    pub has_error: bool,
    pub signature_range: std::ops::Range<usize>,
    pub body_range: Option<std::ops::Range<usize>>,
    pub fingerprints: SymbolFingerprints,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SymbolFingerprints {
    pub raw: [u8; 32],
    pub normalized_tokens: [u8; 32],
    pub structural: [u8; 32],
    pub signature: [u8; 32],
    pub body: Option<[u8; 32]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtractorVersion {
    pub parser: &'static str,
    pub grammar: &'static str,
    pub query: u32,
    pub fingerprints: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ParseDiagnosticKind {
    Error,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ParseDiagnostic {
    pub kind: ParseDiagnosticKind,
    pub range: std::ops::Range<usize>,
    pub syntax_kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UnsupportedSyntax {
    pub range: std::ops::Range<usize>,
    pub syntax_kind: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ParsedRust {
    pub symbols: Vec<Symbol>,
    pub has_error: bool,
    #[serde(skip, default = "syntax::extractor_version")]
    pub version: ExtractorVersion,
    pub diagnostics: Vec<ParseDiagnostic>,
    pub unsupported: Vec<UnsupportedSyntax>,
}
