use super::*;

#[derive(Clone, Copy)]
pub(super) struct Rows;

impl Rows {
    pub(super) fn dialog_width(window: &Window, rems: f32) -> Pixels {
        let rem = window.rem_size();
        (rem * rems).min(window.viewport_size().width - rem * 4.)
    }
}
