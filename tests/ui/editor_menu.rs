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

#[gpui_kit::test]
fn find_buttons_show_tooltips_including_when_disabled(cx: &mut TestAppContext) {
    use crate::support::{TestApp, labelled};
    use qrow::model::Workspace;
    use std::time::Duration;
    let app = TestApp::launch(cx, Workspace::default());
    app.type_sql(cx, "SELECT 1; SELECT 2;");
    app.press(cx, "secondary-f");
    app.update(cx, |window, cx| window.input("SELECT", cx));
    let hover = |cx: &mut TestAppContext, name: &str| {
        app.hover_labelled(cx, "SQL Editor");
        app.wait_gone(cx, "tooltip");
        app.hover_labelled(cx, name);
        cx.executor().advance_clock(Duration::from_millis(800));
        app.settle(cx);
        app.wait_for(cx, "tooltip");
        app.press(cx, "left");
        app.wait_gone(cx, "tooltip");
    };
    for name in [
        "Match case",
        "Toggle replace",
        "Previous match",
        "Next match",
        "Close search",
    ] {
        hover(cx, name);
    }
    app.click_labelled(cx, "Toggle replace");
    for name in ["Toggle replace", "Replace match", "Replace all matches"] {
        hover(cx, name);
    }
    // No matches disables navigation and replacement, but descriptions remain available.
    app.update(cx, |window, cx| {
        let query = crate::support::elements(window)
            .into_iter()
            .find(|element| element.value() == Some("SELECT"))
            .expect("Find query input");
        crate::support::click_element(window, &query, cx);
        window.press("secondary-a", cx);
        window.input("absent", cx);
    });
    app.wait_until(
        cx,
        "search with no matches",
        Duration::from_secs(5),
        |window, _| !labelled(window, "Next match").unwrap().is_clickable(),
    );
    for name in [
        "Previous match",
        "Next match",
        "Replace match",
        "Replace all matches",
    ] {
        app.update(cx, |window, _| {
            assert!(!labelled(window, name).unwrap().is_clickable())
        });
        hover(cx, name);
    }
    app.press(cx, "escape");
    app.wait_until(
        cx,
        "Find controls to close",
        Duration::from_secs(5),
        |window, _| labelled(window, "Close search").is_none(),
    );
}
