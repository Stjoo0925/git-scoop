//! Lightweight Rust token coloring for source views. It colors lexical
//! categories only; it never interprets code or affects lineage.

use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    Plain,
    Keyword,
    Type,
    Function,
    Macro,
    String,
    Number,
    Comment,
    Lifetime,
}

const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while",
];

const PRIMITIVES: &[&str] = &[
    "bool", "char", "f32", "f64", "i8", "i16", "i32", "i64", "i128", "isize", "str", "u8", "u16",
    "u32", "u64", "u128", "usize",
];

/// Tokenizer state carried between lines of one source view.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct State {
    block_comment_depth: u32,
}

/// Split one line into colored byte ranges, continuing block comments from
/// earlier lines. Ranges cover the whole line in order.
pub fn line(text: &str, state: &mut State) -> Vec<(Range<usize>, Token)> {
    let bytes = text.as_bytes();
    let mut spans: Vec<(Range<usize>, Token)> = Vec::new();
    let mut push = |range: Range<usize>, token: Token| {
        if range.is_empty() {
            return;
        }
        match spans.last_mut() {
            Some((last, kind)) if *kind == token && last.end == range.start => last.end = range.end,
            _ => spans.push((range, token)),
        }
    };
    let mut index = 0;
    while index < bytes.len() {
        let start = index;
        if state.block_comment_depth > 0 {
            while index < bytes.len() {
                if bytes[index..].starts_with(b"*/") {
                    index += 2;
                    state.block_comment_depth -= 1;
                    if state.block_comment_depth == 0 {
                        break;
                    }
                } else if bytes[index..].starts_with(b"/*") {
                    index += 2;
                    state.block_comment_depth += 1;
                } else {
                    index += 1;
                }
            }
            push(start..index, Token::Comment);
            continue;
        }
        let byte = bytes[index];
        if bytes[index..].starts_with(b"//") {
            push(start..bytes.len(), Token::Comment);
            break;
        }
        if bytes[index..].starts_with(b"/*") {
            state.block_comment_depth = 1;
            index += 2;
            continue_comment(bytes, &mut index, state);
            push(start..index, Token::Comment);
            continue;
        }
        if byte == b'"' || (byte == b'b' && bytes.get(index + 1) == Some(&b'"')) {
            index += if byte == b'b' { 2 } else { 1 };
            while index < bytes.len() {
                match bytes[index] {
                    b'\\' => index += 2,
                    b'"' => {
                        index += 1;
                        break;
                    }
                    _ => index += 1,
                }
            }
            push(start..index.min(bytes.len()), Token::String);
            index = index.min(bytes.len());
            continue;
        }
        if byte == b'\'' {
            // A character literal closes within a few bytes; otherwise this
            // is a lifetime or label.
            let close = text[index + 1..]
                .char_indices()
                .take(3)
                .find(|(offset, character)| *character == '\'' && *offset > 0)
                .map(|(offset, _)| index + 1 + offset + 1);
            let escaped = bytes.get(index + 1) == Some(&b'\\');
            if let Some(end) = close.filter(|_| !escaped) {
                push(start..end, Token::String);
                index = end;
            } else if escaped {
                let end = text[index + 1..]
                    .find('\'')
                    .map_or(bytes.len(), |offset| index + 1 + offset + 1);
                push(start..end, Token::String);
                index = end;
            } else {
                index += 1;
                while index < bytes.len() && is_identifier(bytes[index]) {
                    index += 1;
                }
                push(start..index, Token::Lifetime);
            }
            continue;
        }
        if byte.is_ascii_digit() {
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric()
                    || bytes[index] == b'_'
                    || bytes[index] == b'.')
            {
                // Stop before a method call such as `1.max(2)` or a range.
                if bytes[index] == b'.' && !bytes.get(index + 1).is_some_and(u8::is_ascii_digit) {
                    break;
                }
                index += 1;
            }
            push(start..index, Token::Number);
            continue;
        }
        if is_identifier_start(text, index) {
            while index < bytes.len() && is_identifier_continue(text, index) {
                index += text[index..].chars().next().map_or(1, char::len_utf8);
            }
            let word = &text[start..index];
            let token = if bytes.get(index) == Some(&b'!') && !KEYWORDS.contains(&word) {
                index += 1;
                Token::Macro
            } else if KEYWORDS.contains(&word) {
                Token::Keyword
            } else if PRIMITIVES.contains(&word)
                || word.chars().next().is_some_and(char::is_uppercase)
            {
                Token::Type
            } else if text[index..].trim_start().starts_with('(') {
                Token::Function
            } else {
                Token::Plain
            };
            push(start..index, token);
            continue;
        }
        index += text[index..].chars().next().map_or(1, char::len_utf8);
        push(start..index, Token::Plain);
    }
    spans
}

fn continue_comment(bytes: &[u8], index: &mut usize, state: &mut State) {
    while *index < bytes.len() && state.block_comment_depth > 0 {
        if bytes[*index..].starts_with(b"*/") {
            *index += 2;
            state.block_comment_depth -= 1;
        } else if bytes[*index..].starts_with(b"/*") {
            *index += 2;
            state.block_comment_depth += 1;
        } else {
            *index += 1;
        }
    }
}

fn is_identifier(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn is_identifier_start(text: &str, index: usize) -> bool {
    text[index..]
        .chars()
        .next()
        .is_some_and(|character| character == '_' || character.is_alphabetic())
}

fn is_identifier_continue(text: &str, index: usize) -> bool {
    text[index..]
        .chars()
        .next()
        .is_some_and(|character| character == '_' || character.is_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colored(text: &str, state: &mut State) -> Vec<(String, Token)> {
        line(text, state)
            .into_iter()
            .filter(|(_, token)| *token != Token::Plain)
            .map(|(range, token)| (text[range].to_owned(), token))
            .collect()
    }

    fn owned(expected: &[(&str, Token)]) -> Vec<(String, Token)> {
        expected
            .iter()
            .map(|(text, token)| ((*text).to_owned(), *token))
            .collect()
    }

    #[test]
    fn colors_common_rust_tokens() {
        let mut state = State::default();
        assert_eq!(
            colored(
                "pub fn sum<'a>(x: &'a [u64]) -> Vec<u64> { println!(\"{x:?}\"); 0x1f + 'c' as u64 // done",
                &mut state
            ),
            owned(&[
                ("pub", Token::Keyword),
                ("fn", Token::Keyword),
                ("'a", Token::Lifetime),
                ("'a", Token::Lifetime),
                ("u64", Token::Type),
                ("Vec", Token::Type),
                ("u64", Token::Type),
                ("println!", Token::Macro),
                ("\"{x:?}\"", Token::String),
                ("0x1f", Token::Number),
                ("'c'", Token::String),
                ("as", Token::Keyword),
                ("u64", Token::Type),
                ("// done", Token::Comment),
            ])
        );
        assert_eq!(
            colored("x.0.max(1..2)", &mut State::default()),
            owned(&[
                ("0", Token::Number),
                ("max", Token::Function),
                ("1", Token::Number),
                ("2", Token::Number),
            ])
        );
    }

    #[test]
    fn spans_cover_the_line_and_block_comments_continue() {
        let mut state = State::default();
        let text = "let 합계 = \"a\\\"b\"; /* open";
        let spans = line(text, &mut state);
        assert_eq!(spans.first().map(|(range, _)| range.start), Some(0));
        assert_eq!(spans.last().map(|(range, _)| range.end), Some(text.len()));
        assert!(
            spans
                .windows(2)
                .all(|pair| pair[0].0.end == pair[1].0.start)
        );
        assert!(
            spans
                .iter()
                .any(|(range, token)| *token == Token::String
                    && &text[range.clone()] == "\"a\\\"b\"")
        );
        assert_ne!(state, State::default());
        assert_eq!(
            colored("still /* nested */ comment */ fn after()", &mut state),
            owned(&[
                ("still /* nested */ comment */", Token::Comment),
                ("fn", Token::Keyword),
                ("after", Token::Function),
            ])
        );
        assert_eq!(state, State::default());
    }
}
