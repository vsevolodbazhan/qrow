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
