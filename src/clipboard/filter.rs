//! Search and filtering rules shared by GUI and CLI.
//!
//! Filter rules:
//! 1. Query is trimmed and lower-cased once. Empty query matches everything.
//! 2. Tokenisation splits on Unicode whitespace and matches **all** tokens (AND, not OR).
//! 3. A protected row's preview is matched; the locked plaintext is not searched. The `locked`
//!    column lets a caller ask only the public label.
//! 4. Result preserves whatever order the caller passes in; callers feed the MRU order.

use super::items::ClipboardItem;

/// Return the indices of `items` matching `query`. Preserves caller order.
pub fn filter_indices(items: &[ClipboardItem], query: &str) -> Vec<usize> {
    let tokens: Vec<String> = query
        .split_whitespace()
        .map(|token| token.to_lowercase())
        .collect();

    items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            if matches(item, &tokens) {
                Some(index)
            } else {
                None
            }
        })
        .collect()
}

fn matches(item: &ClipboardItem, tokens: &[String]) -> bool {
    if tokens.is_empty() {
        return true;
    }
    if let Some(label) = &item.public_label {
        let haystack = label.to_lowercase();
        if tokens.iter().all(|token| haystack.contains(token)) {
            return true;
        }
    }
    match item.kind {
        super::items::ClipboardItemKind::Plain => {
            if let Some(preview) = &item.plain_preview {
                let haystack = preview.to_lowercase();
                if tokens.iter().all(|token| haystack.contains(token)) {
                    return true;
                }
            }
            let content = item.content.to_lowercase();
            tokens.iter().all(|token| content.contains(token))
        }
        super::items::ClipboardItemKind::Protected => {
            let Some(preview) = &item.plain_preview else {
                return false;
            };
            let haystack = preview.to_lowercase();
            tokens.iter().all(|token| haystack.contains(token))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard::items::{ClipboardItemKind, RetentionState};
    use chrono::Utc;

    fn item(id: i64, content: &str, preview: Option<&str>) -> ClipboardItem {
        let now = Utc::now().timestamp_millis();
        ClipboardItem {
            id,
            kind: ClipboardItemKind::Plain,
            content: content.to_string(),
            plain_preview: preview.map(str::to_owned),
            public_label: None,
            byte_size: content.len() as i64,
            created_at: now,
            last_copied_at: now,
            last_used_at: None,
            last_edited_at: None,
            retention: RetentionState::default(),
        }
    }

    #[test]
    fn empty_query_returns_everything() {
        let items = vec![item(1, "foo", None), item(2, "bar", None)];
        assert_eq!(filter_indices(&items, ""), vec![0, 1]);
    }

    #[test]
    fn single_token_match() {
        let items = vec![item(1, "foo", None), item(2, "bar", None)];
        assert_eq!(filter_indices(&items, "FOO"), vec![0]);
    }

    #[test]
    fn multiple_tokens_must_all_match() {
        let items = vec![
            item(1, "rust cargo test", None),
            item(2, "rust and ts", None),
            item(3, "cargo ts", None),
        ];
        let indices = filter_indices(&items, "rust cargo");
        assert_eq!(indices, vec![0]);
    }

    #[test]
    fn preview_short_circuit() {
        let items = vec![item(
            1,
            "full\nbody\nthat\u{1F4A9}\u{2603}long",
            Some("full body …"),
        )];
        let indices = filter_indices(&items, "body");
        assert_eq!(indices, vec![0]);
        // The full-body content has an emoji; the lowercase preview path does the matching.
        let indices = filter_indices(&items, "emoji");
        assert_eq!(indices, Vec::<usize>::new());
    }
}
