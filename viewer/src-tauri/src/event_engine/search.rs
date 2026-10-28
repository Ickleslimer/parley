use std::collections::VecDeque;

#[derive(Debug)]
pub(crate) struct SearchPattern {
    characters: Vec<char>,
    prefixes: Vec<usize>,
}

impl SearchPattern {
    pub(crate) fn new(query: &str) -> Self {
        let characters: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
        let mut prefixes = vec![0; characters.len()];
        let mut matched = 0;
        for position in 1..characters.len() {
            while matched > 0 && characters[position] != characters[matched] {
                matched = prefixes[matched - 1];
            }
            if characters[position] == characters[matched] {
                matched += 1;
            }
            prefixes[position] = matched;
        }
        Self {
            characters,
            prefixes,
        }
    }

    pub(crate) fn find(&self, text: &str) -> Option<usize> {
        if self.characters.is_empty() {
            return None;
        }
        let mut matched = 0;
        let mut origins = VecDeque::with_capacity(self.characters.len());
        for (character_index, character) in text.chars().enumerate() {
            for (fold_index, folded) in character.to_lowercase().enumerate() {
                if origins.len() == self.characters.len() {
                    origins.pop_front();
                }
                origins.push_back((character_index, fold_index == 0));
                while matched > 0 && folded != self.characters[matched] {
                    matched = self.prefixes[matched - 1];
                }
                if folded == self.characters[matched] {
                    matched += 1;
                }
                if matched == self.characters.len() {
                    if let Some(&(offset, true)) = origins.front() {
                        return Some(offset);
                    }
                    matched = self.prefixes[matched - 1];
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::SearchPattern;

    #[test]
    fn matches_case_and_reports_original_unicode_offsets() {
        assert_eq!(SearchPattern::new("ME").find("🧬λ Some message"), Some(5));
        assert_eq!(SearchPattern::new("λ").find("xΛy"), Some(1));
        assert_eq!(SearchPattern::new("k").find("🧬Kelvin"), Some(1));
        assert_eq!(SearchPattern::new("i\u{307}").find("xİy"), Some(1));
        assert_eq!(SearchPattern::new("\u{307}").find("İ\u{307}"), Some(1));
        assert_eq!(SearchPattern::new("\u{307}").find("İ"), None);
        assert_eq!(SearchPattern::new("").find("anything"), None);
    }

    #[test]
    fn keeps_scalar_lowercase_distinct_from_contextual_casing_and_full_case_folding() {
        assert_eq!("Σ".to_lowercase(), "σ");
        assert_eq!("ΟΣ".to_lowercase(), "ος");
        assert_eq!(SearchPattern::new("Σ").find("ΟΣ"), Some(1));
        assert_eq!(SearchPattern::new("ος").find("ΟΣ"), None);
        assert_eq!(SearchPattern::new("οσ").find("ΟΣ"), Some(0));
        assert_eq!(SearchPattern::new("ss").find("ß"), None);
        assert_eq!(SearchPattern::new("ß").find("ẞ"), Some(0));
    }

    #[test]
    fn handles_repeated_prefixes_and_large_unicode_bodies() {
        let mut body = format!("🧬{}", "a".repeat(1_000_000));
        body.push_str("ME");
        assert_eq!(SearchPattern::new("me").find(&body), Some(1_000_001));
        assert_eq!(SearchPattern::new("unmatched").find(&body), None);
        let mut repeated = format!("λ{}", "a".repeat(100_000));
        repeated.push('b');
        let query = format!("{}b", "a".repeat(10_000));
        assert_eq!(SearchPattern::new(&query).find(&repeated), Some(90_001));
    }
}
