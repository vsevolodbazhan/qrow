//! Toast notifications share one layout: the type icon, a bold title that
//! names the event, and optional detail lines. One action shares the row
//! with the text; two or more go on a row below it.
use super::*;
pub(super) use gpui_kit::component::notification::NotificationType;
use gpui_kit::component::{h_flex, notification::Notification, v_flex};
use std::path::PathBuf;
use std::rc::Rc;

/// A toast with a title and an optional detail line. `id` names its body in tests.
pub(super) fn notice(
    kind: NotificationType,
    id: &'static str,
    title: impl Into<SharedString>,
    detail: Option<SharedString>,
) -> Notification {
    let title = title.into();
    Notification::new().with_type(kind).content(move |_, _, _| {
        body(kind, id)
            .test_support()
            .child(title_line("title", title.clone()))
            .when_some(detail.clone(), |body, detail| {
                body.child(detail_line("detail", detail))
            })
            .into_any_element()
    })
}

/// What a toast button does after the toast closes.
pub(super) type OnClick = Rc<dyn Fn(&mut Window, &mut App)>;

/// A button of a toast. A click closes the toast, then runs `on_click`.
#[derive(Clone)]
pub(super) struct ToastAction {
    pub id: &'static str,
    pub label: &'static str,
    pub on_click: OnClick,
}

/// A name that a toast shows in a code font, like the editor.
#[derive(Clone)]
pub(super) struct CodeName {
    pub text: SharedString,
    pub font: SharedString,
}

/// A toast with a title, an optional name, an optional detail line, and
/// actions. One action shares the row with the text; two or more go on a
/// row below it. The name uses the full width of the text, so a toast with
/// a name has no action or two or more.
pub(super) fn notice_with_actions(
    kind: NotificationType,
    id: &'static str,
    title: impl Into<SharedString>,
    name: Option<CodeName>,
    detail: Option<SharedString>,
    actions: Vec<ToastAction>,
) -> Notification {
    let title = title.into();
    let side = actions.len() == 1;
    debug_assert!(!(side && name.is_some()), "a named toast with one action");
    let row = if side { Vec::new() } else { actions.clone() };
    let notification = Notification::new()
        .with_type(kind)
        .content(move |_, window, cx| {
            body(kind, id)
                .test_support()
                .child(title_line("title", title.clone()))
                .when_some(name.clone(), |body, name| {
                    body.child(name_line(name, window, cx))
                })
                .when_some(detail.clone(), |body, detail| {
                    body.child(detail_line("detail", detail))
                })
                .when(!row.is_empty(), |body| {
                    body.child(
                        h_flex()
                            .mt(rems(0.625))
                            .gap_2()
                            .children(row.iter().map(|action| action_button(action, cx))),
                    )
                })
                .into_any_element()
        });
    match actions.into_iter().next().filter(|_| side) {
        Some(action) => notification.action(move |_, _, cx| action_button(&action, cx)),
        None => notification,
    }
}

fn action_button(action: &ToastAction, cx: &mut Context<Notification>) -> Button {
    let on_click = action.on_click.clone();
    Button::new(action.id)
        .small()
        .label(action.label)
        .on_click(cx.listener(move |toast, _, window, cx| {
            toast.dismiss(window, cx);
            on_click(window, cx);
        }))
}

/// One line of a name in its code font, cut with an ellipsis. A cut name
/// shows in full in a tooltip.
fn name_line(name: CodeName, window: &mut Window, cx: &App) -> impl IntoElement {
    let size = rems(0.8125).to_pixels(window.rem_size());
    // The text of a toast is its width less the border, the padding, and
    // the indent of the type icon.
    let room = cx.theme().notification.width - px(2.) - rems(3.5).to_pixels(window.rem_size());
    let cut = text_width(window, &name.text, &font(name.font.clone()), size) > room;
    let tooltip = name.text.clone();
    div()
        .id("name")
        .test_support()
        .role(Role::Label)
        .aria_label(name.text.clone())
        .w_full()
        .min_w_0()
        .font_family(name.font)
        .text_size(size)
        .text_color(cx.theme().muted_foreground)
        .when(cut, |line| {
            line.tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
            })
        })
        .truncate()
        .child(name.text)
}

/// An error toast: the failed operation as the title, the cause below it.
pub(super) fn error(
    id: &'static str,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
) -> Notification {
    notice(NotificationType::Error, id, title, Some(detail.into()))
}

/// The column that holds the text of a toast. Errors and warnings are alerts.
/// Its lines have the line height of other small text, not the taller
/// default of the window.
pub(super) fn body(kind: NotificationType, id: impl Into<ElementId>) -> Stateful<Div> {
    let role = match kind {
        NotificationType::Error | NotificationType::Warning => Role::Alert,
        NotificationType::Info | NotificationType::Success => Role::Status,
    };
    v_flex().id(id).role(role).min_w_0().line_height(rems(1.25))
}

/// The bold first line that names the event.
pub(super) fn title_line(id: &'static str, title: SharedString) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
        .role(Role::Label)
        .aria_label(title.clone())
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .child(title)
}

/// A line of detail below the title. Long text wraps.
pub(super) fn detail_line(id: &'static str, detail: SharedString) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
        .role(Role::Label)
        .aria_label(detail.clone())
        .text_sm()
        .whitespace_normal()
        .child(detail)
}

/// Test IDs of one kind of saved-file toast.
pub(super) struct SavedFileIds {
    pub notice: &'static str,
    pub title: &'static str,
    pub filename: &'static str,
    pub reveal: &'static str,
}

/// The label of the action that shows a file in the system file manager.
pub(super) const REVEAL_LABEL: &str = if cfg!(target_os = "macos") {
    "Reveal in Finder"
} else if cfg!(windows) {
    "Reveal in File Explorer"
} else {
    "Open Containing Folder"
};

/// A success toast for a written file, with a [`REVEAL_LABEL`] action.
pub(super) fn saved_file(ids: SavedFileIds, title: String, path: PathBuf) -> Notification {
    let filename = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let title = SharedString::from(title);
    Notification::new()
        .with_type(NotificationType::Success)
        .content(move |_, _, _| {
            body(NotificationType::Success, ids.notice)
                .test_support()
                .child(title_line(ids.title, title.clone()))
                .child(filename_line(ids.filename, filename.clone()))
                .into_any_element()
        })
        .action(move |_, _, _| {
            let path = path.clone();
            Button::new(ids.reveal)
                .label(REVEAL_LABEL)
                .on_click(move |_, _, cx| cx.reveal_path(&path))
        })
}

/// One line of a file name, cut with an ellipsis. The tooltip shows the full name.
pub(super) fn filename_line(id: &'static str, filename: String) -> impl IntoElement {
    let tooltip = filename.clone();
    div()
        .id(id)
        .test_support()
        .role(Role::Label)
        .aria_label(filename.clone())
        .w_full()
        .min_w_0()
        .text_sm()
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
        })
        .truncate()
        .child(filename)
}
