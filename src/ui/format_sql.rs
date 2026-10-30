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
        let database_type = self.tab_database_type(tab.saved.profile);
        let editor = tab.input.clone();
        let text = editor.read(cx).value().to_string();
        let selected = editor.read(cx).selected_range();
        let range = if selected.is_empty() {
            sql::statement_range_at_for(&text, selected.start, database_type)
        } else {
            Some(selected.clone())
        };
        let Some(range) = range else {
            format_notice(
                "Move the cursor into a SQL statement or select SQL to format.",
                window,
                cx,
            );
            return;
        };
        let mut replacement = text[range.clone()].to_string();
        let mut unsupported = false;
        let mut rejected = false;
        for statement in sql::statement_ranges_for(&replacement, database_type)
            .into_iter()
            .rev()
        {
            match sql::format_statement_for(
                &replacement[statement.clone()],
                self.settings.sql_style(),
                sql::FormatMode::Manual,
                database_type,
            ) {
                sql::FormatOutcome::Formatted(formatted) => {
                    replacement.replace_range(statement, &formatted)
                }
                sql::FormatOutcome::Unsupported => unsupported = true,
                sql::FormatOutcome::Rejected => rejected = true,
                _ => {}
            }
        }
        if sql::statement_ranges_for(&replacement, database_type).is_empty() {
            unsupported = true;
        }
        if replacement != text[range.clone()] {
            let end = range.start + replacement.len();
            let mut formatted = text.clone();
            formatted.replace_range(range.clone(), &replacement);
            // A selection can begin inside a quoted token. Check the surrounding
            // document too, so formatting cannot change a literal or identifier.
            if !sql::same_tokens_for(&text, &formatted, database_type) {
                format_notice("Some SQL could not be formatted safely.", window, cx);
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
            format_notice("Some SQL could not be formatted safely.", window, cx);
        } else if unsupported {
            format_notice(
                "Some SQL statements are not supported by the formatter.",
                window,
                cx,
            );
        }
    }
}

fn format_notice(message: &'static str, window: &mut Window, cx: &mut App) {
    use gpui_kit::component::notification::{Notification, NotificationType};
    window.push_notification(
        Notification::new()
            .with_type(NotificationType::Warning)
            .id::<FormatSql>()
            .content(move |_, _, _| {
                div()
                    .id("format-sql-notice")
                    .test_support()
                    .role(Role::Alert)
                    .aria_label(message)
                    .text_sm()
                    .child(message)
                    .into_any_element()
            }),
        cx,
    );
}

/// Keep editor commands in the native right-click menu, with clipboard commands below.
pub(super) fn editor_menu(
    capabilities: gpui_kit::base::input::InputContextMenuCapabilities,
    has_clipboard: bool,
) -> Menu {
    use gpui_kit::component::input::{
        Copy, Cut, GoToDefinition, Paste, SelectAll, ToggleCodeActions,
    };
    let enabled = !capabilities.is_disabled();
    let editable = capabilities.is_editable();
    Menu {
        name: "SQL Editor".into(),
        disabled: false,
        items: vec![
            MenuItem::action("Go to Definition", GoToDefinition)
                .disabled(!(enabled && capabilities.has_definition())),
            MenuItem::action("Show Code Actions", ToggleCodeActions)
                .disabled(!(editable && capabilities.has_code_actions())),
            MenuItem::action("Format", FormatSql).disabled(!editable),
            MenuItem::separator(),
            MenuItem::action("Cut", Cut).disabled(!(editable && capabilities.is_copyable())),
            MenuItem::action("Copy", Copy).disabled(!capabilities.is_copyable()),
            MenuItem::action("Paste", Paste).disabled(!(editable && has_clipboard)),
            MenuItem::separator(),
            MenuItem::action("Select All", SelectAll),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::editor_menu;
    use gpui_kit::{MenuItem, base::input::InputContextMenuCapabilities};

    #[test]
    fn editor_menu_orders_format_before_clipboard_commands_and_obeys_editability() {
        for (capabilities, clipboard, expected) in [
            (
                InputContextMenuCapabilities::new(),
                false,
                [true, false, false, false],
            ),
            (
                InputContextMenuCapabilities::new().selection(true),
                true,
                [true, true, true, true],
            ),
            (
                InputContextMenuCapabilities::new()
                    .selection(true)
                    .readonly(true),
                true,
                [false, false, true, false],
            ),
            (
                InputContextMenuCapabilities::new().disabled(true),
                true,
                [false, false, false, false],
            ),
        ] {
            let menu = editor_menu(capabilities, clipboard);
            let entries: Vec<_> = menu
                .items
                .iter()
                .map(|item| match item {
                    MenuItem::Action { name, disabled, .. } => (name.as_ref(), !disabled),
                    MenuItem::Separator => ("---", false),
                    _ => panic!("Unexpected editor menu entry"),
                })
                .collect();
            assert_eq!(
                entries.iter().map(|(label, _)| *label).collect::<Vec<_>>(),
                [
                    "Go to Definition",
                    "Show Code Actions",
                    "Format",
                    "---",
                    "Cut",
                    "Copy",
                    "Paste",
                    "---",
                    "Select All"
                ]
            );
            assert_eq!(
                [entries[2].1, entries[4].1, entries[5].1, entries[6].1],
                expected
            );
            let MenuItem::Action { action, .. } = &menu.items[2] else {
                unreachable!()
            };
            assert!(action.as_any().is::<crate::ui::FormatSql>());
        }
    }
}
