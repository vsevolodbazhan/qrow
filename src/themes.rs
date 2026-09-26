#[cfg(test)]
use gpui_kit::component::ThemeSet;
use gpui_kit::component::{Theme, ThemeRegistry};
use gpui_kit::{App, Window};
use qrow::model::SYSTEM_THEME;

#[cfg(test)]
pub(crate) const ONE_DARK_THEME: &str = "One Dark";

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

const ONE_DARK_THEME_SET: &str = concat!(
    r#"{"name":"Qrow","themes":["#,
    include_str!("one-dark.json"),
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
        .chain(std::iter::once(ONE_DARK_THEME_SET))
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

pub(crate) fn apply(name: &str, window: Option<&mut Window>, cx: &mut App) -> bool {
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
    fn bundled_theme_sets_parse() {
        for source in THEME_SET_SOURCES
            .iter()
            .copied()
            .chain(std::iter::once(ONE_DARK_THEME_SET))
        {
            let themes: ThemeSet = serde_json::from_str(source).unwrap();
            assert!(!themes.themes.is_empty());
        }
    }

    #[test]
    fn one_dark_keeps_its_public_name() {
        let themes: ThemeSet = serde_json::from_str(ONE_DARK_THEME_SET).unwrap();
        assert_eq!(themes.themes[0].name.as_ref(), ONE_DARK_THEME);
        assert!(themes.themes[0].mode.is_dark());
    }
}
