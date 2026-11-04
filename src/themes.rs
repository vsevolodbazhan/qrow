use crate::model::SYSTEM_THEME;
#[cfg(test)]
use gpui_kit::component::ThemeSet;
use gpui_kit::component::{Theme, ThemeRegistry};
use gpui_kit::{App, Window};

#[cfg(test)]
pub(crate) const ONE_DARK_THEME: &str = "One Dark";
#[cfg(test)]
const CM_TWILIGHT_THEME: &str = "CM Twilight";

const THEME_SET_SOURCES: &[&str] = &[
    include_str!("../themes/adventure.json"),
    include_str!("../themes/alduin.json"),
    include_str!("../themes/asciinema.json"),
    include_str!("../themes/aurora.json"),
    include_str!("../themes/ayu.json"),
    include_str!("../themes/catppuccin.json"),
    include_str!("../themes/everforest.json"),
    include_str!("../themes/fahrenheit.json"),
    include_str!("../themes/flexoki.json"),
    include_str!("../themes/gruvbox.json"),
    include_str!("../themes/harper.json"),
    include_str!("../themes/hybrid.json"),
    include_str!("../themes/jellybeans.json"),
    include_str!("../themes/kibble.json"),
    include_str!("../themes/macos-classic.json"),
    include_str!("../themes/mellifluous.json"),
    include_str!("../themes/molokai.json"),
    include_str!("../themes/solarized.json"),
    include_str!("../themes/spaceduck.json"),
    include_str!("../themes/tokyonight.json"),
    include_str!("../themes/twilight.json"),
];

const QROW_THEME_SET: &str = concat!(
    r#"{"name":"Qrow","themes":["#,
    include_str!("one-dark.json"),
    ",",
    include_str!("cm-twilight.json"),
    r#"]}"#,
);

fn reset_configurable_defaults(cx: &mut App) {
    let defaults = Theme::default();
    let theme = Theme::global_mut(cx);
    theme.font_family = defaults.font_family;
    theme.font_size = defaults.font_size;
    theme.mono_font_family = defaults.mono_font_family;
    theme.mono_font_size = defaults.mono_font_size;
    theme.radius = defaults.radius;
    theme.radius_lg = defaults.radius_lg;
    theme.shadow = defaults.shadow;
}

pub(crate) fn init(cx: &mut App) {
    let registry = ThemeRegistry::global_mut(cx);
    for source in THEME_SET_SOURCES
        .iter()
        .copied()
        .chain(std::iter::once(QROW_THEME_SET))
    {
        registry
            .load_themes_from_str(source)
            .expect("valid bundled GPUI Kit theme set");
    }
}

pub(crate) fn options(cx: &App) -> Vec<String> {
    let mut names: Vec<_> = ThemeRegistry::global(cx)
        .themes()
        .values()
        .filter(|theme| !theme.is_default)
        .map(|theme| theme.name.to_string())
        .collect();
    names.sort_unstable_by_key(|name| name.to_lowercase());

    let mut options = Vec::with_capacity(names.len() + 1);
    options.push(SYSTEM_THEME.to_owned());
    options.extend(names);
    options
}

pub(crate) fn is_available(name: &str, cx: &App) -> bool {
    name == SYSTEM_THEME
        || ThemeRegistry::global(cx)
            .themes()
            .get(name)
            .is_some_and(|theme| !theme.is_default)
}

/// The least opacity of the dialog backdrop. Many themes, the GPUI Kit
/// defaults among them, dim so little that a dialog blends into the window.
const MIN_OVERLAY_OPACITY_LIGHT: f32 = 0.2;
const MIN_OVERLAY_OPACITY_DARK: f32 = 0.45;

/// Raise the backdrop of the active theme to the least opacity. The hue of
/// the theme stays.
fn strengthen_overlay(cx: &mut App) {
    let theme = Theme::global_mut(cx);
    let least = if theme.is_dark() {
        MIN_OVERLAY_OPACITY_DARK
    } else {
        MIN_OVERLAY_OPACITY_LIGHT
    };
    if theme.overlay.a < least {
        theme.overlay = theme.overlay.alpha(least);
    }
}

/// The highest saturation of a SQL comment in a light theme. A more colorful
/// comment outweighs the SQL around it.
const MAX_LIGHT_COMMENT_SATURATION: f32 = 0.35;

/// Give saturated SQL comments of a light theme the muted text color. The
/// GPUI Kit default and macOS Classic paint them in a strong blue.
fn quiet_light_comments(cx: &mut App) {
    use gpui_kit::HighlightStyle;
    use gpui_kit::component::highlighter::HighlightTheme;
    use std::sync::Arc;

    let theme = Theme::global_mut(cx);
    if theme.is_dark() {
        return;
    }
    let saturated = theme
        .highlight_theme
        .style
        .syntax
        .comment
        .clone()
        .map(HighlightStyle::from)
        .and_then(|style| style.color)
        .is_some_and(|color| color.s > MAX_LIGHT_COMMENT_SATURATION);
    if !saturated {
        return;
    }
    let muted = theme.muted_foreground.to_rgb();
    let channel = |value: f32| (value.clamp(0., 1.) * 255.).round() as u8;
    let color = format!(
        "#{:02x}{:02x}{:02x}",
        channel(muted.r),
        channel(muted.g),
        channel(muted.b)
    );
    let Ok(comment) = serde_json::from_value(serde_json::json!({ "color": color })) else {
        return;
    };
    let mut highlight: HighlightTheme = (*theme.highlight_theme).clone();
    highlight.style.syntax.comment = Some(comment);
    theme.highlight_theme = Arc::new(highlight);
}

/// Give the SQL keywords that the grammar marks as attributes, like `DESC`,
/// `ASC`, and `NULLS`, the keyword color, so that all keywords share one
/// color in each theme.
fn unify_sql_keywords(cx: &mut App) {
    use gpui_kit::component::highlighter::HighlightTheme;
    use std::sync::Arc;

    let theme = Theme::global_mut(cx);
    let Some(keyword) = theme.highlight_theme.style.syntax.keyword.clone() else {
        return;
    };
    let mut highlight: HighlightTheme = (*theme.highlight_theme).clone();
    highlight.style.syntax.attribute = Some(keyword);
    theme.highlight_theme = Arc::new(highlight);
}

/// The least difference in lightness between the rows of a striped table.
const MIN_STRIPE_LIGHTNESS: f32 = 0.025;
/// The share of the foreground color in a stripe that a theme makes too faint.
const STRIPE_FOREGROUND: f32 = 0.045;

/// Make the alternate rows of a dark table faintly visible. Some dark
/// themes, the GPUI Kit default among them, stripe them in almost the color
/// of the other rows.
fn visible_dark_stripes(cx: &mut App) {
    let theme = Theme::global_mut(cx);
    if !theme.is_dark() {
        return;
    }
    let base: gpui_kit::Hsla = theme.tokens.table.into();
    let even: gpui_kit::Hsla = theme.tokens.table_even.into();
    if (even.l - base.l).abs() >= MIN_STRIPE_LIGHTNESS {
        return;
    }
    let stripe = base.blend(theme.foreground.opacity(STRIPE_FOREGROUND));
    theme.table_even = stripe;
    theme.tokens.table_even = stripe.into();
}

/// Give the native parts of every window, like the title bar and its
/// buttons, the appearance of the theme. The System theme follows macOS.
fn match_window_appearance(name: &str, cx: &mut App) {
    let appearance = (name != SYSTEM_THEME).then(|| {
        if Theme::global(cx).is_dark() {
            gpui_kit::WindowAppearance::Dark
        } else {
            gpui_kit::WindowAppearance::Light
        }
    });
    cx.set_window_appearance(appearance);
}

pub(crate) fn apply(name: &str, window: Option<&mut Window>, cx: &mut App) -> bool {
    let applied = apply_theme(name, window, cx);
    if applied {
        match_window_appearance(name, cx);
        strengthen_overlay(cx);
        quiet_light_comments(cx);
        unify_sql_keywords(cx);
        visible_dark_stripes(cx);
    }
    applied
}

fn apply_theme(name: &str, window: Option<&mut Window>, cx: &mut App) -> bool {
    if name == SYSTEM_THEME {
        reset_configurable_defaults(cx);
        let (light_theme, dark_theme) = {
            let registry = ThemeRegistry::global(cx);
            (
                registry.default_light_theme().clone(),
                registry.default_dark_theme().clone(),
            )
        };
        let theme = Theme::global_mut(cx);
        theme.light_theme = light_theme;
        theme.dark_theme = dark_theme;
        Theme::sync_system_appearance(window, cx);
        return true;
    }

    let Some(config) = ThemeRegistry::global(cx)
        .themes()
        .get(name)
        .filter(|theme| !theme.is_default)
        .cloned()
    else {
        return false;
    };
    reset_configurable_defaults(cx);
    Theme::global_mut(cx).apply_config(&config);
    Theme::change(config.mode, window, cx);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_comment_color_parses_from_hex() {
        let style: Result<gpui_kit::component::highlighter::ThemeStyle, _> =
            serde_json::from_value(serde_json::json!({ "color": "#8c8c8c" }));
        assert!(style.is_ok());
    }

    #[test]
    fn bundled_theme_sets_parse() {
        for source in THEME_SET_SOURCES
            .iter()
            .copied()
            .chain(std::iter::once(QROW_THEME_SET))
        {
            let themes: ThemeSet = serde_json::from_str(source).unwrap();
            assert!(!themes.themes.is_empty());
        }
    }

    #[test]
    fn qrow_themes_keep_their_public_names() {
        let themes: ThemeSet = serde_json::from_str(QROW_THEME_SET).unwrap();
        let names: Vec<_> = themes
            .themes
            .iter()
            .map(|theme| theme.name.as_ref())
            .collect();
        assert_eq!(names, [ONE_DARK_THEME, CM_TWILIGHT_THEME]);
        assert!(themes.themes.iter().all(|theme| theme.mode.is_dark()));
    }

    #[test]
    fn cm_twilight_is_darker_than_one_dark_but_not_black() {
        use std::rc::Rc;

        let themes: ThemeSet = serde_json::from_str(QROW_THEME_SET).unwrap();
        let luminance = |name: &str| {
            let config = themes
                .themes
                .iter()
                .find(|theme| theme.name.as_ref() == name)
                .unwrap();
            let mut theme = Theme::default();
            theme.apply_config(&Rc::new(config.clone()));
            [theme.background, theme.sidebar, theme.title_bar].map(|color| {
                let color = color.to_rgb();
                0.2126 * color.r + 0.7152 * color.g + 0.0722 * color.b
            })
        };
        let one_dark = luminance(ONE_DARK_THEME);
        let cm_twilight = luminance(CM_TWILIGHT_THEME);
        for (twilight, dark) in cm_twilight.iter().zip(one_dark) {
            assert!(
                *twilight < dark,
                "{cm_twilight:?} is not darker than {one_dark:?}"
            );
            assert!(*twilight > 0.05, "{cm_twilight:?} is close to black");
        }
    }

    #[test]
    fn text_selection_shows_on_user_messages() {
        use gpui_kit::component::Colorize as _;
        use std::rc::Rc;

        for source in THEME_SET_SOURCES
            .iter()
            .copied()
            .chain(std::iter::once(QROW_THEME_SET))
        {
            let themes: ThemeSet = serde_json::from_str(source).unwrap();
            for config in themes.themes {
                let name = config.name.clone();
                let mut theme = Theme::default();
                theme.apply_config(&Rc::new(config));
                let colors = theme.semantic_tokens().colors;
                // GPUI Kit's tinted bubble, which the assistant uses for the user's messages.
                let bubble = colors
                    .primary
                    .mix_oklab(colors.background, if theme.is_dark() { 0.24 } else { 0.12 });
                let selected = bubble.blend(colors.selection).to_rgb();
                let bubble = bubble.to_rgb();
                let distance = ((selected.r - bubble.r).powi(2)
                    + (selected.g - bubble.g).powi(2)
                    + (selected.b - bubble.b).powi(2))
                .sqrt();
                assert!(
                    distance > 0.05,
                    "{name}: selection is {distance:.3} from the user message bubble"
                );
            }
        }
    }
}
