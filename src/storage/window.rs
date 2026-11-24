//! Machine-local main window geometry persistence (`window.json`).
//!
//! Window origin, size, and maximized state belong to the current computer and
//! monitor layout rather than roaming workspace data. On startup, saved bounds
//! are validated against connected displays so disconnected monitors fall back
//! safely to the default centered size.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Minimum valid main window width, matching `window_min_size` in `src/main.rs`.
pub const MIN_WINDOW_WIDTH: f32 = 850.;
/// Minimum valid main window height, matching `window_min_size` in `src/main.rs`.
pub const MIN_WINDOW_HEIGHT: f32 = 560.;
/// Default main window width when no saved geometry is usable.
pub const DEFAULT_WINDOW_WIDTH: f32 = 1280.;
/// Default main window height when no saved geometry is usable.
pub const DEFAULT_WINDOW_HEIGHT: f32 = 820.;

/// Minimum visible intersection width and height required on a connected
/// display before restoring saved coordinates.
const MIN_VISIBLE_WIDTH: f32 = 120.;
const MIN_VISIBLE_HEIGHT: f32 = 80.;
/// Title bar margin: the top edge of the restored window must remain reachable.
const TITLE_BAR_BAND: f32 = 40.;

/// Machine-local saved window bounds stored in `<local_data_dir>/window.json`.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct SavedWindowBounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub maximized: bool,
}

/// Path to the machine-local window geometry file inside `local_dir`.
pub fn window_state_path(local_dir: &Path) -> PathBuf {
    local_dir.join("window.json")
}

/// Loads saved window bounds if the file exists and parses cleanly.
/// Missing or corrupt files return `None` so startup falls back to defaults.
pub fn load_window_bounds(path: &Path) -> Option<SavedWindowBounds> {
    let bytes = fs::read(path).ok()?;
    let bounds: SavedWindowBounds = serde_json::from_slice(&bytes).ok()?;
    bounds.is_finite_and_sized().then_some(bounds)
}

/// Persists `bounds` atomically to `path`, creating parent folders as needed.
pub fn save_window_bounds(path: &Path, bounds: SavedWindowBounds) -> Result<()> {
    if !bounds.is_finite_and_sized() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_vec_pretty(&bounds)?;
    super::write_atomically(path, &data)
}

impl SavedWindowBounds {
    /// Returns whether every coordinate is finite and the dimensions meet the
    /// minimum window size.
    pub fn is_finite_and_sized(self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.width.is_finite()
            && self.height.is_finite()
            && self.width >= MIN_WINDOW_WIDTH
            && self.height >= MIN_WINDOW_HEIGHT
    }

    /// Returns whether this rectangle is safely visible on at least one of the
    /// connected displays `(x, y, width, height)`.
    pub fn fits_any_display(self, displays: &[(f32, f32, f32, f32)]) -> bool {
        if !self.is_finite_and_sized() || displays.is_empty() {
            return false;
        }
        let right = self.x + self.width;
        let bottom = self.y + self.height;
        displays.iter().any(|&(dx, dy, dw, dh)| {
            if !dx.is_finite()
                || !dy.is_finite()
                || !dw.is_finite()
                || !dh.is_finite()
                || dw <= 0.
                || dh <= 0.
            {
                return false;
            }
            let d_right = dx + dw;
            let d_bottom = dy + dh;
            let overlap_w = (right.min(d_right) - self.x.max(dx)).max(0.);
            let overlap_h = (bottom.min(d_bottom) - self.y.max(dy)).max(0.);
            let title_reachable = self.y >= dy - 8. && self.y + TITLE_BAR_BAND <= d_bottom;
            overlap_w >= MIN_VISIBLE_WIDTH && overlap_h >= MIN_VISIBLE_HEIGHT && title_reachable
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_bounds_round_trip_and_validate_displays() {
        let dir = tempfile::tempdir().unwrap();
        let path = window_state_path(dir.path());
        assert_eq!(load_window_bounds(&path), None);

        let bounds = SavedWindowBounds {
            x: 120.,
            y: 80.,
            width: 1400.,
            height: 900.,
            maximized: false,
        };
        save_window_bounds(&path, bounds).unwrap();
        assert_eq!(load_window_bounds(&path), Some(bounds));

        // Fits primary 1920x1080 display.
        assert!(bounds.fits_any_display(&[(0., 0., 1920., 1080.)]));
        // Disconnected second monitor on the right (x=2500) is rejected when only primary exists.
        let offscreen = SavedWindowBounds {
            x: 2500.,
            y: 80.,
            width: 1280.,
            height: 820.,
            maximized: false,
        };
        assert!(!offscreen.fits_any_display(&[(0., 0., 1920., 1080.)]));
        // Title bar stranded above top edge is rejected.
        let top_cut = SavedWindowBounds {
            x: 100.,
            y: -200.,
            width: 1280.,
            height: 820.,
            maximized: false,
        };
        assert!(!top_cut.fits_any_display(&[(0., 0., 1920., 1080.)]));
        // Below minimum size is rejected.
        let too_small = SavedWindowBounds {
            x: 100.,
            y: 100.,
            width: 500.,
            height: 400.,
            maximized: false,
        };
        assert!(!too_small.fits_any_display(&[(0., 0., 1920., 1080.)]));
    }
}
