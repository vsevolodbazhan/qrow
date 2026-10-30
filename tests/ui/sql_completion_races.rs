//! Delayed provider fixtures exercise the real editor's overlay lifecycle.
use gpui_kit::base::input::lsp_types as lsp;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, App, AppContext, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement, Render, Styled, Task, TestAppContext, TestSupportExt as _, Window,
    component::{
        Root,
        input::{CompletionNote, CompletionProvider, Editor, EditorState, Rope},
    },
    div, px, size,
};
use std::{cell::RefCell, rc::Rc};

type Gates = Rc<RefCell<Vec<async_channel::Sender<Vec<lsp::CompletionItem>>>>>;
struct Provider {
    gates: Gates,
    note: Rc<RefCell<Option<CompletionNote>>>,
}
impl CompletionProvider for Provider {
    fn completions(
        &self,
        _: &Rope,
        _: usize,
        _: lsp::CompletionContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<lsp::CompletionResponse>> {
        let (send, receive) = async_channel::bounded(1);
        self.gates.borrow_mut().push(send);
        let note = self.note.clone();
        cx.spawn(async move |_| {
            let items = receive.recv().await?;
            *note.borrow_mut() = Some(CompletionNote::new("Asynchronous catalog note"));
            Ok(lsp::CompletionResponse::Array(items))
        })
    }
    fn completion_note(&self) -> Option<CompletionNote> {
        self.note.borrow().clone()
    }
    fn is_completion_trigger(&self, _: usize, text: &str, _: &mut App) -> bool {
        text.chars().count() == 1 && text.chars().all(char::is_alphabetic)
    }
}
struct Probe {
    editor: Entity<EditorState>,
}
impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("completion-probe")
            .test_support()
            .w_full()
            .h(px(120.))
            .child(Editor::new(&self.editor).size_full())
    }
}
struct Harness {
    window: AnyWindowHandle,
    editor: Entity<EditorState>,
    gates: Gates,
}
fn launch(cx: &mut TestAppContext) -> Harness {
    cx.update(qrow::ui::init);
    let gates = Gates::default();
    let mut editor = None;
    let window = cx.open_window(size(px(640.), px(480.)), |window, cx| {
        let state = cx.new(|cx| {
            let mut state = EditorState::new(window, cx).language("sql");
            state.lsp_mut().completion_provider = Some(Rc::new(Provider {
                gates: gates.clone(),
                note: Default::default(),
            }));
            state
        });
        editor = Some(state.clone());
        let probe = cx.new(|_| Probe { editor: state });
        Root::new(probe, window, cx)
    });
    let harness = Harness {
        window: window.into(),
        editor: editor.unwrap(),
        gates,
    };
    cx.update_window(harness.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("completion-probe", cx);
        window.input("a", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(harness.gates.borrow().len(), 1);
    harness
}
fn old_item() -> Vec<lsp::CompletionItem> {
    vec![lsp::CompletionItem {
        label: "obsolete".into(),
        ..Default::default()
    }]
}

#[gpui_kit::test]
fn dismissing_pending_work_or_typing_a_nontrigger_does_not_reopen_the_menu(
    cx: &mut TestAppContext,
) {
    for action in ["escape", "left", "!"] {
        let harness = launch(cx);
        cx.update_window(harness.window, |_, window, cx| {
            if action == "!" {
                window.input(action, cx);
            } else {
                window.press(action, cx);
            }
        })
        .unwrap();
        cx.run_until_parked();
        let _ = harness.gates.borrow()[0].try_send(old_item());
        cx.run_until_parked();
        cx.update_window(harness.window, |_, window, cx| {
            window.render_frame(cx);
            assert!(!harness.editor.read(cx).completion_menu_state().open);
            assert!(window.try_find("completion-menu").is_none());
        })
        .unwrap();
    }
}

#[gpui_kit::test]
fn manual_presentation_invalidates_an_older_provider_response(cx: &mut TestAppContext) {
    let harness = launch(cx);
    cx.update_window(harness.window, |_, _, cx| {
        harness.editor.update(cx, |editor, cx| {
            editor.present_completion_items(
                0,
                "a",
                vec![lsp::CompletionItem {
                    label: "active".into(),
                    ..Default::default()
                }],
                cx,
            )
        });
    })
    .unwrap();
    let _ = harness.gates.borrow()[0].try_send(old_item());
    cx.run_until_parked();
    cx.update_window(harness.window, |_, window, cx| {
        window.render_frame(cx);
        let state = harness.editor.read(cx).completion_menu_state();
        assert!(state.open);
        assert_eq!(state.items[0].label, "active");
    })
    .unwrap();
}

#[gpui_kit::test]
fn asynchronous_empty_response_keeps_its_completion_note(cx: &mut TestAppContext) {
    let harness = launch(cx);
    harness.gates.borrow()[0].try_send(vec![]).unwrap();
    cx.run_until_parked();
    cx.update_window(harness.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("completion-note").is_some());
        assert!(harness.editor.read(cx).completion_menu_state().open);
    })
    .unwrap();
}
