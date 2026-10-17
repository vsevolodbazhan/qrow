//! CPU ownership of atlas allocations; GL uploads run only with a current context.
use anyhow::{Result, anyhow, ensure};
use gpui::{
    AtlasBackend, AtlasKey, AtlasState, AtlasTextureId, AtlasTextureKind, AtlasTile, Bounds,
    DevicePixels, PlatformAtlas, Size, TileId, point,
};
use parking_lot::Mutex;
use std::{borrow::Cow, collections::HashMap};

pub(super) const PAGE_SIZE: i32 = 2048;
const MAX_PAGES: usize = 8;

pub(super) struct Upload {
    pub page: u32,
    pub origin: [i32; 2],
    pub size: [i32; 2],
    pub pixels: Vec<u8>,
}

struct Page {
    kind: AtlasTextureKind,
    allocator: etagere::AtlasAllocator,
}

#[derive(Default)]
pub(super) struct Pixels {
    pages: Vec<Page>,
    allocations: HashMap<u32, (usize, etagere::AllocId)>,
    next_id: u32,
    uploads: Vec<Upload>,
}

impl AtlasBackend for Pixels {
    fn insert(
        &mut self,
        kind: AtlasTextureKind,
        dimensions: Size<DevicePixels>,
        bytes: &[u8],
    ) -> Result<AtlasTile> {
        let (w, h) = (dimensions.width.0, dimensions.height.0);
        ensure!(
            w > 0 && h > 0 && w <= PAGE_SIZE - 2 && h <= PAGE_SIZE - 2,
            "OpenGL atlas tile exceeds page size"
        );
        let channels = if kind == AtlasTextureKind::Monochrome {
            1
        } else {
            4
        };
        ensure!(
            bytes.len() == w as usize * h as usize * channels,
            "invalid OpenGL atlas pixel count"
        );
        let id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| anyhow!("OpenGL tile id exhausted"))?;
        let requested = etagere::size2(w + 2, h + 2);
        let mut found = None;
        for (ix, page) in self.pages.iter_mut().enumerate() {
            if page.kind == kind {
                if let Some(allocation) = page.allocator.allocate(requested) {
                    found = Some((ix, allocation));
                    break;
                }
            }
        }
        let (ix, allocation) = match found {
            Some(found) => found,
            None => {
                ensure!(
                    self.pages.len() < MAX_PAGES,
                    "OpenGL atlas memory limit reached (128 MiB)"
                );
                let mut page = Page {
                    kind,
                    allocator: etagere::AtlasAllocator::new(etagere::size2(PAGE_SIZE, PAGE_SIZE)),
                };
                let allocation = page
                    .allocator
                    .allocate(requested)
                    .ok_or_else(|| anyhow!("OpenGL atlas allocation failed"))?;
                let ix = self.pages.len();
                self.pages.push(page);
                (ix, allocation)
            }
        };
        self.next_id = id;
        self.allocations.insert(id, (ix, allocation.id));
        let origin = allocation.rectangle.min;
        let mut pixels = vec![0; (w + 2) as usize * (h + 2) as usize * 4];
        for y in 0..h as usize {
            for x in 0..w as usize {
                let source = (y * w as usize + x) * channels;
                let target = ((y + 1) * (w + 2) as usize + x + 1) * 4;
                let color = match kind {
                    AtlasTextureKind::Monochrome => [255, 255, 255, bytes[source]],
                    // Linear sampling must interpolate premultiplied colors.
                    AtlasTextureKind::Polychrome => {
                        let alpha = bytes[source + 3];
                        let premul = |c: u8| ((c as u16 * alpha as u16 + 127) / 255) as u8;
                        [
                            premul(bytes[source + 2]),
                            premul(bytes[source + 1]),
                            premul(bytes[source]),
                            alpha,
                        ]
                    }
                    AtlasTextureKind::Subpixel => [
                        255,
                        255,
                        255,
                        ((bytes[source] as u16
                            + bytes[source + 1] as u16
                            + bytes[source + 2] as u16)
                            / 3) as u8,
                    ],
                };
                pixels[target..target + 4].copy_from_slice(&color);
            }
        }
        if kind == AtlasTextureKind::Polychrome {
            // Extend image edges into the padding so enlargement does not
            // sample a transparent border from the next atlas allocation.
            for y in 0..(h + 2) as usize {
                for x in 0..(w + 2) as usize {
                    if x == 0 || y == 0 || x == (w + 1) as usize || y == (h + 1) as usize {
                        let source = (y.clamp(1, h as usize) * (w + 2) as usize
                            + x.clamp(1, w as usize))
                            * 4;
                        let target = (y * (w + 2) as usize + x) * 4;
                        let color: [u8; 4] = pixels[source..source + 4].try_into().unwrap();
                        pixels[target..target + 4].copy_from_slice(&color);
                    }
                }
            }
        }
        self.uploads.push(Upload {
            page: ix as u32,
            origin: [origin.x, origin.y],
            size: [w + 2, h + 2],
            pixels,
        });
        Ok(AtlasTile {
            texture_id: AtlasTextureId {
                index: ix as u32,
                kind,
            },
            tile_id: TileId(id),
            padding: 0,
            bounds: Bounds {
                origin: point(DevicePixels(origin.x + 1), DevicePixels(origin.y + 1)),
                size: dimensions,
            },
        })
    }
    fn remove(&mut self, tile: AtlasTile) {
        if let Some((page, allocation)) = self.allocations.remove(&tile.tile_id.0) {
            self.pages[page].allocator.deallocate(allocation);
        }
    }
}

#[derive(Default)]
pub(crate) struct OpenGlAtlas(Mutex<AtlasState<Pixels>>);

impl OpenGlAtlas {
    #[cfg(test)]
    pub(super) fn test_insert(
        &self,
        kind: AtlasTextureKind,
        size: Size<DevicePixels>,
        bytes: &[u8],
    ) -> Result<AtlasTile> {
        self.0.lock().backend.insert(kind, size, bytes)
    }
    pub(super) fn take_uploads(&self) -> Vec<Upload> {
        std::mem::take(&mut self.0.lock().backend.uploads)
    }
}

impl PlatformAtlas for OpenGlAtlas {
    fn get_or_insert_with<'a>(
        &self,
        key: AtlasKey,
        build: &mut dyn FnMut() -> Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> Result<Option<AtlasTile>> {
        self.0.lock().get_or_insert_with(key, build)
    }
    fn remove(&self, key: &AtlasKey) {
        self.0.lock().remove(key);
    }
    #[cfg(any(test, feature = "test-support", feature = "bench-support"))]
    fn contains(&self, key: &AtlasKey) -> bool {
        self.0.lock().contains(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;
    #[test]
    fn tiles_share_pages_reuse_released_space_and_preserve_image_channels() {
        let mut atlas = Pixels::default();
        let a = atlas
            .insert(
                AtlasTextureKind::Polychrome,
                size(DevicePixels(1), DevicePixels(1)),
                &[12, 34, 56, 128],
            )
            .unwrap();
        let b = atlas
            .insert(
                AtlasTextureKind::Polychrome,
                size(DevicePixels(1), DevicePixels(1)),
                &[0, 0, 0, 0],
            )
            .unwrap();
        assert_eq!(a.texture_id, b.texture_id);
        assert_ne!(a.bounds.origin, b.bounds.origin);
        assert_eq!(&atlas.uploads[0].pixels[16..20], &[28, 17, 6, 128]);
        atlas.remove(a);
        atlas.remove(b);
        assert!(atlas.allocations.is_empty());
        assert!(atlas.pages[0].allocator.is_empty());
        assert!(
            atlas
                .insert(
                    AtlasTextureKind::Monochrome,
                    size(DevicePixels(0), DevicePixels(1)),
                    &[]
                )
                .is_err()
        );
    }
}
