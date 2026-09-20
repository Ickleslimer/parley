use super::types::EXCERPT_LIMIT;

pub(crate) fn bound_excerpt(text: &str) -> String {
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index >= EXCERPT_LIMIT {
            break;
        }
        out.push(ch);
    }
    out
}

pub(crate) fn excerpt(content: &str, is_request: bool) -> (String, bool) {
    if is_request {
        if let Some(remainder) = task_remainder(content) {
            return (bound_excerpt(remainder), true);
        }
    }
    (bound_excerpt(content), false)
}

fn task_remainder(content: &str) -> Option<&str> {
    for line in content.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("task:") {
            return Some(rest);
        }
    }
    None
}

pub(crate) fn window_from(content: &str, char_offset: usize) -> String {
    content
        .chars()
        .skip(char_offset)
        .take(EXCERPT_LIMIT)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_engine::types::EXCERPT_LIMIT;

    #[test]
    fn extracts_exact_task_remainder_and_bounds_prefix() {
        let prompt = "policy:\n- AGENTS.md\n  task: Implement the engine\nextra";
        let (text, extracted) = excerpt(prompt, true);
        assert!(extracted);
        assert_eq!(text, " Implement the engine");

        let long: String = "n".repeat(EXCERPT_LIMIT + 40);
        let (prefix, extracted) = excerpt(&long, false);
        assert!(!extracted);
        assert_eq!(prefix.chars().count(), EXCERPT_LIMIT);
        assert!(long.starts_with(&prefix));
    }

    #[test]
    fn responses_do_not_extract_task_lines() {
        let body = "task: should stay a prefix only";
        let (text, extracted) = excerpt(body, false);
        assert!(!extracted);
        assert_eq!(text, body);
    }
}
