use std::{
    cell::{RefCell, RefMut},
    collections::HashMap,
    env, fs,
    io::Cursor,
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use image::{DynamicImage, GenericImageView, ImageReader, imageops::FilterType};
use ratatui_image::{
    picker::{Picker, ProtocolType},
    protocol::StatefulProtocol,
};
use tracing::debug;

const IMAGE_PREVIEW_TIMEOUT: Duration = Duration::from_secs(12);
const IMAGE_PREVIEW_MAX_BYTES: u64 = 12 * 1024 * 1024;
const IMAGE_PREVIEW_MAX_DECODED_EDGE: u32 = 960;
const IMAGE_PREVIEW_PROTOCOL_ENV: &str = "GHR_IMAGE_PREVIEW_PROTOCOL";

pub(super) struct ImagePreviewCache {
    enabled: bool,
    picker: Picker,
    entries: RefCell<HashMap<String, ImagePreviewEntry>>,
}

pub(super) struct ImagePreviewData {
    image: DynamicImage,
    width: u32,
    height: u32,
}

pub(super) struct LoadedImagePreview {
    protocol: StatefulProtocol,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum ImagePreviewBackend {
    Auto,
    Disabled,
    Halfblocks,
    Native(ProtocolType),
    QueryNative,
}

enum ImagePreviewEntry {
    Loading,
    Loaded(LoadedImagePreview),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ImagePreviewStatus {
    Disabled,
    Queued,
    Loading,
    Loaded { width: u32, height: u32 },
    Error(String),
}

impl Default for ImagePreviewCache {
    fn default() -> Self {
        Self {
            enabled: false,
            picker: Picker::halfblocks(),
            entries: RefCell::new(HashMap::new()),
        }
    }
}

impl ImagePreviewCache {
    pub(super) fn enable_from_environment(&mut self) -> Result<ImagePreviewBackend> {
        match image_preview_backend_from_env(env::var(IMAGE_PREVIEW_PROTOCOL_ENV).ok().as_deref()) {
            ImagePreviewBackend::Auto => {
                if let Some(protocol) = native_protocol_from_env() {
                    self.enable_native_protocol(protocol);
                    Ok(ImagePreviewBackend::Native(protocol))
                } else {
                    self.enable_halfblocks();
                    Ok(ImagePreviewBackend::Halfblocks)
                }
            }
            ImagePreviewBackend::Disabled => {
                self.enabled = false;
                self.entries.borrow_mut().clear();
                Ok(ImagePreviewBackend::Disabled)
            }
            ImagePreviewBackend::Halfblocks => {
                self.enable_halfblocks();
                Ok(ImagePreviewBackend::Halfblocks)
            }
            ImagePreviewBackend::Native(protocol) => {
                self.enable_native_protocol(protocol);
                Ok(ImagePreviewBackend::Native(protocol))
            }
            ImagePreviewBackend::QueryNative => {
                self.enable_from_terminal()?;
                Ok(ImagePreviewBackend::QueryNative)
            }
        }
    }

    fn enable_from_terminal(&mut self) -> Result<()> {
        self.picker =
            Picker::from_query_stdio().context("failed to query terminal image support")?;
        self.enabled = true;
        Ok(())
    }

    fn enable_native_protocol(&mut self, protocol: ProtocolType) {
        let mut picker = match Picker::from_query_stdio() {
            Ok(picker) => picker,
            Err(error) => {
                debug!(
                    error = %error,
                    "failed to query terminal image sizing; using default cell size for native image previews"
                );
                Picker::halfblocks()
            }
        };
        picker.set_protocol_type(protocol);
        self.picker = picker;
        self.enabled = true;
    }

    pub(super) fn enable_halfblocks(&mut self) {
        self.picker = Picker::halfblocks();
        self.enabled = true;
    }

    pub(super) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(super) fn status(&self, url: &str) -> ImagePreviewStatus {
        if !self.enabled {
            return ImagePreviewStatus::Disabled;
        }

        match self.entries.borrow().get(url) {
            Some(ImagePreviewEntry::Loading) => ImagePreviewStatus::Loading,
            Some(ImagePreviewEntry::Loaded(preview)) => ImagePreviewStatus::Loaded {
                width: preview.width,
                height: preview.height,
            },
            Some(ImagePreviewEntry::Error(error)) => ImagePreviewStatus::Error(error.clone()),
            None => ImagePreviewStatus::Queued,
        }
    }

    pub(super) fn start_loading(&self, url: &str) -> bool {
        if !self.enabled {
            return false;
        }

        let mut entries = self.entries.borrow_mut();
        if entries.contains_key(url) {
            return false;
        }
        entries.insert(url.to_string(), ImagePreviewEntry::Loading);
        true
    }

    pub(super) fn finish_loading(
        &self,
        url: String,
        result: std::result::Result<ImagePreviewData, String>,
    ) {
        let entry = match result {
            Ok(data) => {
                let width = data.width;
                let height = data.height;
                ImagePreviewEntry::Loaded(LoadedImagePreview {
                    protocol: self.picker.new_resize_protocol(data.image),
                    width,
                    height,
                })
            }
            Err(error) => ImagePreviewEntry::Error(error),
        };
        self.entries.borrow_mut().insert(url, entry);
    }

    pub(super) fn loaded_protocol_mut(&self, url: &str) -> Option<RefMut<'_, StatefulProtocol>> {
        {
            let entries = self.entries.borrow();
            if !matches!(entries.get(url), Some(ImagePreviewEntry::Loaded(_))) {
                return None;
            }
        }

        Some(RefMut::map(self.entries.borrow_mut(), |entries| {
            let Some(ImagePreviewEntry::Loaded(preview)) = entries.get_mut(url) else {
                unreachable!("loaded image preview checked before borrowing");
            };
            &mut preview.protocol
        }))
    }
}

fn image_preview_backend_from_env(value: Option<&str>) -> ImagePreviewBackend {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return ImagePreviewBackend::Auto;
    };

    match value.to_ascii_lowercase().as_str() {
        "auto" => ImagePreviewBackend::Auto,
        "native" | "terminal" | "query" => ImagePreviewBackend::QueryNative,
        "iterm" | "iterm2" | "iip" | "inline" => ImagePreviewBackend::Native(ProtocolType::Iterm2),
        "kitty" | "kgp" => ImagePreviewBackend::Native(ProtocolType::Kitty),
        "sixel" => ImagePreviewBackend::Native(ProtocolType::Sixel),
        "off" | "false" | "0" | "disabled" | "none" => ImagePreviewBackend::Disabled,
        "halfblocks" | "halfblock" | "blocks" | "block" => ImagePreviewBackend::Halfblocks,
        _ => ImagePreviewBackend::Auto,
    }
}

fn native_protocol_from_env() -> Option<ProtocolType> {
    let term = env::var("TERM").unwrap_or_default();
    let term_program = env::var("TERM_PROGRAM").unwrap_or_default();

    if env::var("WARP_HONOR_PS1").is_ok_and(|value| !value.is_empty())
        || term_program == "WarpTerminal"
    {
        return Some(ProtocolType::Iterm2);
    }
    if env::var("ITERM_SESSION_ID").is_ok_and(|value| !value.is_empty())
        || term_program.contains("iTerm")
        || env::var("LC_TERMINAL").is_ok_and(|value| value.contains("iTerm"))
    {
        return Some(ProtocolType::Iterm2);
    }
    if env::var("WEZTERM_EXECUTABLE").is_ok_and(|value| !value.is_empty())
        || term_program == "WezTerm"
    {
        return Some(ProtocolType::Iterm2);
    }
    if env::var("VSCODE_INJECTION").is_ok_and(|value| !value.is_empty()) || term_program == "vscode"
    {
        return Some(ProtocolType::Iterm2);
    }
    if env::var("TABBY_CONFIG_DIRECTORY").is_ok_and(|value| !value.is_empty())
        || term_program == "Tabby"
        || term_program == "Hyper"
        || term_program == "mintty"
        || term_program == "Bobcat"
    {
        return Some(ProtocolType::Iterm2);
    }
    if env::var("GHOSTTY_RESOURCES_DIR").is_ok_and(|value| !value.is_empty())
        || term == "xterm-ghostty"
        || term_program == "ghostty"
    {
        return Some(ProtocolType::Kitty);
    }
    if env::var("KITTY_WINDOW_ID").is_ok_and(|value| !value.is_empty()) || term == "xterm-kitty" {
        return Some(ProtocolType::Kitty);
    }
    if term == "foot" || term == "foot-extra" || env::var("WT_SESSION").is_ok() {
        return Some(ProtocolType::Sixel);
    }

    None
}

pub(super) async fn download_image_preview(url: &str) -> Result<ImagePreviewData> {
    let parsed = reqwest::Url::parse(url).with_context(|| format!("invalid image URL: {url}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        bail!("image URL must use http or https");
    }

    if let Some(bytes) = read_image_preview_cache(url).await
        && let Ok(data) = decode_image_preview(bytes).await
    {
        return Ok(data);
    }

    let client = reqwest::Client::builder()
        .timeout(IMAGE_PREVIEW_TIMEOUT)
        .user_agent(concat!("ghr/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("failed to build image preview HTTP client")?;
    let response = client
        .get(parsed)
        .send()
        .await
        .context("failed to download image")?
        .error_for_status()
        .context("image request returned an error")?;

    if let Some(length) = response.content_length()
        && length > IMAGE_PREVIEW_MAX_BYTES
    {
        bail!("image is too large for preview");
    }

    let bytes = response
        .bytes()
        .await
        .context("failed to read image body")?;
    if bytes.len() as u64 > IMAGE_PREVIEW_MAX_BYTES {
        bail!("image is too large for preview");
    }

    let bytes = bytes.to_vec();
    write_image_preview_cache(url, bytes.clone()).await;
    decode_image_preview(bytes).await
}

async fn decode_image_preview(bytes: Vec<u8>) -> Result<ImagePreviewData> {
    tokio::task::spawn_blocking(move || {
        if bytes.len() as u64 > IMAGE_PREVIEW_MAX_BYTES {
            bail!("image is too large for preview");
        }

        let image = ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .context("failed to detect image format")?
            .decode()
            .context("failed to decode image")?;
        let (width, height) = image.dimensions();
        let image = downscale_preview_image(image);
        Ok(ImagePreviewData {
            image,
            width,
            height,
        })
    })
    .await
    .context("image preview decode task failed")?
}

fn downscale_preview_image(image: DynamicImage) -> DynamicImage {
    let (width, height) = image.dimensions();
    if width <= IMAGE_PREVIEW_MAX_DECODED_EDGE && height <= IMAGE_PREVIEW_MAX_DECODED_EDGE {
        return image;
    }

    image.resize(
        IMAGE_PREVIEW_MAX_DECODED_EDGE,
        IMAGE_PREVIEW_MAX_DECODED_EDGE,
        FilterType::Triangle,
    )
}

async fn read_image_preview_cache(url: &str) -> Option<Vec<u8>> {
    let path = image_preview_cache_path(url)?;
    tokio::task::spawn_blocking(move || {
        let bytes = fs::read(path).ok()?;
        (bytes.len() as u64 <= IMAGE_PREVIEW_MAX_BYTES).then_some(bytes)
    })
    .await
    .ok()
    .flatten()
}

async fn write_image_preview_cache(url: &str, bytes: Vec<u8>) {
    let Some(path) = image_preview_cache_path(url) else {
        return;
    };
    let _ = tokio::task::spawn_blocking(move || -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::write(&path, bytes).with_context(|| format!("failed to write {}", path.display()))?;
        Ok(())
    })
    .await;
}

fn image_preview_cache_path(url: &str) -> Option<PathBuf> {
    let root = dirs::home_dir()?.join(".ghr").join("image-cache");
    let digest = format!("{:x}", md5::compute(url.as_bytes()));
    Some(root.join(digest))
}

#[cfg(test)]
pub(super) fn tiny_test_image() -> ImagePreviewData {
    let image = DynamicImage::new_rgba8(2, 2);
    ImagePreviewData {
        image,
        width: 2,
        height: 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_preview_backend_defaults_to_auto() {
        assert_eq!(
            image_preview_backend_from_env(None),
            ImagePreviewBackend::Auto
        );
        assert_eq!(
            image_preview_backend_from_env(Some("")),
            ImagePreviewBackend::Auto
        );
        assert_eq!(
            image_preview_backend_from_env(Some("unknown")),
            ImagePreviewBackend::Auto
        );
    }

    #[test]
    fn image_preview_backend_can_be_forced() {
        assert_eq!(
            image_preview_backend_from_env(Some("native")),
            ImagePreviewBackend::QueryNative
        );
        assert_eq!(
            image_preview_backend_from_env(Some("AUTO")),
            ImagePreviewBackend::Auto
        );
        assert_eq!(
            image_preview_backend_from_env(Some("iterm2")),
            ImagePreviewBackend::Native(ProtocolType::Iterm2)
        );
        assert_eq!(
            image_preview_backend_from_env(Some("kitty")),
            ImagePreviewBackend::Native(ProtocolType::Kitty)
        );
        assert_eq!(
            image_preview_backend_from_env(Some("off")),
            ImagePreviewBackend::Disabled
        );
        assert_eq!(
            image_preview_backend_from_env(Some("blocks")),
            ImagePreviewBackend::Halfblocks
        );
    }

    #[test]
    fn downscale_preview_image_caps_large_sources() {
        let image = DynamicImage::new_rgba8(2400, 1200);
        let resized = downscale_preview_image(image);

        assert!(resized.width() <= IMAGE_PREVIEW_MAX_DECODED_EDGE);
        assert!(resized.height() <= IMAGE_PREVIEW_MAX_DECODED_EDGE);
    }
}
