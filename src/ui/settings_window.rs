//! Settings in a window of its own, like the Settings of other macOS apps.
//! ⌘, opens the window, or brings an open one to the front. ⌘W, Escape, and
//! the close button close it. Every change applies as it is made; closing
//! keeps the edits that apply only when complete, like a number being typed.

use super::*;
use gpui_kit::component::{Root, TitleBar, v_flex};

actions!(qrow_settings, [CloseSettings]);

/// The key context of the Settings window.
pub(super) const CONTEXT: &str = "SettingsWindow";

/// The size of a new Settings window, in rems of the interface scale.
const WIDTH_REMS: f32 = 56.;
const HEIGHT_REMS: f32 = 42.;
/// The smallest Settings window, in points.
const MIN_WIDTH: f32 = 640.;
const MIN_HEIGHT: f32 = 420.;

pub(super) fn bindings() -> [KeyBinding; 2] {
    [
        KeyBinding::new("secondary-w", CloseSettings, Some(CONTEXT)),
        KeyBinding::new("escape", CloseSettings, Some(CONTEXT)),
    ]
}

/// The root view of the Settings window. Qrow owns the settings and the
/// form; the view renders them and renders again when Qrow changes.
pub(super) struct SettingsWindow {
    qrow: WeakEntity<Qrow>,
    focus: FocusHandle,
    _observe: Option<Subscription>,
}

impl SettingsWindow {
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let _ = self
            .qrow
            .update(cx, |qrow, cx| qrow.settings_window_closed(window, cx));
        window.remove_window();
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = self
            .qrow
            .update(cx, |qrow, cx| qrow.settings_window_content(window, cx))
            .unwrap_or_else(|_| div().into_any_element());
        v_flex()
            .id("settings-window")
            .test_support()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &CloseSettings, window, cx| this.close(window, cx)))
            // ⌘, brings this window to the front; it does not open another.
            .on_action(|_: &OpenSettings, window, _| window.activate_window())
            .on_action(cx.listener(|this, _: &Quit, window, cx| {
                let _ = this
                    .qrow
                    .update(cx, |qrow, cx| qrow.request_quit(window, cx));
            }))
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .font_family(cx.theme().font_family.clone())
            .text_sm()
            .when(
                cfg!(target_os = "linux")
                    && matches!(window.window_decorations(), Decorations::Client { .. }),
                |view| {
                    view.child(
                        TitleBar::new()
                            .on_close_window(
                                cx.listener(|this, _, window, cx| this.close(window, cx)),
                            )
                            .child(div().flex_1().text_center().child("Settings")),
                    )
                },
            )
            .child(div().flex_1().min_h_0().child(content))
            .children(Root::render_dialog_layer(window, cx))
            .children(Root::render_notification_layer(window, cx))
    }
}

/// Open the Settings window for `qrow`. Called outside any update of Qrow,
/// because building the window creates the form inside Qrow.
pub(super) fn open(qrow: WeakEntity<Qrow>, cx: &mut App) {
    let rem = cx.theme().font_size;
    let size = size(
        (rem * WIDTH_REMS).max(px(MIN_WIDTH)),
        (rem * HEIGHT_REMS).max(px(MIN_HEIGHT)),
    );
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size, cx))),
        titlebar: Some(TitlebarOptions {
            title: Some("Settings".into()),
            appears_transparent: false,
            traffic_light_position: None,
        }),
        window_min_size: Some(gpui_kit::size(px(MIN_WIDTH), px(MIN_HEIGHT))),
        app_id: cfg!(target_os = "linux").then(|| "qrow".into()),
        window_decorations: cfg!(target_os = "linux").then_some(WindowDecorations::Client),
        // Like other macOS Settings windows: it can be minimized, but it has
        // a fixed size, so the zoom button and full screen are off.
        is_minimizable: true,
        is_resizable: false,
        ..WindowOptions::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        window.set_rem_size(cx.theme().font_size);
        let _ = qrow.update(cx, |qrow, cx| qrow.init_settings_form(window, cx));
        let closing = qrow.clone();
        window.on_window_should_close(cx, move |window, cx| {
            let _ = closing.update(cx, |qrow, cx| qrow.settings_window_closed(window, cx));
            true
        });
        let view = cx.new(|cx| {
            let observe = qrow
                .upgrade()
                .map(|qrow| cx.observe(&qrow, |_, _, cx| cx.notify()));
            SettingsWindow {
                qrow: qrow.clone(),
                focus: cx.focus_handle(),
                _observe: observe,
            }
        });
        let focus = view.read(cx).focus.clone();
        focus.focus(window, cx);
        cx.new(|cx| Root::new(view, window, cx))
    });
    let _ = qrow.update(cx, |qrow, cx| {
        qrow.settings_window = opened.ok().map(Into::into);
        qrow.settings_open = qrow.settings_window.is_some();
        cx.notify();
    });
}
