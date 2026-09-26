use super::setting_row::Rows;
use super::*;
use gpui_kit::component::{
    alert::Alert,
    form::{field, v_form},
    h_flex, v_flex,
};

/// A name is a short value, so the dialog stays narrow and focused.
const DIALOG_REMS: f32 = 30.;

/// The name field and its validation error.
pub(super) type RenameForm<'a> = (&'a Entity<InputState>, Option<String>);

/// One name field with Cancel and Rename. Query tabs and assistant
/// conversations use the same dialog, so their rename forms look the same.
#[derive(Clone, Copy)]
pub(super) struct RenameDialog {
    /// Element ID suffix, for example `tab` in `rename-tab`.
    pub key: &'static str,
    pub label: &'static str,
    pub tooltip: &'static str,
    /// The name field and the current error, while the dialog is open.
    pub form: fn(&Qrow) -> Option<RenameForm<'_>>,
    /// Validates and saves the name. It closes the dialog when it accepts it.
    pub submit: fn(&mut Qrow, &mut Window, &mut Context<Qrow>),
    pub clear: fn(&mut Qrow),
}

pub(super) const TAB_RENAME: RenameDialog = RenameDialog {
    key: "tab",
    label: "Tab Name",
    tooltip: "Rename Tab · ⌘Enter",
    form: |this| {
        this.tab_form
            .as_ref()
            .map(|form| (&form.title, form.error.clone()))
    },
    submit: Qrow::rename_tab,
    clear: |this| this.tab_form = None,
};

impl Qrow {
    pub(super) fn open_rename_dialog(
        &self,
        spec: RenameDialog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let close = weak.clone();
            let save = weak.clone();
            let content = weak
                .update(cx, |this, cx| this.rename_content(spec, cx))
                .ok();
            let footer = weak
                .update(cx, |this, cx| this.rename_footer(spec, cx))
                .ok();
            dialog
                .w(Rows::dialog_width(window, DIALOG_REMS))
                .overlay_closable(false)
                .on_ok(move |_, window, cx| {
                    let _ = save.update(cx, |this, cx| (spec.submit)(this, window, cx));
                    // An accepted name closes the popup itself; a rejected one keeps it open.
                    false
                })
                .children(content)
                .when_some(footer, |dialog, footer| dialog.footer(footer))
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |this, cx| {
                        (spec.clear)(this);
                        cx.notify();
                    });
                })
        });
    }

    fn rename_content(&self, spec: RenameDialog, cx: &mut Context<Self>) -> AnyElement {
        let Some((input, error)) = (spec.form)(self) else {
            return div().into_any_element();
        };
        v_flex()
            .id(SharedString::from(format!("rename-{}-form", spec.key)))
            .key_context("RenameDialog")
            .on_action(cx.listener(move |this, _: &SubmitRename, window, cx| {
                (spec.submit)(this, window, cx)
            }))
            .w_full()
            .gap_2()
            .child(
                v_form().w_full().child(
                    field()
                        .label(spec.label)
                        .child(Input::new(input).w_full().aria_label(spec.label)),
                ),
            )
            .when_some(error, |content, error| {
                content.child(Alert::error(
                    SharedString::from(format!("{}-name-error", spec.key)),
                    error,
                ))
            })
            .into_any_element()
    }

    fn rename_footer(&self, spec: RenameDialog, cx: &mut Context<Self>) -> AnyElement {
        if (spec.form)(self).is_none() {
            return div().into_any_element();
        }
        v_flex()
            .w_full()
            .gap_2()
            .key_context("RenameDialog")
            .on_action(cx.listener(move |this, _: &SubmitRename, window, cx| {
                (spec.submit)(this, window, cx)
            }))
            .child(
                h_flex()
                    .gap_2()
                    // One field and two buttons read as a single block; a rule
                    // here would separate nothing. The dialog's own body-to-footer
                    // gap is the whole separation.
                    .child(div().flex_1())
                    .child(
                        Button::new(SharedString::from(format!("cancel-{}", spec.key)))
                            .label("Cancel")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                (spec.clear)(this);
                                window.close_dialog(cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("rename-{}", spec.key)))
                            .primary()
                            .label("Rename")
                            .tooltip(spec.tooltip)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                (spec.submit)(this, window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }
}
