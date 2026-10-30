//! The custom native-menu hook must use the capabilities of the right-click.
use gpui_kit::component::{
    Root,
    input::{Editor, EditorState},
    native_menu::NativeMenu,
};
use gpui_kit::test::{TestSupportExt, TestWindowExt};
use gpui_kit::{
    AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render, Styled,
    TestAppContext, Window, div, px, size,
};
use std::{cell::RefCell, rc::Rc};

struct MenuProbe {
    editor: Entity<EditorState>,
    selections: Rc<RefCell<Vec<bool>>>,
}

impl Render for MenuProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let selections = self.selections.clone();
        div()
            .id("menu-probe")
            .test_support()
            .w(px(160.))
            .h(px(40.))
            .child(
                Editor::new(&self.editor)
                    .size_full()
                    .context_menu_with_capabilities(move |_, capabilities, _, _| {
                        selections.borrow_mut().push(capabilities.has_selection());
                        // An empty menu exercises the callback without invoking AppKit.
                        NativeMenu::new()
                    }),
            )
    }
}

#[gpui_kit::test]
fn editor_menu_receives_current_selection_without_borrowing_its_input(cx: &mut TestAppContext) {
    cx.update(qrow::ui::init);
    let selections = Rc::new(RefCell::new(Vec::new()));
    let mut editor = None;
    let window = cx.open_window(size(px(640.), px(480.)), |window, cx| {
        let state = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("sql")
                .default_value("select value from t")
        });
        editor = Some(state.clone());
        let probe = cx.new(|_| MenuProbe {
            editor: state,
            selections: selections.clone(),
        });
        Root::new(probe, window, cx)
    });
    let editor = editor.unwrap();
    for (range, expected) in [(0..19, true), (0..0, false)] {
        cx.update_window(window.into(), |_, window, cx| {
            editor.update(cx, |editor, cx| editor.set_selected_range(range, cx));
            window.render_frame(cx);
            window.right_click("menu-probe", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(selections.borrow().last(), Some(&expected));
    }
}
