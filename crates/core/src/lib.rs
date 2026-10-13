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
    /// Built on first use from `symbols`; parse results are shared immutably
    /// during analysis, so the summary stays consistent with its symbols.
    #[serde(skip)]
    summaries: std::sync::OnceLock<Vec<SymbolSummary>>,
    #[serde(skip)]
    owned_bytes: std::sync::OnceLock<usize>,
}

/// Compact per-symbol matching data. Lineage scans every preserved symbol for
/// each commit pair; reading these few bytes avoids touching full symbols.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SymbolSummary {
    pub tokens_prefix: u64,
    pub body_prefix: Option<u64>,
    pub has_error: bool,
}

/// Leading bytes of a uniform BLAKE3 fingerprint, usable as its hash.
pub fn fingerprint_prefix(hash: &[u8; 32]) -> u64 {
    let [a, b, c, d, e, f, g, h, ..] = *hash;
    u64::from_le_bytes([a, b, c, d, e, f, g, h])
}

impl ParsedRust {
    pub fn new(
        symbols: Vec<Symbol>,
        has_error: bool,
        version: ExtractorVersion,
        diagnostics: Vec<ParseDiagnostic>,
        unsupported: Vec<UnsupportedSyntax>,
    ) -> Self {
        Self {
            symbols,
            has_error,
            version,
            diagnostics,
            unsupported,
            summaries: std::sync::OnceLock::new(),
            owned_bytes: std::sync::OnceLock::new(),
        }
    }

    /// Heap bytes owned by this parse result, including its matching summary,
    /// for the analysis memory budget. Computed once per shared parse.
    pub fn owned_bytes(&self) -> usize {
        *self.owned_bytes.get_or_init(|| {
            self.symbols.capacity() * std::mem::size_of::<Symbol>()
                + self
                    .symbols
                    .iter()
                    .map(|s| {
                        s.name.capacity()
                            + s.context.capacity() * std::mem::size_of::<String>()
                            + s.context.iter().map(String::capacity).sum::<usize>()
                    })
                    .sum::<usize>()
                + self.symbols.len() * std::mem::size_of::<SymbolSummary>()
                + self.diagnostics.capacity() * std::mem::size_of::<ParseDiagnostic>()
                + self
                    .diagnostics
                    .iter()
                    .map(|d| d.syntax_kind.capacity())
                    .sum::<usize>()
                + self.unsupported.capacity() * std::mem::size_of::<UnsupportedSyntax>()
                + self
                    .unsupported
                    .iter()
                    .map(|d| d.syntax_kind.capacity())
                    .sum::<usize>()
        })
    }

    pub fn summaries(&self) -> &[SymbolSummary] {
        self.summaries.get_or_init(|| {
            self.symbols
                .iter()
                .map(|symbol| SymbolSummary {
                    tokens_prefix: fingerprint_prefix(&symbol.fingerprints.normalized_tokens),
                    body_prefix: symbol.fingerprints.body.as_ref().map(fingerprint_prefix),
                    has_error: symbol.has_error,
                })
                .collect()
        })
    }
}
