mod ui;

use gpui_kit::component::{
    Root, Theme,
    highlighter::{LanguageConfig, LanguageRegistry},
};
use gpui_kit::*;

fn main() {
    let demo = std::env::args().any(|arg| arg == "--demo");
    let started = std::time::Instant::now();
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            Theme::sync_system_appearance(None, cx);
            Theme::global_mut(cx).font_size = px(14.);
            Theme::sync_base(cx);
            // Wide result sets need a persistent, discoverable horizontal scrollbar.
            Theme::set_scrollbar_mode(gpui_kit::component::scroll::ScrollbarMode::Always, cx);

            LanguageRegistry::singleton().register(
                "sql",
                &LanguageConfig::new(
                    "sql",
                    tree_sitter_sequel::LANGUAGE.into(),
                    vec![],
                    tree_sitter_sequel::HIGHLIGHTS_QUERY,
                    "",
                    "",
                ),
            );
            ui::init(cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(1280.), px(820.)),
                        cx,
                    ))),
                    titlebar: Some(TitlebarOptions {
                        title: Some(if demo { "Qrow · Demo" } else { "Qrow" }.into()),
                        ..Default::default()
                    }),
                    window_min_size: Some(size(px(850.), px(560.))),
                    ..Default::default()
                },
                |window, cx| {
                    window
                        .observe_window_appearance(|window, cx| {
                            Theme::sync_system_appearance(Some(window), cx);
                        })
                        .detach();
                    let view = cx.new(|cx| ui::Qrow::new(window, cx, demo, started));
                    let shell = cx.new(|_| ui::WindowView::new(view));
                    cx.new(|cx| Root::new(shell, window, cx))
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
