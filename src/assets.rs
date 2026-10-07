use std::borrow::Cow;

use gpui_kit::{AssetSource, SharedString};

pub(crate) const SPARK_ICON: &str = "connection-type-icons/apache-spark.svg";
pub(crate) const APP_ICON: &str = "app-icons/qrow-256.png";
#[cfg(test)]
const TRIANGLE_ALERT_ICON: &str = "icons/triangle-alert.svg";
#[cfg(test)]
const CLIPBOARD_PASTE_ICON: &str = "icons/clipboard-paste.svg";
#[cfg(test)]
const STATUS_BAR_ICONS: [&str; 4] = [
    "icons/bot.svg",
    "icons/plug.svg",
    "icons/key-round.svg",
    "icons/activity.svg",
];

// Icons outside the GPUI Kit default set. The assistant tool call cards use
// the icons after `Send`. The status bar uses `Activity`, `Plug`, and
// `KeyRound`, and the Sign-ins sidebar uses `KeyRound` and `ClipboardPaste`.
gpui_kit::assets::icon_assets!(
    QrowIconAssets,
    [
        TriangleAlert,
        Send,
        Pencil,
        CircleStop,
        Activity,
        Table,
        Database,
        Eye,
        RefreshCw,
        ListPlus,
        ScrollText,
        Table2,
        Columns3,
        KeyRound,
        Plug,
        ClipboardPaste,
        FileCode,
    ]
);

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        if path == SPARK_ICON {
            return Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/connection-type-icons/apache-spark.svg"
            ))));
        }
        if path == APP_ICON {
            return Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/app-icons/qrow-256.png"
            ))));
        }
        if let Some(icon) = QrowIconAssets.load(path)? {
            return Ok(Some(icon));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(QrowIconAssets.list(path)?);
        for asset in [SPARK_ICON, APP_ICON] {
            if asset.starts_with(path) {
                paths.push(asset.into());
            }
        }
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::AssetSource;

    #[test]
    fn triangle_alert_asset_is_available() {
        assert!(Assets.load(TRIANGLE_ALERT_ICON).unwrap().is_some());
        assert!(
            Assets
                .list("icons/")
                .unwrap()
                .iter()
                .any(|path| path == TRIANGLE_ALERT_ICON)
        );
    }

    #[test]
    fn status_bar_icons_are_available() {
        for icon in STATUS_BAR_ICONS {
            assert!(Assets.load(icon).unwrap().is_some(), "{icon} is missing");
        }
    }

    #[test]
    fn the_paste_sign_in_icon_is_available() {
        assert!(Assets.load(CLIPBOARD_PASTE_ICON).unwrap().is_some());
    }
}
