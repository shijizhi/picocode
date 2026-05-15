use std::{
    env, fs, io,
    io::Cursor,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use arboard::Clipboard;
use image::{DynamicImage, ImageFormat, RgbaImage};

const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageAttachment {
    pub source_path: String,
    pub file_name: String,
    pub mime_type: String,
    pub byte_len: usize,
    pub data_url: String,
}

pub fn attach_image(path: impl AsRef<Path>) -> io::Result<ImageAttachment> {
    let path = path.as_ref();
    let bytes = fs::read(path)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| path.display().to_string());

    image_attachment_from_bytes(display_path(path), file_name, path, bytes)
}

pub fn attach_image_from_clipboard() -> io::Result<ImageAttachment> {
    let output_path = clipboard_output_path();
    let result = (|| {
        paste_image_to_temp_png(&output_path)?;
        let attachment = attach_image(&output_path)?;
        Ok(ImageAttachment {
            source_path: "clipboard".to_owned(),
            file_name: "clipboard.png".to_owned(),
            ..attachment
        })
    })();
    let _ = fs::remove_file(&output_path);
    result
}

pub fn clipboard_text() -> io::Result<String> {
    let output = std::process::Command::new("pbpaste").output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(io::Error::new(
            io::ErrorKind::Other,
            if stderr.is_empty() {
                "failed to read clipboard text".to_owned()
            } else {
                format!("failed to read clipboard text: {stderr}")
            },
        ))
    }
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn image_attachment_from_bytes(
    source_path: String,
    file_name: String,
    path: &Path,
    bytes: Vec<u8>,
) -> io::Result<ImageAttachment> {
    if bytes.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("image file is empty: {}", path.display()),
        ));
    }
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "image is too large: {} bytes exceeds {} bytes",
                bytes.len(),
                MAX_IMAGE_BYTES
            ),
        ));
    }

    let mime_type = guess_image_mime(path, &bytes).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "unsupported image format: {}",
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .unwrap_or("<unknown>")
            ),
        )
    })?;

    Ok(ImageAttachment {
        source_path,
        file_name,
        mime_type: mime_type.to_owned(),
        byte_len: bytes.len(),
        data_url: format!("data:{};base64,{}", mime_type, base64_encode(&bytes)),
    })
}

fn clipboard_output_path() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    env::temp_dir().join(format!(
        "picocode-clipboard-image-{}-{nonce}.png",
        std::process::id()
    ))
}

fn paste_image_to_temp_png(output_path: &Path) -> io::Result<()> {
    let mut clipboard = Clipboard::new().map_err(|error| io::Error::other(error.to_string()))?;

    let files = clipboard
        .get()
        .file_list()
        .map_err(|error| io::Error::other(error.to_string()))
        .unwrap_or_default();
    if let Some(image) = files.into_iter().find_map(|path| image::open(path).ok()) {
        write_dynamic_image_as_png(output_path, image)?;
        return Ok(());
    }

    let image = clipboard
        .get_image()
        .map_err(|error| io::Error::other(error.to_string()))?;
    let width = image.width as u32;
    let height = image.height as u32;
    let Some(rgba_image) = RgbaImage::from_raw(width, height, image.bytes.into_owned()) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid RGBA clipboard image buffer",
        ));
    };
    write_dynamic_image_as_png(output_path, DynamicImage::ImageRgba8(rgba_image))
}

fn write_dynamic_image_as_png(output_path: &Path, image: DynamicImage) -> io::Result<()> {
    let mut png = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .map_err(|error| io::Error::other(error.to_string()))?;
    fs::write(output_path, png)
}

fn guess_image_mime(path: &Path, bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.starts_with(b"BM") {
        return Some("image/bmp");
    }

    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_lowercase())
        .as_deref()
    {
        Some("png") => Some("image/png"),
        Some("jpg") | Some("jpeg") => Some("image/jpeg"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        Some("bmp") => Some("image/bmp"),
        Some("svg") => Some("image/svg+xml"),
        _ => None,
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut encoded = String::with_capacity(((bytes.len() + 2) / 3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = *chunk.get(1).unwrap_or(&0);
        let third = *chunk.get(2).unwrap_or(&0);

        let index_0 = first >> 2;
        let index_1 = ((first & 0b0000_0011) << 4) | (second >> 4);
        let index_2 = ((second & 0b0000_1111) << 2) | (third >> 6);
        let index_3 = third & 0b0011_1111;

        encoded.push(TABLE[index_0 as usize] as char);
        encoded.push(TABLE[index_1 as usize] as char);
        if chunk.len() > 1 {
            encoded.push(TABLE[index_2 as usize] as char);
        } else {
            encoded.push('=');
        }
        if chunk.len() > 2 {
            encoded.push(TABLE[index_3 as usize] as char);
        } else {
            encoded.push('=');
        }
    }
    encoded
}
