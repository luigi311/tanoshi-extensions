use crate::parse::Gallery;
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use url::Url;

const GALLERY_CACHE_CAPACITY: usize = 4;
const GALLERY_CACHE_TTL: Duration = Duration::from_secs(15);
const CDN_CONFIG_CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);

struct CachedGallery {
    url: String,
    gallery: Gallery,
    fetched_at: Instant,
}

#[derive(Default)]
pub(super) struct GalleryCache {
    entries: VecDeque<CachedGallery>,
}

struct CachedCdnConfig {
    image_server: Url,
    fetched_at: Instant,
}

#[derive(Default)]
pub(super) struct CdnConfigCache {
    entry: Option<CachedCdnConfig>,
}

impl CdnConfigCache {
    pub(super) fn get(&mut self, now: Instant) -> Option<Url> {
        let entry = self.entry.as_ref()?;
        if now.saturating_duration_since(entry.fetched_at) > CDN_CONFIG_CACHE_TTL {
            self.entry = None;
            return None;
        }

        Some(entry.image_server.clone())
    }

    pub(super) fn insert(&mut self, image_server: Url, now: Instant) {
        self.entry = Some(CachedCdnConfig {
            image_server,
            fetched_at: now,
        });
    }
}

impl GalleryCache {
    pub(super) fn get(&mut self, url: &str, now: Instant) -> Option<Gallery> {
        let index = self.entries.iter().position(|entry| entry.url == url)?;
        if now.saturating_duration_since(self.entries[index].fetched_at) > GALLERY_CACHE_TTL {
            self.entries.remove(index);
            return None;
        }

        let entry = self.entries.remove(index)?;
        let gallery = entry.gallery.clone();
        self.entries.push_front(entry);
        Some(gallery)
    }

    pub(super) fn insert(&mut self, url: String, gallery: Gallery, now: Instant) {
        self.entries.retain(|entry| entry.url != url);
        self.entries.push_front(CachedGallery {
            url,
            gallery,
            fetched_at: now,
        });
        self.entries.truncate(GALLERY_CACHE_CAPACITY);
    }
}
