use std::borrow::Cow;

use gpui_kit::{AssetSource, SharedString};

pub(crate) const SPARK_ICON: &str = "connection-type-icons/apache-spark.svg";
pub(crate) const APP_ICON: &str = "app-icons/qrow-256.png";
const TRIANGLE_ALERT_ICON: &str = "icons/triangle-alert.svg";
const SPARKLES_ICON: &str = "icons/sparkles.svg";
const SEND_ICON: &str = "icons/send.svg";

gpui_kit::assets::icon_assets!(QrowIconAssets, [TriangleAlert, Sparkles, Send]);

pub(crate) struct Assets;

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
        if path == TRIANGLE_ALERT_ICON || path == SPARKLES_ICON || path == SEND_ICON {
            return QrowIconAssets.load(path);
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        for asset in [
            SPARK_ICON,
            APP_ICON,
            TRIANGLE_ALERT_ICON,
            SPARKLES_ICON,
            SEND_ICON,
        ] {
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
    fn assistant_toggle_icon_is_available() {
        assert!(Assets.load(SPARKLES_ICON).unwrap().is_some());
    }
}
