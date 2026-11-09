use crate::sql::StatementIndex;
use gpui_kit::component::{Rope, input::EditorState};
use gpui_kit::{AppContext as _, Context, Entity, Subscription, Task};
use std::{cell::RefCell, ops::Range, rc::Rc};

/// SQL up to this size indexes its statements on the UI thread. Larger SQL
/// indexes on a background thread, and the tint follows when it ends.
const BACKGROUND_INDEX_BYTES: usize = 64 * 1024;

/// The statements of one text of the editor.
struct Indexed {
    text: Rope,
    sql: String,
    statements: StatementIndex,
}

impl Indexed {
    fn new(text: Rope) -> Self {
        let sql = text.to_string();
        let statements = StatementIndex::new(&sql);
        Self {
            text,
            sql,
            statements,
        }
    }

    fn run_highlight(&self, cursor: usize) -> Option<Range<usize>> {
        self.statements.run_highlight(&self.sql, cursor)
    }
}

/// Tints the statement that Run uses when nothing is selected, and keeps the
/// tint on that statement while the cursor moves and the SQL changes.
pub(super) fn track<T: 'static>(input: &Entity<EditorState>, cx: &mut Context<T>) -> Subscription {
    let index: Rc<RefCell<Option<Indexed>>> = Rc::default();
    // The background index of large SQL, with its text. An edit replaces
    // it; a cursor move or a caret blink waits for it.
    let pending: RefCell<Option<(Rope, Task<()>)>> = RefCell::default();
    let subscription = cx.observe(input, move |_, input, cx| {
        let (text, selection) = {
            let state = input.read(cx);
            (state.text().clone(), state.selected_range())
        };
        let target = if !selection.is_empty() {
            None
        } else {
            let current = index
                .borrow()
                .as_ref()
                .is_some_and(|indexed| indexed.text == text);
            if !current && text.len() > BACKGROUND_INDEX_BYTES {
                // The tint stays until the index of this text is ready. The
                // editor then notifies, and this observer runs again.
                let mut pending = pending.borrow_mut();
                if pending
                    .as_ref()
                    .is_some_and(|(indexing, _)| *indexing == text)
                {
                    return;
                }
                let index = index.clone();
                let input = input.downgrade();
                let indexing = text.clone();
                let task = cx.spawn(async move |_, cx| {
                    let indexed = cx.background_spawn(async move { Indexed::new(text) }).await;
                    *index.borrow_mut() = Some(indexed);
                    let _ = input.update(cx, |_, cx| cx.notify());
                });
                *pending = Some((indexing, task));
                return;
            }
            if !current {
                *index.borrow_mut() = Some(Indexed::new(text));
            }
            index
                .borrow()
                .as_ref()
                .and_then(|indexed| indexed.run_highlight(selection.start))
        };
        if input.read(cx).emphasized_range() != target {
            input.update(cx, |state, cx| state.set_emphasized_range(target, cx));
        }
    });
    // The observer sets the first tint.
    input.update(cx, |_, cx| cx.notify());
    subscription
}
