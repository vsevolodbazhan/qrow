//! The notification that confirms a file write and offers to reveal the file.
use super::*;
use gpui_kit::component::{
    notification::{Notification, NotificationType},
    v_flex,
};
use std::path::PathBuf;

/// Test IDs of one kind of saved-file notification.
pub(super) struct SavedFileIds {
    pub notice: &'static str,
    pub title: &'static str,
    pub filename: &'static str,
    pub reveal: &'static str,
}

/// A success notification: the event as the title, the file name below it,
/// and **Reveal in Finder** on the same row.
pub(super) fn saved_file(ids: SavedFileIds, title: String, path: PathBuf) -> Notification {
    let filename = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    Notification::new()
        .with_type(NotificationType::Success)
        .content(move |_, _, _| {
            v_flex()
                .id(ids.notice)
                .test_support()
                .min_w_0()
                .child(
                    div()
                        .id(ids.title)
                        .test_support()
                        .role(Role::Status)
                        .aria_label(title.clone())
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title.clone()),
                )
                .child(filename_line(
                    ids.filename,
                    filename.clone(),
                    FontWeight::NORMAL,
                ))
                .into_any_element()
        })
        .action(move |_, _, _| {
            let path = path.clone();
            Button::new(ids.reveal)
                .label("Reveal in Finder")
                .on_click(move |_, _, cx| cx.reveal_path(&path))
        })
}

/// One line of a file name, cut with an ellipsis. The tooltip shows the full name.
pub(super) fn filename_line(
    id: &'static str,
    filename: String,
    weight: FontWeight,
) -> impl IntoElement {
    let tooltip = filename.clone();
    div()
        .id(id)
        .test_support()
        .role(Role::Label)
        .aria_label(filename.clone())
        .w_full()
        .min_w_0()
        .text_sm()
        .font_weight(weight)
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
        })
        .truncate()
        .child(filename)
}
