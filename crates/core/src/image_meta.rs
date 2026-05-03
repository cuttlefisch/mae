//! Image metadata extraction without fully decoding the image.
//!
//! Uses `imagesize` for fast dimension reading (~10μs per image) and
//! `kamadak-exif` for EXIF extraction (JPEG/TIFF only).

use std::path::Path;

/// Image metadata extracted without fully decoding.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ImageMeta {
    pub path: String,
    pub format: String,
    pub width: u32,
    pub height: u32,
    pub file_size: u64,
    pub color_type: Option<String>,
    pub bit_depth: Option<u8>,
    pub exif: Option<ExifData>,
}

/// EXIF metadata (JPEG/TIFF only).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExifData {
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub date_taken: Option<String>,
    pub exposure_time: Option<String>,
    pub f_number: Option<String>,
    pub iso: Option<u32>,
    pub focal_length: Option<String>,
    pub gps_latitude: Option<f64>,
    pub gps_longitude: Option<f64>,
    pub orientation: Option<u16>,
    pub software: Option<String>,
    pub image_description: Option<String>,
}

/// Read image metadata from a file path.
pub fn read_image_meta(path: &Path) -> Result<ImageMeta, String> {
    if !path.exists() {
        return Err(format!("Image not found: {}", path.display()));
    }

    let file_size = std::fs::metadata(path)
        .map_err(|e| format!("Failed to read file metadata: {}", e))?
        .len();

    // Fast dimension + format detection via imagesize.
    let (width, height, format) = read_dimensions(path)?;

    // EXIF extraction (JPEG/TIFF only).
    let exif = read_exif(path);

    Ok(ImageMeta {
        path: path.display().to_string(),
        format,
        width,
        height,
        file_size,
        color_type: None,
        bit_depth: None,
        exif,
    })
}

fn read_dimensions(path: &Path) -> Result<(u32, u32, String), String> {
    // Try imagesize first (handles PNG, JPEG, WebP, GIF, BMP).
    match imagesize::size(path) {
        Ok(size) => {
            let format = format_from_extension(path);
            Ok((size.width as u32, size.height as u32, format))
        }
        Err(_) => {
            // SVG fallback: read file and look for width/height/viewBox.
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if ext == "svg" {
                return read_svg_dimensions(path);
            }
            Err(format!(
                "Failed to read image dimensions: {}",
                path.display()
            ))
        }
    }
}

fn format_from_extension(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_uppercase())
        .unwrap_or_else(|| "Unknown".to_string())
}

fn read_svg_dimensions(path: &Path) -> Result<(u32, u32, String), String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read SVG: {}", e))?;

    // Look for width="N" height="N" in the <svg> tag.
    let width = extract_svg_attr(&content, "width");
    let height = extract_svg_attr(&content, "height");

    if let (Some(w), Some(h)) = (width, height) {
        return Ok((w, h, "SVG".to_string()));
    }

    // Fallback: parse viewBox="minX minY width height".
    if let Some(vb) = extract_svg_viewbox(&content) {
        return Ok((vb.0, vb.1, "SVG".to_string()));
    }

    Ok((0, 0, "SVG".to_string()))
}

fn extract_svg_attr(content: &str, attr: &str) -> Option<u32> {
    let pattern = format!("{}=\"", attr);
    let idx = content.find(&pattern)?;
    let rest = &content[idx + pattern.len()..];
    let num_str: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    num_str.parse::<f64>().ok().map(|f| f as u32)
}

fn extract_svg_viewbox(content: &str) -> Option<(u32, u32)> {
    let idx = content.find("viewBox=\"")?;
    let rest = &content[idx + 9..];
    let end = rest.find('"')?;
    let parts: Vec<&str> = rest[..end].split_whitespace().collect();
    if parts.len() >= 4 {
        let w: f64 = parts[2].parse().ok()?;
        let h: f64 = parts[3].parse().ok()?;
        Some((w as u32, h as u32))
    } else {
        None
    }
}

fn read_exif(path: &Path) -> Option<ExifData> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    // EXIF only makes sense for JPEG and TIFF.
    if ext != "jpg" && ext != "jpeg" && ext != "tiff" && ext != "tif" {
        return None;
    }

    let file = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(file);
    let exif_reader = exif::Reader::new();
    let exif_data = exif_reader.read_from_container(&mut reader).ok()?;

    let get_string = |tag: exif::Tag| -> Option<String> {
        exif_data
            .get_field(tag, exif::In::PRIMARY)
            .map(|f| f.display_value().to_string())
    };

    let get_u32 = |tag: exif::Tag| -> Option<u32> {
        exif_data
            .get_field(tag, exif::In::PRIMARY)
            .and_then(|f| match f.value {
                exif::Value::Short(ref v) => v.first().map(|&x| x as u32),
                exif::Value::Long(ref v) => v.first().copied(),
                _ => f.display_value().to_string().parse().ok(),
            })
    };

    let get_u16 = |tag: exif::Tag| -> Option<u16> {
        exif_data
            .get_field(tag, exif::In::PRIMARY)
            .and_then(|f| match f.value {
                exif::Value::Short(ref v) => v.first().copied(),
                _ => f.display_value().to_string().parse().ok(),
            })
    };

    let gps_latitude = parse_gps_coord(
        &exif_data,
        exif::Tag::GPSLatitude,
        exif::Tag::GPSLatitudeRef,
    );
    let gps_longitude = parse_gps_coord(
        &exif_data,
        exif::Tag::GPSLongitude,
        exif::Tag::GPSLongitudeRef,
    );

    Some(ExifData {
        camera_make: get_string(exif::Tag::Make),
        camera_model: get_string(exif::Tag::Model),
        date_taken: get_string(exif::Tag::DateTimeOriginal),
        exposure_time: get_string(exif::Tag::ExposureTime),
        f_number: get_string(exif::Tag::FNumber),
        iso: get_u32(exif::Tag::PhotographicSensitivity),
        focal_length: get_string(exif::Tag::FocalLength),
        gps_latitude,
        gps_longitude,
        orientation: get_u16(exif::Tag::Orientation),
        software: get_string(exif::Tag::Software),
        image_description: get_string(exif::Tag::ImageDescription),
    })
}

/// Convert EXIF GPS rational degrees/minutes/seconds to decimal degrees.
fn parse_gps_coord(
    exif_data: &exif::Exif,
    coord_tag: exif::Tag,
    ref_tag: exif::Tag,
) -> Option<f64> {
    let field = exif_data.get_field(coord_tag, exif::In::PRIMARY)?;
    let rationals = match &field.value {
        exif::Value::Rational(v) => v,
        _ => return None,
    };
    if rationals.len() < 3 {
        return None;
    }
    let deg = rationals[0].to_f64();
    let min = rationals[1].to_f64();
    let sec = rationals[2].to_f64();
    let mut decimal = deg + min / 60.0 + sec / 3600.0;

    // Apply N/S or E/W reference.
    if let Some(ref_field) = exif_data.get_field(ref_tag, exif::In::PRIMARY) {
        let ref_str = ref_field.display_value().to_string();
        if ref_str == "S" || ref_str == "W" {
            decimal = -decimal;
        }
    }

    Some(decimal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn read_image_meta_missing_file() {
        let result = read_image_meta(Path::new("/nonexistent/image.png"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn read_image_meta_non_image_file() {
        // Use Cargo.toml as a non-image file.
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let result = read_image_meta(&path);
        assert!(result.is_err());
    }

    #[test]
    fn format_from_extension_basic() {
        assert_eq!(format_from_extension(Path::new("photo.jpg")), "JPG");
        assert_eq!(format_from_extension(Path::new("img.PNG")), "PNG");
        assert_eq!(format_from_extension(Path::new("no_ext")), "Unknown");
    }

    #[test]
    fn svg_viewbox_parsing() {
        let svg = r#"<svg viewBox="0 0 100 200"></svg>"#;
        let result = extract_svg_viewbox(svg);
        assert_eq!(result, Some((100, 200)));
    }

    #[test]
    fn svg_attr_parsing() {
        let svg = r#"<svg width="300" height="150"></svg>"#;
        assert_eq!(extract_svg_attr(svg, "width"), Some(300));
        assert_eq!(extract_svg_attr(svg, "height"), Some(150));
    }

    #[test]
    fn read_test_fixture_png() {
        // Validate the test-image.png fixture in assets/.
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("assets/test-image.png");
        if !path.exists() {
            // Skip if running outside workspace root.
            return;
        }
        let meta = read_image_meta(&path).expect("should read test PNG");
        assert_eq!(meta.width, 16);
        assert_eq!(meta.height, 16);
        assert!(meta.format.contains("PNG") || meta.format.contains("Png"));
        assert!(meta.file_size > 0);
    }
}
