use anyhow::{Context, Result, bail};
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

use crate::{
    ExtractorVersion, MAX_BLOB_BYTES, ParseDiagnostic, ParseDiagnosticKind, ParsedRust, Symbol,
    SymbolFingerprints, SymbolKind, UnsupportedSyntax,
};

pub const EXTRACTOR_VERSION: ExtractorVersion = ExtractorVersion {
    parser: "tree-sitter-0.27.1",
    grammar: "tree-sitter-rust-0.24.2",
    query: 1,
    fingerprints: 1,
};

pub const fn extractor_version() -> ExtractorVersion {
    EXTRACTOR_VERSION
}

const DECLARATIONS: &str = r#"
(function_item name: (identifier) @name) @symbol
(function_signature_item name: (identifier) @name) @symbol
(struct_item name: (type_identifier) @name) @symbol
(enum_item name: (type_identifier) @name) @symbol
(trait_item name: (type_identifier) @name) @symbol
"#;

/// The declaration query is constant, so compile it once per process.
static DECLARATION_QUERY: std::sync::LazyLock<std::result::Result<Query, String>> =
    std::sync::LazyLock::new(|| {
        Query::new(&tree_sitter_rust::LANGUAGE.into(), DECLARATIONS)
            .map_err(|error| error.to_string())
    });

pub fn extract_rust(source: &str) -> Result<ParsedRust> {
    if source.len() > MAX_BLOB_BYTES {
        bail!("The source exceeds the 2 MiB analysis limit.");
    }
    let language = tree_sitter_rust::LANGUAGE.into();
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .context("Could not load the Rust grammar.")?;
    let tree = parser
        .parse(source, None)
        .context("Rust parsing was interrupted.")?;
    let query = DECLARATION_QUERY
        .as_ref()
        .map_err(|error| anyhow::anyhow!("The declaration query is invalid: {error}"))?;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), source.as_bytes());
    let mut symbols = Vec::new();
    while let Some(found) = matches.next() {
        let declaration = found
            .captures()
            .iter()
            .find(|capture| query.capture_names()[capture.index as usize] == "symbol")
            .context("The declaration capture is missing.")?
            .node;
        let name = found
            .captures()
            .iter()
            .find(|capture| query.capture_names()[capture.index as usize] == "name")
            .context("The name capture is missing.")?
            .node;
        let (context, owner) = declaration_context(declaration, source)?;
        let kind = match declaration.kind() {
            "struct_item" => SymbolKind::Struct,
            "enum_item" => SymbolKind::Enum,
            "trait_item" => SymbolKind::Trait,
            _ if owner == Some("trait_item") => SymbolKind::TraitMethod,
            _ if owner == Some("impl_item") => SymbolKind::Method,
            _ => SymbolKind::Function,
        };
        let body = declaration.child_by_field_name("body");
        let signature_end = body.map_or(declaration.end_byte(), |node| node.start_byte());
        symbols.push(Symbol {
            name: name.utf8_text(source.as_bytes())?.to_owned(),
            kind,
            context,
            start_byte: declaration.start_byte(),
            end_byte: declaration.end_byte(),
            line: declaration.start_position().row + 1,
            has_error: declaration.has_error(),
            signature_range: declaration.start_byte()..signature_end,
            body_range: body.map(|node| node.byte_range()),
            fingerprints: fingerprints(declaration, name, body, signature_end, source),
        });
    }
    symbols.sort_by_key(|symbol| symbol.start_byte);
    let mut diagnostics = Vec::new();
    let mut unsupported = Vec::new();
    visit_nodes(tree.root_node(), |node, _depth| {
        if node.is_error() || node.is_missing() {
            diagnostics.push(ParseDiagnostic {
                kind: if node.is_missing() {
                    ParseDiagnosticKind::Missing
                } else {
                    ParseDiagnosticKind::Error
                },
                range: node.byte_range(),
                syntax_kind: node.kind().to_owned(),
            });
        }
        if matches!(
            node.kind(),
            "macro_invocation" | "macro_definition" | "attribute_item" | "inner_attribute_item"
        ) {
            unsupported.push(UnsupportedSyntax {
                range: node.byte_range(),
                syntax_kind: node.kind().to_owned(),
            });
        }
        true
    });
    Ok(ParsedRust::new(
        symbols,
        tree.root_node().has_error(),
        EXTRACTOR_VERSION,
        diagnostics,
        unsupported,
    ))
}

// Length-framed fields avoid collisions between different token boundaries.
fn hash_field(hasher: &mut blake3::Hasher, value: &[u8]) {
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value);
}

/// Compute every declaration fingerprint in one traversal. Each hash sees the
/// same node sequence a dedicated traversal of its scope would visit:
/// comments and literal internals are skipped for all of them, the signature
/// ignores nodes starting at or after the body, and the body hash covers only
/// the body subtree.
fn fingerprints(
    declaration: Node<'_>,
    name: Node<'_>,
    body: Option<Node<'_>>,
    signature_end: usize,
    source: &str,
) -> SymbolFingerprints {
    let start = || {
        let mut hash = blake3::Hasher::new();
        hash_field(&mut hash, b"git-scoop-fingerprint-v1");
        hash
    };
    let (mut tokens, mut structure, mut signature) = (start(), start(), start());
    let mut body_hash = body.map(|_| start());
    let mut body_depth = None;
    visit_nodes(declaration, |node, depth| {
        if body_depth.is_some_and(|body_depth| depth <= body_depth) {
            body_depth = None;
        }
        if body.is_some_and(|body| body.id() == node.id()) {
            body_depth = Some(depth);
        }
        if matches!(node.kind(), "line_comment" | "block_comment") {
            return false;
        }
        let in_signature = node.start_byte() < signature_end;
        // String and numeric literals are indivisible tokens: their internal
        // spaces and spelling must survive whitespace/comment normalization.
        let literal = matches!(
            node.kind(),
            "string_literal"
                | "raw_string_literal"
                | "char_literal"
                | "integer_literal"
                | "float_literal"
                | "boolean_literal"
        );
        let kind = node.kind().as_bytes();
        hash_field(&mut structure, &(depth as u64).to_le_bytes());
        hash_field(&mut structure, kind);
        if literal || node.child_count() == 0 {
            let text = &source.as_bytes()[node.byte_range()];
            let identifier = matches!(
                node.kind(),
                "identifier"
                    | "type_identifier"
                    | "field_identifier"
                    | "shorthand_field_identifier"
                    | "lifetime"
            );
            hash_field(&mut tokens, kind);
            hash_field(&mut tokens, text);
            hash_field(
                &mut structure,
                if identifier {
                    b"<identifier>".as_slice()
                } else {
                    text
                },
            );
            if in_signature {
                hash_field(&mut signature, kind);
                hash_field(
                    &mut signature,
                    if node.id() == name.id() {
                        b"<declaration>".as_slice()
                    } else {
                        text
                    },
                );
            }
            if body_depth.is_some()
                && let Some(hash) = body_hash.as_mut()
            {
                hash_field(hash, kind);
                hash_field(hash, text);
            }
        }
        !literal
    });
    SymbolFingerprints {
        raw: *blake3::hash(&source.as_bytes()[declaration.byte_range()]).as_bytes(),
        normalized_tokens: *tokens.finalize().as_bytes(),
        structural: *structure.finalize().as_bytes(),
        signature: *signature.finalize().as_bytes(),
        body: body_hash.map(|hash| *hash.finalize().as_bytes()),
    }
}

// Walk iteratively so deeply nested valid or malformed source cannot exhaust
// the Rust call stack. Returning false skips a node's children.
fn visit_nodes(root: Node<'_>, mut visit: impl FnMut(Node<'_>, usize) -> bool) {
    let mut cursor = root.walk();
    let mut depth = 0;
    loop {
        if visit(cursor.node(), depth) && cursor.goto_first_child() {
            depth += 1;
            continue;
        }
        loop {
            if depth == 0 {
                return;
            }
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return;
            }
            depth -= 1;
        }
    }
}

fn declaration_context(
    node: Node<'_>,
    source: &str,
) -> Result<(Vec<String>, Option<&'static str>)> {
    let mut context = Vec::new();
    let mut ancestor = node.parent();
    let mut owner = None;
    while let Some(parent) = ancestor {
        match parent.kind() {
            "mod_item" | "trait_item" | "function_item" => {
                if let Some(name) = parent.child_by_field_name("name") {
                    context.push(name.utf8_text(source.as_bytes())?.to_owned());
                }
                if owner.is_none() && parent.kind() != "mod_item" {
                    owner = Some(if parent.kind() == "trait_item" {
                        "trait_item"
                    } else {
                        "function_item"
                    });
                }
            }
            "impl_item" => {
                let target = parent
                    .child_by_field_name("type")
                    .context("The impl target type is missing.")?;
                let target_text = target.utf8_text(source.as_bytes())?;
                let label = if let Some(trait_node) = parent.child_by_field_name("trait") {
                    format!(
                        "impl {} for {}",
                        trait_node.utf8_text(source.as_bytes())?,
                        target_text
                    )
                } else {
                    format!("impl {target_text}")
                };
                context.push(label);
                if owner.is_none() {
                    owner = Some("impl_item");
                }
            }
            _ => {}
        }
        ancestor = parent.parent();
    }
    context.reverse();
    Ok((context, owner))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_declarations_keep_distinct_contexts() -> Result<()> {
        let source = "mod inner { struct A; enum E { One } trait T { fn run(&self); } impl A { fn run(&self) {} } fn free() {} }";
        let parsed = extract_rust(source)?;
        assert!(!parsed.has_error);
        assert_eq!(parsed.symbols.len(), 6);
        let methods: Vec<_> = parsed
            .symbols
            .iter()
            .filter(|symbol| symbol.name == "run")
            .collect();
        assert_eq!(methods[0].kind, SymbolKind::TraitMethod);
        assert_eq!(methods[1].kind, SymbolKind::Method);
        assert_ne!(methods[0].context, methods[1].context);
        for symbol in &parsed.symbols {
            assert!(source[symbol.start_byte..symbol.end_byte].contains(&symbol.name));
        }
        Ok(())
    }

    #[test]
    fn malformed_and_oversized_sources_are_not_silent_success() -> Result<()> {
        assert!(extract_rust("fn broken( {")?.has_error);
        assert!(extract_rust(&" ".repeat(MAX_BLOB_BYTES + 1)).is_err());
        Ok(())
    }

    #[test]
    fn nested_function_is_not_an_impl_method() -> Result<()> {
        let parsed = extract_rust("struct A; impl A { fn outer() { fn inner() {} } }")?;
        assert_eq!(
            parsed
                .symbols
                .iter()
                .find(|s| s.name == "inner")
                .unwrap()
                .kind,
            SymbolKind::Function
        );
        Ok(())
    }

    fn only_symbol(source: &str) -> Result<Symbol> {
        let mut parsed = extract_rust(source)?;
        assert!(!parsed.has_error);
        assert_eq!(parsed.symbols.len(), 1);
        Ok(parsed.symbols.remove(0))
    }

    #[test]
    fn whitespace_and_comments_preserve_token_fingerprints() -> Result<()> {
        let before = only_symbol("fn run(x: u32) -> u32 { x + 7 }")?;
        let after = only_symbol("fn run ( x : u32 ) -> u32 { /* ignored */ x + 7 // ignored\n }")?;
        assert_ne!(before.fingerprints.raw, after.fingerprints.raw);
        assert_eq!(
            before.fingerprints.normalized_tokens,
            after.fingerprints.normalized_tokens
        );
        assert_eq!(
            before.fingerprints.structural,
            after.fingerprints.structural
        );
        assert_eq!(before.fingerprints.signature, after.fingerprints.signature);
        assert_eq!(before.fingerprints.body, after.fingerprints.body);
        Ok(())
    }

    #[test]
    fn rename_signature_and_body_changes_have_separate_evidence() -> Result<()> {
        let original = only_symbol("fn run(x: u32) -> u32 { x + 7 }")?;
        let renamed = only_symbol("fn execute(x: u32) -> u32 { x + 7 }")?;
        let signature = only_symbol("fn run(x: u64) -> u32 { x + 7 }")?;
        let body = only_symbol("fn run(x: u32) -> u32 { x + 8 }")?;
        assert_ne!(
            original.fingerprints.normalized_tokens,
            renamed.fingerprints.normalized_tokens
        );
        assert_eq!(
            original.fingerprints.signature,
            renamed.fingerprints.signature
        );
        assert_eq!(
            original.fingerprints.structural,
            renamed.fingerprints.structural
        );
        assert_ne!(
            original.fingerprints.signature,
            signature.fingerprints.signature
        );
        assert_eq!(original.fingerprints.body, signature.fingerprints.body);
        assert_eq!(original.fingerprints.signature, body.fingerprints.signature);
        assert_ne!(original.fingerprints.body, body.fingerprints.body);
        assert_ne!(
            original.fingerprints.structural,
            body.fingerprints.structural
        );
        Ok(())
    }

    #[test]
    fn literal_contents_and_token_boundaries_are_preserved() -> Result<()> {
        let original = only_symbol("fn run() { let s = \"a b /* c */\"; }")?;
        let changed = only_symbol("fn run() { let s = \"ab /* c */\"; }")?;
        assert_ne!(
            original.fingerprints.normalized_tokens,
            changed.fingerprints.normalized_tokens
        );
        assert_ne!(
            original.fingerprints.structural,
            changed.fingerprints.structural
        );
        let split = only_symbol("fn run() { let _ = a + bc; }")?;
        let joined = only_symbol("fn run() { let _ = ab + c; }")?;
        assert_ne!(
            split.fingerprints.normalized_tokens,
            joined.fingerprints.normalized_tokens
        );
        Ok(())
    }

    #[test]
    fn declaration_ranges_versions_and_unexpanded_syntax_are_explicit() -> Result<()> {
        let source = "#[cfg(feature = \"x\")] trait Task { fn declared(&self); fn default(&self) {} }\nmacro_rules! generated { () => { fn not_expanded() {} }; }\ngenerated!();";
        let parsed = extract_rust(source)?;
        assert_eq!(parsed.version, EXTRACTOR_VERSION);
        assert!(
            parsed
                .unsupported
                .iter()
                .any(|s| s.syntax_kind == "attribute_item")
        );
        assert!(
            parsed
                .unsupported
                .iter()
                .any(|s| s.syntax_kind == "macro_definition")
        );
        assert!(
            parsed
                .unsupported
                .iter()
                .any(|s| s.syntax_kind == "macro_invocation")
        );
        assert!(!parsed.symbols.iter().any(|s| s.name == "not_expanded"));
        let declared = parsed
            .symbols
            .iter()
            .find(|s| s.name == "declared")
            .unwrap();
        let default = parsed.symbols.iter().find(|s| s.name == "default").unwrap();
        assert_eq!(declared.kind, SymbolKind::TraitMethod);
        assert_eq!(default.kind, SymbolKind::TraitMethod);
        assert!(declared.body_range.is_none());
        assert!(default.body_range.is_some());
        assert_eq!(
            &source[declared.signature_range.clone()],
            "fn declared(&self);"
        );
        assert_eq!(&source[default.body_range.clone().unwrap()], "{}");
        for symbol in &parsed.symbols {
            assert!(source.get(symbol.signature_range.clone()).is_some());
            assert!(
                symbol
                    .body_range
                    .as_ref()
                    .is_none_or(|range| source.get(range.clone()).is_some())
            );
        }
        Ok(())
    }

    #[test]
    fn error_and_missing_nodes_keep_original_byte_ranges() -> Result<()> {
        for source in [
            "fn broken( {",
            "fn missing() { let x = 1 }",
            "fn unclosed() {",
        ] {
            let parsed = extract_rust(source)?;
            assert!(parsed.has_error);
            assert!(!parsed.diagnostics.is_empty());
            for diagnostic in parsed.diagnostics {
                assert!(source.get(diagnostic.range).is_some());
            }
        }
        let missing = extract_rust("fn missing() { let x = 1 }")?;
        assert!(
            missing
                .diagnostics
                .iter()
                .any(|d| d.kind == ParseDiagnosticKind::Missing),
            "Expected a missing semicolon: {:?}",
            missing.diagnostics
        );
        Ok(())
    }
}
