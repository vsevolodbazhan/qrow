use super::setting_row::Rows;
use super::*;
use gpui_kit::component::{
    alert::Alert,
    form::{field, v_form},
    h_flex, v_flex,
};

/// A name is a short value, so the dialog stays narrow and focused.
const DIALOG_REMS: f32 = 30.;

impl Qrow {
    pub(super) fn open_tab_rename_dialog(&self, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let close = weak.clone();
            let save = weak.clone();
            let content = weak.update(cx, |this, cx| this.tab_content(cx)).ok();
            let footer = weak.update(cx, |this, cx| this.tab_footer(cx)).ok();
            dialog
                .w(Rows::dialog_width(window, DIALOG_REMS))
                .overlay_closable(false)
                .on_ok(move |_, window, cx| {
                    let _ = save.update(cx, |this, cx| this.rename_tab(window, cx));
                    // An accepted name closes the popup itself; a rejected one keeps it open.
                    false
                })
                .children(content)
                .when_some(footer, |dialog, footer| dialog.footer(footer))
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |this, cx| {
                        this.tab_form = None;
                        cx.notify();
                    });
                })
        });
    }

    fn tab_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.tab_form else {
            return div().into_any_element();
        };
        v_flex()
            .id("rename-tab-form")
            .key_context("RenameTab")
            .on_action(cx.listener(|this, _: &RenameTab, window, cx| this.rename_tab(window, cx)))
            .w_full()
            .gap_2()
            .child(
                v_form().w_full().child(
                    field()
                        .label("Tab Name")
                        .child(Input::new(&form.title).w_full().aria_label("Tab Name")),
                ),
            )
            .when_some(form.error.clone(), |content, error| {
                content.child(Alert::error("tab-name-error", error))
            })
            .into_any_element()
    }

    fn tab_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.tab_form.is_none() {
            return div().into_any_element();
        }
        v_flex()
            .w_full()
            .gap_2()
            .key_context("RenameTab")
            .on_action(cx.listener(|this, _: &RenameTab, window, cx| this.rename_tab(window, cx)))
            .child(
                h_flex()
                    .gap_2()
                    // One field and two buttons read as a single block; a rule
                    // here would separate nothing. The dialog's own body-to-footer
                    // gap is the whole separation.
                    .child(div().flex_1())
                    .child(
                        Button::new("cancel-tab")
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.tab_form = None;
                                window.close_dialog(cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("rename-tab")
                            .primary()
                            .label("Rename")
                            .tooltip("Rename Tab · ⌘Enter")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.rename_tab(window, cx)),
                            ),
                    ),
            )
            .into_any_element()
    }
}
