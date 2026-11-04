use crate::sql::StatementIndex;
use gpui_kit::component::{Rope, input::EditorState};
use gpui_kit::{Context, Entity, Subscription};

/// Tints the statement that Run uses when nothing is selected, and keeps the
/// tint on that statement while the cursor moves and the SQL changes.
pub(super) fn track<T: 'static>(input: &Entity<EditorState>, cx: &mut Context<T>) -> Subscription {
    let mut index: Option<(Rope, String, StatementIndex)> = None;
    let subscription = cx.observe(input, move |_, input, cx| {
        let target = {
            let state = input.read(cx);
            let selection = state.selected_range();
            if !selection.is_empty() {
                None
            } else {
                if index.as_ref().is_none_or(|(text, ..)| text != state.text()) {
                    let sql = state.text().to_string();
                    let statements = StatementIndex::new(&sql);
                    index = Some((state.text().clone(), sql, statements));
                }
                let (_, sql, statements) = index.as_ref().expect("the index was built");
                statements.run_highlight(sql, selection.start)
            }
        };
        if input.read(cx).emphasized_range() != target {
            input.update(cx, |state, cx| state.set_emphasized_range(target, cx));
        }
    });
    // The observer sets the first tint.
    input.update(cx, |_, cx| cx.notify());
    subscription
}
