use super::setting_row::Rows;
use super::*;
use gpui_kit::component::{h_flex, v_flex};

/// Matches the settings dialogs: label and description left, control right.
const DIALOG_REMS: f32 = 44.;

impl Qrow {
    pub(super) fn open_tab_dialog(&self, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let close = weak.clone();
            let save = weak.clone();
            let content = weak
                .update(cx, |this, cx| this.tab_content(window, cx))
                .ok();
            let footer = weak.update(cx, |this, cx| this.tab_footer(cx)).ok();
            dialog
                .title("Tab Settings")
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

    fn tab_content(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.tab_form else {
            return div().into_any_element();
        };
        let rows = Rows::filling(Rows::dialog_width(window, DIALOG_REMS), window.rem_size());
        v_flex()
            .key_context("TabSettings")
            .on_action(cx.listener(|this, _: &RenameTab, window, cx| this.rename_tab(window, cx)))
            .w_full()
            .child(
                rows.row(
                    "Name",
                    "Leave blank to keep the current name.",
                    Input::new(&form.title)
                        .aria_label("Tab Name")
                        .into_any_element(),
                    cx,
                ),
            )
            .into_any_element()
    }

    fn tab_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.tab_form else {
            return div().into_any_element();
        };
        v_flex()
            .w_full()
            .gap_2()
            .key_context("TabSettings")
            .on_action(cx.listener(|this, _: &RenameTab, window, cx| this.rename_tab(window, cx)))
            .when_some(form.error.clone(), |el, error| {
                el.child(div().text_color(cx.theme().danger).child(error))
            })
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
                        Button::new("save-tab")
                            .primary()
                            .label("Save")
                            .tooltip("Rename Tab · ⌘Enter")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.rename_tab(window, cx)),
                            ),
                    ),
            )
            .into_any_element()
    }
}
