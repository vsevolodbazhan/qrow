use gpui_kit::component::TitleBar;
use gpui_kit::*;
use qrow::ui::{self, Environment};

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
                "Qrow · Demo"
            } else {
                "Qrow"
            };
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(1280.), px(820.)),
                        cx,
                    ))),
                    titlebar: Some(TitlebarOptions {
                        title: Some(title.into()),
                        ..TitleBar::title_bar_options()
                    }),
                    window_min_size: Some(size(px(850.), px(560.))),
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
