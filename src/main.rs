#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use gpui_kit::component::TitleBar;
use gpui_kit::*;
use qrow::{
    storage,
    ui::{self, Environment},
};

fn initial_window_placement(
    environment: &Environment,
    cx: &App,
) -> (WindowBounds, Option<DisplayId>) {
    let default_placement = || {
        (
            WindowBounds::Windowed(Bounds::centered(
                None,
                size(
                    px(storage::DEFAULT_WINDOW_WIDTH),
                    px(storage::DEFAULT_WINDOW_HEIGHT),
                ),
                cx,
            )),
            None,
        )
    };
    if environment.is_demo() {
        return default_placement();
    }
    let Some(local_dir) = environment.local_directory_path() else {
        return default_placement();
    };
    let Some(saved) = storage::load_window_bounds(&storage::window_state_path(local_dir)) else {
        return default_placement();
    };
    let displays = cx.displays();
    let display_rects = |list: &[std::rc::Rc<dyn PlatformDisplay>]| -> Vec<(f32, f32, f32, f32)> {
        list.iter()
            .map(|display| {
                let b = display.bounds();
                (
                    f32::from(b.origin.x),
                    f32::from(b.origin.y),
                    f32::from(b.size.width),
                    f32::from(b.size.height),
                )
            })
            .collect()
    };

    // Match the saved monitor by its stable UUID when present so secondary and
    // mixed-DPI monitors restore onto the same display. If that monitor was
    // disconnected, fall back to default centered bounds on the active display.
    let matched_display = if let Some(target_uuid) = saved.display_uuid {
        let Some(display) = displays
            .iter()
            .find(|d| d.uuid().ok() == Some(target_uuid))
            .cloned()
        else {
            return default_placement();
        };
        // On macOS, GPUI stores window bounds in display-local coordinates
        // (origin `(0, 0)` on the target display); on other platforms they are
        // in global desktop coordinates matching `display.bounds()`.
        let b = display.bounds();
        let target_rect = if cfg!(target_os = "macos") {
            (0., 0., f32::from(b.size.width), f32::from(b.size.height))
        } else {
            (
                f32::from(b.origin.x),
                f32::from(b.origin.y),
                f32::from(b.size.width),
                f32::from(b.size.height),
            )
        };
        if !saved.fits_any_display(&[target_rect]) {
            return default_placement();
        }
        Some(display.id())
    } else {
        let rects = display_rects(&displays);
        if !saved.fits_any_display(&rects) {
            return default_placement();
        }
        displays
            .iter()
            .zip(&rects)
            .find(|&(_, &rect)| saved.fits_any_display(&[rect]))
            .map(|(display, _)| display.id())
    };

    let rect = Bounds::new(
        point(px(saved.x), px(saved.y)),
        size(px(saved.width), px(saved.height)),
    );
    let bounds = if saved.maximized {
        WindowBounds::Maximized(rect)
    } else {
        WindowBounds::Windowed(rect)
    };
    (bounds, matched_display)
}

fn main() {
    let environment = if std::env::args().any(|arg| arg == "--demo") {
        Environment::demo()
    } else {
        Environment::user()
    };
    let started = std::time::Instant::now();
    gpui_kit::application()
        .with_assets(ui::Assets)
        .run(move |cx| {
            ui::init(cx);
            let title = if environment.is_demo() {
                "Qrow (Demo)"
            } else {
                "Qrow"
            };
            let (window_bounds, display_id) = initial_window_placement(&environment, cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(window_bounds),
                    display_id,
                    titlebar: Some(TitlebarOptions {
                        title: Some(title.into()),
                        ..TitleBar::title_bar_options()
                    }),
                    window_min_size: Some(size(
                        px(storage::MIN_WINDOW_WIDTH),
                        px(storage::MIN_WINDOW_HEIGHT),
                    )),
                    app_id: cfg!(target_os = "linux").then(|| "qrow".into()),
                    window_decorations: if cfg!(not(target_os = "macos")) {
                        Some(WindowDecorations::Client)
                    } else {
                        None
                    },
                    ..TitleBar::window_options()
                },
                |window, cx| {
                    let view = cx.new(|cx| ui::Qrow::new(environment, started, window, cx));
                    cx.new(|cx| ui::root(view, window, cx))
                },
            )
            .expect("open Qrow window");
            cx.activate(true);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
        });
}
