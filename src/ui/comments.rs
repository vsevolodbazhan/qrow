use super::*;
use std::ops::Range;

impl Qrow {
    pub(super) fn toggle_comment(
        &mut self,
        _: &ToggleComment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.form.is_some() {
            return;
        }
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let text = tab.input.read(cx).value().to_string();
        let selection = tab.input.read(cx).selected_range();
        let (replacement, selection) = toggle_lines(&text, selection);
        if replacement == text {
            return;
        }
        tab.input.update(cx, |editor, cx| {
            let scroll = editor.scroll_offset();
            editor.replace_all(replacement, window, cx);
            editor.set_selected_range(selection, cx);
            editor.set_scroll_offset(scroll, cx);
        });
    }
}

/// A selection ending at a line start does not include that line.
fn toggle_lines(text: &str, selection: Range<usize>) -> (String, Range<usize>) {
    let start = text[..selection.start].rfind('\n').map_or(0, |ix| ix + 1);
    let end = if !selection.is_empty() && text[..selection.end].ends_with('\n') {
        selection.end - 1
    } else {
        selection.end
    };
    let end = text[end..].find('\n').map_or(text.len(), |ix| end + ix);
    let mut lines = Vec::new();
    let mut offset = start;
    for line in text[start..end].split('\n') {
        let content = line.trim_start_matches([' ', '\t']);
        if !content.trim().is_empty() {
            lines.push((offset + line.len() - content.len(), content));
        }
        offset += line.len() + 1;
    }
    let uncomment = lines.iter().all(|(_, content)| content.starts_with("--"));
    let mut result = text.to_owned();
    let mut selected = selection;
    // Reverse order keeps every edit in the original byte coordinates.
    for (offset, content) in lines.into_iter().rev() {
        let removed = if uncomment {
            if content.starts_with("-- ") { 3 } else { 2 }
        } else {
            0
        };
        let inserted = if uncomment { "" } else { "-- " };
        result.replace_range(offset..offset + removed, inserted);
        let map = |position: usize| {
            if position < offset {
                position
            } else {
                position.saturating_sub(removed).max(offset) + inserted.len()
            }
        };
        selected = map(selected.start)..map(selected.end);
    }
    (result, selected)
}

#[cfg(test)]
mod tests {
    use super::toggle_lines;

    #[test]
    fn toggle_line_comments_preserves_text_and_selection() {
        for text in [
            "  SELECT '日本語😀';\n\n\tFROM t;\n",
            "SELECT 1;\n-- existing\n  SELECT 2;",
            "  \r\n\t\n",
            "",
        ] {
            let (commented, selected) = toggle_lines(text, 0..text.len());
            let (restored, _) = toggle_lines(&commented, selected);
            assert_eq!(restored, text);
        }
    }

    #[test]
    fn line_boundaries_cursor_and_existing_comments() {
        assert_eq!(
            toggle_lines("one\ntwo\nthree", 1..4).0,
            "-- one\ntwo\nthree"
        );
        assert_eq!(toggle_lines("one\ntwo", 5..5), ("one\n-- two".into(), 8..8));
        assert_eq!(toggle_lines(" --one\n\t-- two", 0..14).0, " one\n\ttwo");
        assert_eq!(toggle_lines("one\n", 4..4).0, "one\n");
    }
}
