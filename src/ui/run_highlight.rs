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

/// The statements of the editor text, and the background index of newer
/// large SQL.
#[derive(Default)]
struct State {
    index: Option<Indexed>,
    /// The text that indexes on a background thread, with its task. An edit
    /// replaces it; a cursor move or a caret blink waits for it.
    indexing: Option<(Rope, Task<()>)>,
}

/// Tints the statement that Run uses when nothing is selected, and keeps the
/// tint on that statement while the cursor moves and the SQL changes.
pub(super) fn track<T: 'static>(input: &Entity<EditorState>, cx: &mut Context<T>) -> Subscription {
    let state: Rc<RefCell<State>> = Rc::default();
    let subscription = cx.observe(input, move |_, input, cx| {
        let (text, selection) = {
            let editor = input.read(cx);
            (editor.text().clone(), editor.selected_range())
        };
        let target = if !selection.is_empty() {
            None
        } else {
            let current = state
                .borrow()
                .index
                .as_ref()
                .is_some_and(|indexed| indexed.text == text);
            if !current && text.len() > BACKGROUND_INDEX_BYTES {
                // The tint stays until the index of this text is ready. The
                // editor then notifies, and this observer runs again.
                let mut pending = state.borrow_mut();
                if pending
                    .indexing
                    .as_ref()
                    .is_some_and(|(indexing, _)| *indexing == text)
                {
                    return;
                }
                let finished = state.clone();
                let input = input.downgrade();
                let indexing = text.clone();
                let task = cx.spawn(async move |_, cx| {
                    let indexed = cx.background_spawn(async move { Indexed::new(text) }).await;
                    {
                        let mut state = finished.borrow_mut();
                        if state
                            .indexing
                            .as_ref()
                            .is_some_and(|(indexing, _)| *indexing == indexed.text)
                            && let Some((_, task)) = state.indexing.take()
                        {
                            // This task runs now, so it must not cancel itself.
                            task.detach();
                        }
                        state.index = Some(indexed);
                    }
                    let _ = input.update(cx, |_, cx| cx.notify());
                });
                pending.indexing = Some((indexing, task));
                return;
            }
            let mut state = state.borrow_mut();
            if !current {
                state.indexing = None;
                state.index = Some(Indexed::new(text));
            }
            state
                .index
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
