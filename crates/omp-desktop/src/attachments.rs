//! Images attached to the composer: pasted bitmaps or image files, sent as
//! `images` on the next prompt. omp converts formats and resizes for the
//! model, so the bytes go out as they came in. Free of GPUI rendering so it
//! can be tested directly.

use std::path::Path;
use std::sync::Arc;

use gpui_kit::{ClipboardEntry, ClipboardItem, Image, ImageFormat};
use omp_rpc::ImageContent;

/// Larger images are refused rather than sent: one frame carries the whole
/// prompt, and providers reject payloads far below this anyway.
pub const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Attachment {
	/// The decoded image, shared with the thumbnail in the composer.
	pub image: Arc<Image>,
	/// File name for pasted files; `None` for a pasted bitmap.
	pub name:  Option<String>,
}

impl Attachment {
	pub fn mime_type(&self) -> &'static str {
		self.image.format.mime_type()
	}

	pub fn label(&self) -> String {
		self.name.clone().unwrap_or_else(|| format!("图片.{}", self.image.format.extension()))
	}

	pub fn to_wire(&self) -> ImageContent {
		ImageContent {
			r#type: Some(omp_rpc::LitImage),
			data: Some(encode_base64(&self.image.bytes)),
			mime_type: Some(self.mime_type().to_owned()),
			..Default::default()
		}
	}
}

/// What a paste carried, beyond plain text.
#[derive(Debug, Default)]
pub struct Pasted {
	pub images:   Vec<Attachment>,
	/// Files that were not images, or images over [`MAX_IMAGE_BYTES`].
	pub rejected: Vec<String>,
}

impl Pasted {
	pub fn is_empty(&self) -> bool {
		self.images.is_empty() && self.rejected.is_empty()
	}
}

/// Image format for a file name, by extension.
pub fn format_for_path(path: &Path) -> Option<ImageFormat> {
	let ext = path.extension()?.to_str()?.to_ascii_lowercase();
	Some(match ext.as_str() {
		"png" => ImageFormat::Png,
		"jpg" | "jpeg" => ImageFormat::Jpeg,
		"webp" => ImageFormat::Webp,
		"gif" => ImageFormat::Gif,
		"bmp" => ImageFormat::Bmp,
		"tif" | "tiff" => ImageFormat::Tiff,
		_ => return None,
	})
}

fn file_name(path: &Path) -> String {
	path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// Reads the image files among `paths`; others go to `rejected`.
pub fn from_paths(paths: &[std::path::PathBuf]) -> Pasted {
	let mut pasted = Pasted::default();
	for path in paths {
		let name = file_name(path);
		let Some(format) = format_for_path(path) else {
			pasted.rejected.push(format!("{name}（不是图片）"));
			continue;
		};
		match std::fs::read(path) {
			Ok(bytes) if bytes.len() > MAX_IMAGE_BYTES => pasted.rejected.push(format!("{name}（超过 20 MB）")),
			Ok(bytes) => pasted.images.push(Attachment { image: Arc::new(Image::from_bytes(format, bytes)), name: Some(name) }),
			Err(error) => pasted.rejected.push(format!("{name}（{error}）")),
		}
	}
	pasted
}

/// The images in a clipboard item: bitmaps, or image files copied in Finder.
/// SVG is skipped: providers take raster images only.
pub fn from_clipboard(item: &ClipboardItem) -> Pasted {
	let mut pasted = Pasted::default();
	for entry in item.entries() {
		match entry {
			ClipboardEntry::Image(image) if image.format == ImageFormat::Svg => {
				pasted.rejected.push("SVG 图片（不支持）".into());
			},
			ClipboardEntry::Image(image) if image.bytes.len() > MAX_IMAGE_BYTES => {
				pasted.rejected.push("剪贴板图片（超过 20 MB）".into());
			},
			ClipboardEntry::Image(image) => pasted.images.push(Attachment { image: Arc::new(image.clone()), name: None }),
			ClipboardEntry::ExternalPaths(paths) => {
				let files = from_paths(paths.paths());
				pasted.images.extend(files.images);
				pasted.rejected.extend(files.rejected);
			},
			ClipboardEntry::String(_) => {},
		}
	}
	pasted
}

/// Decodes a history image for display; `None` for unknown formats or bad data.
pub fn decode(mime_type: &str, data: &str) -> Option<Arc<Image>> {
	let format = ImageFormat::from_mime_type(mime_type)?;
	let bytes = decode_base64(data)?;
	Some(Arc::new(Image::from_bytes(format, bytes)))
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode_base64(bytes: &[u8]) -> String {
	let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
	for chunk in bytes.chunks(3) {
		let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
		out.push(ALPHABET[(n >> 18) as usize & 63] as char);
		out.push(ALPHABET[(n >> 12) as usize & 63] as char);
		out.push(if chunk.len() > 1 { ALPHABET[(n >> 6) as usize & 63] as char } else { '=' });
		out.push(if chunk.len() > 2 { ALPHABET[n as usize & 63] as char } else { '=' });
	}
	out
}

pub fn decode_base64(text: &str) -> Option<Vec<u8>> {
	let value = |c: u8| -> Option<u32> {
		Some(match c {
			b'A'..=b'Z' => c - b'A',
			b'a'..=b'z' => c - b'a' + 26,
			b'0'..=b'9' => c - b'0' + 52,
			b'+' | b'-' => 62,
			b'/' | b'_' => 63,
			_ => return None,
		} as u32)
	};
	let digits: Vec<u8> = text.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=').collect();
	if digits.len() % 4 == 1 {
		return None;
	}
	let mut out = Vec::with_capacity(digits.len() * 3 / 4);
	for chunk in digits.chunks(4) {
		let mut n = 0u32;
		for (i, &c) in chunk.iter().enumerate() {
			n |= value(c)? << (18 - 6 * i);
		}
		out.push((n >> 16) as u8);
		if chunk.len() > 2 {
			out.push((n >> 8) as u8);
		}
		if chunk.len() > 3 {
			out.push(n as u8);
		}
	}
	Some(out)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn base64_round_trips() {
		for input in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar", &[0, 255, 128, 7]] {
			let encoded = encode_base64(input);
			assert_eq!(decode_base64(&encoded).as_deref(), Some(input), "{encoded}");
		}
		assert_eq!(encode_base64(b"foobar"), "Zm9vYmFy");
		assert_eq!(encode_base64(b"fo"), "Zm8=");
		assert_eq!(decode_base64("Zm9v\nYmFy").as_deref(), Some(&b"foobar"[..]));
		assert_eq!(decode_base64("Z"), None);
		assert_eq!(decode_base64("Zm9*"), None);
	}

	#[test]
	fn clipboard_images_and_files_are_collected() {
		let dir = std::env::temp_dir().join(format!("omp-desktop-attach-{}", std::process::id()));
		std::fs::create_dir_all(&dir).unwrap();
		let png = dir.join("shot.PNG");
		std::fs::write(&png, b"\x89PNG fake").unwrap();
		let txt = dir.join("notes.txt");
		std::fs::write(&txt, b"hi").unwrap();

		let bitmap = Image::from_bytes(ImageFormat::Jpeg, vec![1, 2, 3]);
		let item = ClipboardItem {
			entries: vec![
				ClipboardEntry::Image(bitmap),
				ClipboardEntry::Image(Image::from_bytes(ImageFormat::Svg, b"<svg/>".to_vec())),
				ClipboardEntry::ExternalPaths(gpui_kit::ExternalPaths(vec![png.clone(), txt].into())),
			],
		};
		let pasted = from_clipboard(&item);
		let labels: Vec<_> = pasted.images.iter().map(Attachment::label).collect();
		assert_eq!(labels, ["图片.jpg", "shot.PNG"]);
		assert_eq!(pasted.images[1].mime_type(), "image/png");
		assert_eq!(pasted.rejected, ["SVG 图片（不支持）", "notes.txt（不是图片）"]);

		let wire = serde_json::to_value(pasted.images[0].to_wire()).unwrap();
		assert_eq!(wire, serde_json::json!({"type":"image","data":"AQID","mimeType":"image/jpeg"}));
		_ = std::fs::remove_dir_all(&dir);
	}

	#[test]
	fn history_images_decode() {
		let image = decode("image/webp", "AQID").unwrap();
		assert_eq!((image.format, image.bytes.as_slice()), (ImageFormat::Webp, &[1u8, 2, 3][..]));
		assert!(decode("image/x-unknown", "AQID").is_none());
	}
}
