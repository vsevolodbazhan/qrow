#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use gpui_kit::component::TitleBar;
use gpui_kit::*;
use qrow::{
    storage,
    ui::{self, Environment},
};

fn initial_window_bounds(environment: &Environment, cx: &App) -> WindowBounds {
    let default_bounds = || {
        WindowBounds::Windowed(Bounds::centered(
            None,
            size(
                px(storage::DEFAULT_WINDOW_WIDTH),
                px(storage::DEFAULT_WINDOW_HEIGHT),
            ),
            cx,
        ))
    };
    if environment.is_demo() {
        return default_bounds();
    }
    let Some(local_dir) = environment.local_directory_path() else {
        return default_bounds();
    };
    let Some(saved) = storage::load_window_bounds(&storage::window_state_path(local_dir)) else {
        return default_bounds();
    };
    let displays: Vec<(f32, f32, f32, f32)> = cx
        .displays()
        .into_iter()
        .map(|display| {
            let b = display.bounds();
            (
                f32::from(b.origin.x),
                f32::from(b.origin.y),
                f32::from(b.size.width),
                f32::from(b.size.height),
            )
        })
        .collect();
    if !saved.fits_any_display(&displays) {
        return default_bounds();
    }
    let rect = Bounds::new(
        point(px(saved.x), px(saved.y)),
        size(px(saved.width), px(saved.height)),
    );
    if saved.maximized {
        WindowBounds::Maximized(rect)
    } else {
        WindowBounds::Windowed(rect)
    }
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
            let window_bounds = initial_window_bounds(&environment, cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(window_bounds),
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
