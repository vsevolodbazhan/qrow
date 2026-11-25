use crate::{model::DatabaseType, sql::StatementIndex};
use gpui_kit::component::{Rope, input::EditorState};
use gpui_kit::{AppContext as _, Context, Entity, Subscription, Task};
use std::{cell::RefCell, ops::Range, rc::Rc};
use uuid::Uuid;

/// SQL up to this size indexes its statements on the UI thread. Larger SQL
/// indexes on a background thread, and the tint follows when it ends.
const BACKGROUND_INDEX_BYTES: usize = 64 * 1024;

/// The statements of one text of the editor.
struct Indexed {
    text: Rope,
    database_type: DatabaseType,
    sql: String,
    statements: StatementIndex,
}

impl Indexed {
    fn new(text: Rope, database_type: DatabaseType) -> Self {
        let sql = text.to_string();
        let statements = StatementIndex::new_for(&sql, database_type);
        Self {
            text,
            database_type,
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
    /// The text and dialect that index on a background thread, with its task.
    /// An edit or dialect change replaces it; a cursor move or caret blink waits for it.
    indexing: Option<(Rope, DatabaseType, Task<()>)>,
}

/// Tints the statement that Run uses when nothing is selected, and keeps the
/// tint on that statement while the cursor moves, the SQL changes, or the
/// connection dialect changes.
pub(super) fn track(
    input: &Entity<EditorState>,
    tab_id: Uuid,
    initial_database_type: DatabaseType,
    cx: &mut Context<super::Qrow>,
) -> Subscription {
    let state: Rc<RefCell<State>> = Rc::default();
    let subscription = cx.observe(input, move |this, input, cx| {
        let database_type = this
            .tabs
            .iter()
            .find(|tab| tab.saved.id == tab_id)
            .map_or(initial_database_type, |tab| {
                this.tab_database_type(tab.saved.profile)
            });
        let (text, selection) = {
            let editor = input.read(cx);
            (editor.text().clone(), editor.selected_range())
        };
        let target = if !selection.is_empty() {
            None
        } else {
            let current = state.borrow().index.as_ref().is_some_and(|indexed| {
                indexed.text == text && indexed.database_type == database_type
            });
            if !current && text.len() > BACKGROUND_INDEX_BYTES {
                // The tint stays until the index of this text is ready. The
                // editor then notifies, and this observer runs again.
                let mut pending = state.borrow_mut();
                if pending
                    .indexing
                    .as_ref()
                    .is_some_and(|(indexing, dialect, _)| {
                        *indexing == text && *dialect == database_type
                    })
                {
                    return;
                }
                let finished = state.clone();
                let input = input.downgrade();
                let indexing = text.clone();
                let task = cx.spawn(async move |_, cx| {
                    let indexed = cx
                        .background_spawn(async move { Indexed::new(text, database_type) })
                        .await;
                    {
                        let mut state = finished.borrow_mut();
                        if state
                            .indexing
                            .as_ref()
                            .is_some_and(|(indexing, dialect, _)| {
                                *indexing == indexed.text && *dialect == indexed.database_type
                            })
                            && let Some((_, _, task)) = state.indexing.take()
                        {
                            // This task runs now, so it must not cancel itself.
                            task.detach();
                        }
                        state.index = Some(indexed);
                    }
                    let _ = input.update(cx, |_, cx| cx.notify());
                });
                pending.indexing = Some((indexing, database_type, task));
                return;
            }
            let mut state = state.borrow_mut();
            if !current {
                state.indexing = None;
                state.index = Some(Indexed::new(text, database_type));
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
