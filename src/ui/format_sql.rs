use super::*;

impl Qrow {
    pub(super) fn format_sql(
        &mut self,
        _: &FormatSql,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.form.is_some() || self.settings_open {
            return;
        }
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        if self.tab_database_type(tab.saved.profile) != crate::model::DatabaseType::Kyuubi {
            window.push_notification("Format SQL supports Spark SQL only.", cx);
            return;
        }
        let editor = tab.input.clone();
        let text = editor.read(cx).value().to_string();
        let selected = editor.read(cx).selected_range();
        let range = if selected.is_empty() {
            sql::statement_range_at(&text, selected.start)
        } else {
            Some(selected.clone())
        };
        let Some(range) = range else {
            window.push_notification(
                "Move the cursor into a SQL statement or select SQL to format.",
                cx,
            );
            return;
        };
        let mut replacement = text[range.clone()].to_string();
        let mut unsupported = false;
        let mut rejected = false;
        for statement in sql::statement_ranges(&replacement).into_iter().rev() {
            match sql::format_statement_with_mode(
                &replacement[statement.clone()],
                self.settings.sql_style(),
                sql::FormatMode::Manual,
            ) {
                sql::FormatOutcome::Formatted(formatted) => {
                    replacement.replace_range(statement, &formatted)
                }
                sql::FormatOutcome::Unsupported => unsupported = true,
                sql::FormatOutcome::Rejected => rejected = true,
                _ => {}
            }
        }
        if sql::statement_ranges(&replacement).is_empty() {
            unsupported = true;
        }
        if replacement != text[range.clone()] {
            let end = range.start + replacement.len();
            let mut formatted = text.clone();
            formatted.replace_range(range.clone(), &replacement);
            // A selection can begin inside a quoted token. Check the surrounding
            // document too, so formatting cannot change a literal or identifier.
            if !sql::same_tokens(&text, &formatted) {
                window.push_notification(
                    "Some SQL could not be formatted safely. That text was kept.",
                    cx,
                );
                return;
            }
            editor.update(cx, |editor, cx| {
                let scroll = editor.scroll_offset();
                editor.replace_all(formatted, window, cx);
                let selection = if selected.is_empty() {
                    range.start..range.start
                } else {
                    range.start..end
                };
                editor.set_selected_range(selection, cx);
                editor.set_scroll_offset(scroll, cx);
            });
        }
        if rejected {
            window.push_notification(
                "Some SQL could not be formatted safely. That text was kept.",
                cx,
            );
        } else if unsupported {
            window.push_notification(
                "Some SQL statements are not supported by the formatter. That text was kept.",
                cx,
            );
        }
    }
}
