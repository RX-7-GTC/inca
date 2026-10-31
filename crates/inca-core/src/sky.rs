//! Sky-dome environment textures.
//!
//! The JS app loads a sky dome light's `textureFile` as an equirectangular image (HDR/EXR are
//! linear, PNG/JPG are sRGB), uploads it as a linear texture and lets three.js PMREM-filter it
//! for `scene.environment`. The Rust viewport does the equivalent on the CPU: decode to linear
//! RGBA with the first row at the nadir (equirect `v = 0`), build a mip chain for the background
//! and prefilter a small irradiance map for the diffuse environment.

use std::path::{Path, PathBuf};

/// Longest side used for the viewport sky texture. The JS downsamples PNG/JPG to 2048 wide and
/// leaves HDR/EXR alone; capping every format keeps uploads and mip generation bounded.
pub const MAX_WIDTH: usize = 2048;

/// Linear RGBA equirect image, row 0 = bottom (equirect `v = 0`, nadir).
pub struct SkyImage {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<f32>,
}

fn srgb_to_linear(x: f32) -> f32 {
    if x <= 0.04045 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| -> Option<u8> {
                match c {
                    b'0'..=b'9' => Some(c - b'0'),
                    b'a'..=b'f' => Some(c - b'a' + 10),
                    b'A'..=b'F' => Some(c - b'A' + 10),
                    _ => None,
                }
            };
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Resolve a `textureFile` like the JS `absPathOf`: `file://` URLs, absolute paths, or a path
/// relative to the project directory.
pub fn resolve_path(project: Option<&str>, file: &str) -> PathBuf {
    let mut f = file.trim().to_string();
    if let Some(rest) = f.strip_prefix("file://") {
        f = percent_decode(rest);
        // `file:///C:/x` decodes to `/C:/x`; drop the leading slash before a drive letter.
        let b = f.as_bytes();
        if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
            f.remove(0);
        }
    }
    let p = Path::new(&f);
    let rooted =
        f.starts_with('/') || f.starts_with('\\') || (f.len() >= 2 && f.as_bytes()[1] == b':');
    if p.is_absolute() || rooted {
        p.to_path_buf()
    } else if let Some(pr) = project {
        Path::new(pr).join(p)
    } else {
        p.to_path_buf()
    }
}

/// Decode an equirectangular image to linear RGBA. `.hdr`/`.exr` are linear; everything else is
/// treated as sRGB like the JS `loadSkyTexture`.
pub fn load(path: &Path) -> Result<SkyImage, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let linear = ext == "hdr" || ext == "exr";
    let img = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w == 0 || h == 0 {
        return Err("empty image".into());
    }
    let (tw, th) = if w > MAX_WIDTH {
        (
            MAX_WIDTH,
            ((h as f64 * MAX_WIDTH as f64 / w as f64).round() as usize).max(1),
        )
    } else {
        (w, h)
    };
    let mut rgba = vec![0.0f32; tw * th * 4];
    if linear {
        let f = img.to_rgba32f();
        let f = if (tw, th) != (w, h) {
            image::imageops::resize(
                &f,
                tw as u32,
                th as u32,
                image::imageops::FilterType::Triangle,
            )
        } else {
            f
        };
        for (i, p) in f.pixels().enumerate() {
            let o = i * 4;
            rgba[o] = p[0];
            rgba[o + 1] = p[1];
            rgba[o + 2] = p[2];
            rgba[o + 3] = p[3];
        }
    } else {
        let b = img.to_rgba8();
        let b = if (tw, th) != (w, h) {
            image::imageops::resize(
                &b,
                tw as u32,
                th as u32,
                image::imageops::FilterType::Triangle,
            )
        } else {
            b
        };
        for (i, p) in b.pixels().enumerate() {
            let o = i * 4;
            rgba[o] = srgb_to_linear(p[0] as f32 / 255.0);
            rgba[o + 1] = srgb_to_linear(p[1] as f32 / 255.0);
            rgba[o + 2] = srgb_to_linear(p[2] as f32 / 255.0);
            rgba[o + 3] = p[3] as f32 / 255.0;
        }
    }
    // image rows run top (zenith) to bottom; the texture stores row 0 = nadir.
    let mut flipped = vec![0.0f32; rgba.len()];
    let row = tw * 4;
    for y in 0..th {
        let src = (th - 1 - y) * row;
        flipped[y * row..y * row + row].copy_from_slice(&rgba[src..src + row]);
    }
    Ok(SkyImage {
        width: tw,
        height: th,
        rgba: flipped,
    })
}

/// Box-filtered mip chain, level 0 first. Horizontal taps wrap (the equirect seam); vertical
/// taps clamp at the poles.
pub fn mip_chain(img: &SkyImage) -> Vec<(u32, u32, Vec<f32>)> {
    let mut levels: Vec<(u32, u32, Vec<f32>)> =
        vec![(img.width as u32, img.height as u32, img.rgba.clone())];
    loop {
        let (w, h) = {
            let (w, h, _) = levels.last().unwrap();
            (*w, *h)
        };
        if w <= 1 && h <= 1 {
            break;
        }
        let nw = (w / 2).max(1);
        let nh = (h / 2).max(1);
        let mut out = vec![0.0f32; (nw * nh * 4) as usize];
        {
            let src = &levels.last().unwrap().2;
            for y in 0..nh as usize {
                for x in 0..nw as usize {
                    let mut acc = [0.0f32; 4];
                    for dy in 0..2 {
                        let sy = ((y * 2 + dy) as u32).min(h - 1) as usize;
                        for dx in 0..2 {
                            let sx = ((x * 2 + dx) as u32 % w) as usize;
                            let o = (sy * w as usize + sx) * 4;
                            for c in 0..4 {
                                acc[c] += src[o + c];
                            }
                        }
                    }
                    let o = (y * nw as usize + x) * 4;
                    for c in 0..4 {
                        out[o + c] = acc[c] * 0.25;
                    }
                }
            }
        }
        levels.push((nw, nh, out));
    }
    levels
}

/// Normalised cosine-weighted average of the environment at each output direction (the PMREM
/// roughness-1 irradiance the JS `scene.environment` uses for diffuse light). Linear RGBA.
pub fn irradiance(img: &SkyImage, out_w: usize, out_h: usize) -> Vec<f32> {
    use std::f32::consts::{PI, TAU};
    // Downsample first: the convolution is smooth, so 128x64 source samples are plenty.
    let sw = img.width.min(128).max(2);
    let sh = img.height.min(64).max(2);
    let mut src = vec![0.0f32; sw * sh * 4];
    for y in 0..sh {
        let y0 = y * img.height / sh;
        let y1 = (((y + 1) * img.height / sh).max(y0 + 1)).min(img.height);
        for x in 0..sw {
            let x0 = x * img.width / sw;
            let x1 = (((x + 1) * img.width / sw).max(x0 + 1)).min(img.width);
            let mut acc = [0.0f32; 4];
            let mut n = 0.0f32;
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let o = (sy * img.width + sx) * 4;
                    for c in 0..4 {
                        acc[c] += img.rgba[o + c];
                    }
                    n += 1.0;
                }
            }
            let o = (y * sw + x) * 4;
            for c in 0..4 {
                src[o + c] = acc[c] / n.max(1.0);
            }
        }
    }
    // Source directions and solid-angle weights (the equirect texel area is sin(theta)).
    let mut dirs = vec![[0.0f32; 3]; sw * sh];
    let mut wts = vec![0.0f32; sw * sh];
    let dw = TAU / sw as f32;
    let dh = PI / sh as f32;
    for y in 0..sh {
        let v = (y as f32 + 0.5) / sh as f32;
        let theta = (v - 0.5) * PI;
        let (st, ct) = theta.sin_cos();
        for x in 0..sw {
            let u = (x as f32 + 0.5) / sw as f32;
            let phi = (u - 0.5) * TAU;
            let (sp, cp) = phi.sin_cos();
            let k = y * sw + x;
            dirs[k] = [ct * cp, st, ct * sp];
            // `theta` is latitude, so the equirect texel area is cos(latitude), not sin.
            wts[k] = ct.max(0.0) * dw * dh;
        }
    }
    let mut out = vec![0.0f32; out_w * out_h * 4];
    for y in 0..out_h {
        let v = (y as f32 + 0.5) / out_h as f32;
        let theta = (v - 0.5) * PI;
        let (st, ct) = theta.sin_cos();
        for x in 0..out_w {
            let u = (x as f32 + 0.5) / out_w as f32;
            let phi = (u - 0.5) * TAU;
            let (sp, cp) = phi.sin_cos();
            let d = [ct * cp, st, ct * sp];
            let mut acc = [0.0f32; 3];
            let mut total = 0.0f32;
            for k in 0..sw * sh {
                let s = dirs[k];
                let dot = (d[0] * s[0] + d[1] * s[1] + d[2] * s[2]).max(0.0);
                if dot <= 0.0 {
                    continue;
                }
                let w = wts[k] * dot;
                let o = k * 4;
                acc[0] += src[o] * w;
                acc[1] += src[o + 1] * w;
                acc[2] += src[o + 2] * w;
                total += w;
            }
            let o = (y * out_w + x) * 4;
            let inv = if total > 1e-12 { 1.0 / total } else { 0.0 };
            out[o] = acc[0] * inv;
            out[o + 1] = acc[1] * inv;
            out[o + 2] = acc[2] * inv;
            out[o + 3] = 1.0;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uniform(w: usize, h: usize, c: [f32; 3]) -> SkyImage {
        let mut rgba = vec![0.0; w * h * 4];
        for p in rgba.chunks_mut(4) {
            p[..3].copy_from_slice(&c);
            p[3] = 1.0;
        }
        SkyImage {
            width: w,
            height: h,
            rgba,
        }
    }

    #[test]
    fn resolve_paths_like_the_js() {
        assert_eq!(
            resolve_path(Some("C:/proj"), "tex/sky.hdr"),
            PathBuf::from("C:/proj/tex/sky.hdr")
        );
        assert_eq!(
            resolve_path(Some("C:/proj"), "C:/tex/sky.hdr"),
            PathBuf::from("C:/tex/sky.hdr")
        );
        assert_eq!(
            resolve_path(Some("C:/proj"), "file:///C:/tex/sky%20dome.hdr"),
            PathBuf::from("C:/tex/sky dome.hdr")
        );
        assert_eq!(resolve_path(None, "sky.png"), PathBuf::from("sky.png"));
    }

    #[test]
    fn uniform_irradiance_keeps_the_colour() {
        let img = uniform(16, 8, [0.2, 0.4, 0.8]);
        let irr = irradiance(&img, 8, 4);
        for p in irr.chunks(4) {
            assert!((p[0] - 0.2).abs() < 1e-5);
            assert!((p[1] - 0.4).abs() < 1e-5);
            assert!((p[2] - 0.8).abs() < 1e-5);
            assert!((p[3] - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn irradiance_follows_a_bright_upper_hemisphere() {
        // top half bright, bottom half black (row 0 is the bottom)
        let mut img = uniform(16, 8, [0.0, 0.0, 0.0]);
        for y in 4..8 {
            for x in 0..16 {
                let o = (y * 16 + x) * 4;
                img.rgba[o] = 1.0;
                img.rgba[o + 1] = 1.0;
                img.rgba[o + 2] = 1.0;
            }
        }
        let irr = irradiance(&img, 8, 4);
        let up = irr[(3 * 8 + 4) * 4];
        let down = irr[(0 * 8 + 4) * 4];
        assert!(up > down + 0.2, "up {up} down {down}");
    }

    #[test]
    fn mip_chain_halves_and_preserves_uniform_colour() {
        let img = uniform(8, 4, [0.25, 0.5, 0.75]);
        let levels = mip_chain(&img);
        assert_eq!(levels.first().map(|l| (l.0, l.1)), Some((8, 4)));
        assert_eq!(levels.last().map(|l| (l.0, l.1)), Some((1, 1)));
        for (_, _, px) in &levels {
            for p in px.chunks(4) {
                assert!((p[0] - 0.25).abs() < 1e-6);
                assert!((p[1] - 0.5).abs() < 1e-6);
                assert!((p[2] - 0.75).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn png_load_flips_rows_and_converts_srgb() {
        let dir = std::env::temp_dir().join("inca-sky-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sky.png");
        // top row white, bottom row black
        let mut px = vec![0u8; 4 * 2 * 4];
        for x in 0..4 {
            for c in 0..3 {
                px[x * 4 + c] = 255;
            }
            px[x * 4 + 3] = 255;
            px[(4 + x) * 4 + 3] = 255;
        }
        image::save_buffer(&path, &px, 4, 2, image::ColorType::Rgba8).unwrap();
        let img = load(&path).unwrap();
        assert_eq!((img.width, img.height), (4, 2));
        // row 0 is the nadir: black; row 1 is the zenith: white (linear 1.0)
        assert!(img.rgba[0] < 1e-6);
        assert!((img.rgba[4 * 4] - 1.0).abs() < 1e-6);
        let _ = std::fs::remove_file(&path);
    }
}
