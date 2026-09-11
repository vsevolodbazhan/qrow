use std::borrow::Cow;

use gpui_kit::{AssetSource, SharedString};

pub(crate) const SPARK_ICON: &str = "icons/apache-spark.svg";

pub(crate) struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        if path == SPARK_ICON {
            return Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/apache-spark.svg"
            ))));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        if SPARK_ICON.starts_with(path) {
            paths.push(SPARK_ICON.into());
        }
        Ok(paths)
    }
}
