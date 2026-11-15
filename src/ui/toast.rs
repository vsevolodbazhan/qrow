//! Toast notifications share one layout: the type icon, a bold title that
//! names the event, optional detail lines, and the actions on a row below
//! the text. A toast shows only text that Qrow writes. The error of the
//! database, the system, or a library goes to Activity, and the toast has
//! **Show activity**.
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
    /// The main action of the toast, like Restart to update.
    pub primary: bool,
    pub on_click: OnClick,
}

/// **Show activity**: opens the Activity log `log`, a connection or
/// [`crate::activity::APPLICATION`].
pub(super) fn show_activity(id: &'static str, owner: WeakEntity<Qrow>, log: Uuid) -> ToastAction {
    ToastAction {
        id,
        label: SHOW_ACTIVITY,
        primary: false,
        on_click: Rc::new(move |window, cx| {
            let _ = owner.update(cx, |this, cx| this.open_activity(Some(log), window, cx));
        }),
    }
}

/// A name that a toast shows in a code font, like the editor.
#[derive(Clone)]
pub(super) struct CodeName {
    pub text: SharedString,
    pub font: SharedString,
}

/// A toast with a title, an optional name, an optional detail line, and
/// actions on a row below the text.
pub(super) fn notice_with_actions(
    kind: NotificationType,
    id: &'static str,
    title: impl Into<SharedString>,
    name: Option<CodeName>,
    detail: Option<SharedString>,
    actions: Vec<ToastAction>,
) -> Notification {
    let title = title.into();
    Notification::new()
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
                .when(!actions.is_empty(), |body| {
                    body.child(actions_row(&actions, cx))
                })
                .into_any_element()
        })
}

/// The row of buttons below the text of a toast, aligned with the text.
fn actions_row(actions: &[ToastAction], cx: &mut Context<Notification>) -> impl IntoElement {
    h_flex()
        .id("actions")
        .test_support()
        .mt(rems(0.625))
        .gap_2()
        .flex_wrap()
        .children(actions.iter().map(|action| action_button(action, cx)))
}

fn action_button(action: &ToastAction, cx: &mut Context<Notification>) -> Button {
    let on_click = action.on_click.clone();
    Button::new(action.id)
        .small()
        .when(action.primary, |button| button.primary())
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
    let cut =
        text_width(window, &name.text, &font(name.font.clone()), size) > text_room(window, cx);
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

/// The width of the text of a toast: its width less the border, the
/// padding, and the indent of the type icon. The actions are below the
/// text, so they take none of it.
fn text_room(window: &Window, cx: &App) -> Pixels {
    cx.theme().notification.width - px(2.) - rems(3.5).to_pixels(window.rem_size())
}

/// An error toast whose cause Qrow wrote: the failed operation as the
/// title, the cause below it. An error of the database, the system, or a
/// library goes through [`Qrow::report_failure`] instead.
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

/// A success toast for a written file, with a [`REVEAL_LABEL`] action. It
/// stays until the user closes it, as for a background export; a routine
/// save sets it to hide.
pub(super) fn saved_file(ids: SavedFileIds, title: String, path: PathBuf) -> Notification {
    let filename = file_name(&path);
    let title = SharedString::from(title);
    let reveal = ToastAction {
        id: ids.reveal,
        label: REVEAL_LABEL,
        primary: false,
        on_click: Rc::new(move |_, cx| cx.reveal_path(&path)),
    };
    Notification::new()
        .with_type(NotificationType::Success)
        .content(move |_, window, cx| {
            body(NotificationType::Success, ids.notice)
                .test_support()
                .child(title_line(ids.title, title.clone()))
                .child(filename_line(ids.filename, filename.clone(), window, cx))
                .child(actions_row(std::slice::from_ref(&reveal), cx))
                .into_any_element()
        })
        .autohide(false)
}

/// The last part of `path`, as a toast shows it.
pub(super) fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// One line of a file name, cut with an ellipsis. A cut name shows in full
/// in a tooltip.
pub(super) fn filename_line(
    id: &'static str,
    filename: String,
    window: &mut Window,
    cx: &App,
) -> impl IntoElement {
    let size = rems(0.875).to_pixels(window.rem_size());
    let ui_font = font(cx.theme().font_family.clone());
    let cut = text_width(window, &filename, &ui_font, size) > text_room(window, cx);
    let tooltip = filename.clone();
    div()
        .id(id)
        .test_support()
        .role(Role::Label)
        .aria_label(filename.clone())
        .w_full()
        .min_w_0()
        .text_sm()
        .when(cut, |line| {
            line.tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
            })
        })
        .truncate()
        .child(filename)
}

/// A failed operation whose error Qrow did not write.
pub(super) struct Failure {
    /// The test ID of the toast.
    pub id: &'static str,
    /// The failed operation, for example "Could not save SQL file".
    pub title: &'static str,
    /// The file of the operation, when it has one.
    pub file: Option<String>,
    /// The Activity log that keeps the error: a connection, or
    /// [`crate::activity::APPLICATION`].
    pub log: Uuid,
    /// The source lane of the Activity entry.
    pub source: &'static str,
    /// The error, for Activity only. It names the path of a file.
    pub error: String,
}

impl Qrow {
    /// Records the error of `failure` in Activity, and shows a toast with
    /// its title, its file, and **Show activity**.
    pub(super) fn report_failure(
        &mut self,
        failure: Failure,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Failure {
            id,
            title,
            file,
            log,
            source,
            error,
        } = failure;
        let entry =
            crate::activity::ActivityEntry::new(Severity::Error, format!("{title}: {error}"))
                .with_source(source);
        self.record_activity(log, entry, cx);
        let show_activity = show_activity("show-activity", cx.entity().downgrade(), log);
        let toast = Notification::new()
            .with_type(NotificationType::Error)
            .content(move |_, window, cx| {
                body(NotificationType::Error, id)
                    .test_support()
                    .child(title_line("title", title.into()))
                    .when_some(file.clone(), |body, file| {
                        body.child(filename_line("filename", file, window, cx))
                    })
                    .child(actions_row(std::slice::from_ref(&show_activity), cx))
                    .into_any_element()
            });
        window.push_notification(toast, cx);
    }
}

/// A weak handle to the window's [`Qrow`], for code that reports a failure
/// without a `Qrow` context, like a copy from the result grid.
pub(super) struct FailureSink(pub WeakEntity<Qrow>);
impl Global for FailureSink {}

/// [`Qrow::report_failure`] for code without a `Qrow` context.
pub(super) fn report_failure(failure: Failure, window: &mut Window, cx: &mut App) {
    let Some(owner) = cx.try_global::<FailureSink>().map(|sink| sink.0.clone()) else {
        return;
    };
    let _ = owner.update(cx, |this, cx| this.report_failure(failure, window, cx));
}

/// The label of the action that opens the Activity of a connection or app.
pub(super) const SHOW_ACTIVITY: &str = "Show activity";
