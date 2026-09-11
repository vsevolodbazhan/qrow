mod ui;

use gpui::*;
use gpui_component::{
    Root, Theme, ThemeMode,
    highlighter::{LanguageConfig, LanguageRegistry},
};

fn main() {
    let demo = std::env::args().any(|arg| arg == "--demo");
    let started = std::time::Instant::now();
    Application::new()
        .with_assets(gpui_component_assets::Assets)
        .run(move |cx| {
            gpui_component::init(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            let theme = Theme::global_mut(cx);
            theme.font_size = px(14.);
            theme.scrollbar_show = gpui_component::scroll::ScrollbarShow::Always;
            theme.background = rgb(0x282c34).into();
            theme.foreground = rgb(0xcdd3de).into();
            theme.border = rgb(0x363c47).into();
            theme.input = rgb(0x20232a).into();
            theme.primary = rgb(0x7aa2f7).into();
            theme.primary_hover = rgb(0x91b2fa).into();
            theme.primary_foreground = rgb(0x182030).into();
            theme.selection = rgb(0x3d4e6c).into();
            theme.table = rgb(0x282c34).into();
            theme.table_even = rgb(0x2c3039).into();
            theme.table_head = rgb(0x242830).into();
            theme.table_head_foreground = rgb(0xb7c1d0).into();
            theme.table_hover = rgb(0x323945).into();
            theme.table_active = rgb(0x34425b).into();
            theme.table_active_border = rgb(0x617dad).into();
            theme.table_row_border = rgb(0x333944).into();
            theme.popover = rgb(0x292e38).into();
            theme.popover_foreground = rgb(0xcdd3de).into();
            let highlight = std::sync::Arc::make_mut(&mut theme.highlight_theme);
            highlight.style.editor_background = Some(rgb(0x282c34).into());
            highlight.style.editor_foreground = Some(rgb(0xcdd3de).into());
            highlight.style.editor_active_line = Some(rgb(0x2e333d).into());
            highlight.style.editor_line_number = Some(rgb(0x636d7c).into());
            highlight.style.editor_active_line_number = Some(rgb(0xb9c4d5).into());

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
                    let view = cx.new(|cx| ui::Qrow::new(window, cx, demo, started));
                    cx.new(|cx| Root::new(view, window, cx))
                },
            )
            .expect("open Qrow window");
            cx.activate(true);
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
        });
}
