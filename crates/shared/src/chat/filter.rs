//! A basic word filter that masks blocked words with `*`.
//!
//! Matching is per word, case-insensitive, and folds common "1337 speak" substitutions and
//! repeated letters, so `D4RNNN` still matches `darn`. It's deliberately simple: good enough
//! for a first milestone, and easy to replace with something smarter later.

use std::collections::HashSet;

#[derive(Debug, Clone, Default)]
pub struct WordFilter {
    blocked: HashSet<String>,
}

impl WordFilter {
    pub fn new<I, S>(words: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let blocked = words
            .into_iter()
            .map(|w| squeeze(&fold(w.as_ref())))
            .filter(|w| !w.is_empty())
            .collect();
        Self { blocked }
    }

    pub fn is_empty(&self) -> bool {
        self.blocked.is_empty()
    }

    /// Returns `text` with every blocked word replaced by asterisks of the same length.
    pub fn apply(&self, text: &str) -> String {
        if self.blocked.is_empty() {
            return text.to_string();
        }
        let mut out = String::with_capacity(text.len());
        let mut word = String::new();
        for c in text.chars() {
            if is_word_char(c) {
                word.push(c);
            } else {
                self.flush(&mut word, &mut out);
                out.push(c);
            }
        }
        self.flush(&mut word, &mut out);
        out
    }

    fn flush(&self, word: &mut String, out: &mut String) {
        if word.is_empty() {
            return;
        }
        if self.blocked.contains(&squeeze(&fold(word))) {
            out.extend(std::iter::repeat_n('*', word.chars().count()));
        } else {
            out.push_str(word);
        }
        word.clear();
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '@' | '$')
}

/// Lowercases and undoes common character substitutions.
fn fold(s: &str) -> String {
    s.chars()
        .map(|c| match c.to_ascii_lowercase() {
            '0' => 'o',
            '1' | '!' => 'i',
            '3' => 'e',
            '4' | '@' => 'a',
            '5' | '$' => 's',
            '7' => 't',
            c => c,
        })
        .collect()
}

/// Collapses repeated letters: "daaarn" -> "darn".
fn squeeze(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if !out.ends_with(c) {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter() -> WordFilter {
        WordFilter::new(["darn", "heck"])
    }

    #[test]
    fn masks_blocked_words() {
        assert_eq!(filter().apply("well darn it"), "well **** it");
        assert_eq!(filter().apply("HECK, no!"), "****, no!");
    }

    #[test]
    fn catches_simple_evasion() {
        assert_eq!(filter().apply("d4rn"), "****");
        assert_eq!(filter().apply("daaaarnnn"), "*********");
        assert_eq!(filter().apply("h3ck"), "****");
    }

    #[test]
    fn leaves_innocent_words() {
        // Whole-word matching: no "Scunthorpe problem" for words that merely contain a blocked one.
        assert_eq!(filter().apply("checking the deck"), "checking the deck");
        assert_eq!(filter().apply("darned socks"), "darned socks");
    }

    #[test]
    fn empty_filter_is_noop() {
        assert_eq!(WordFilter::default().apply("anything goes"), "anything goes");
    }
}
