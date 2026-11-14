//! Build script that generates high-resolution icons for:
//! - Linux (`assets/icon.png` 512x512)
//! - macOS (`assets/icon.icns` multi-res Apple Icon Image: 128x128, 256x256, 512x512)
//! - Windows (`assets/icon.ico` multi-res Windows Icon: 16, 32, 48, 64, 256)
//! And embeds the icon resource into the Windows executable when targeting Windows.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon.svg");
    println!("cargo:rerun-if-changed=assets/icon.ico");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let assets_dir = manifest_dir.join("assets");
    let _ = fs::create_dir_all(&assets_dir);

    let png_path = assets_dir.join("icon.png");
    let ico_path = assets_dir.join("icon.ico");
    let icns_path = assets_dir.join("icon.icns");

    if !png_path.exists() || !ico_path.exists() || !icns_path.exists() {
        generate_all_icon_files(&png_path, &ico_path, &icns_path);
    }

    // If compiling for Windows, embed `assets/icon.ico` into the PE executable resources
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" && ico_path.exists() {
        embed_windows_icon(&ico_path);
    }
}

fn generate_all_icon_files(png_path: &Path, ico_path: &Path, icns_path: &Path) {
    // 1. Linux / Universal 512x512 PNG
    let rgba_512 = render_icon_rgba(512);
    let png_512 = encode_png_rgba(512, 512, &rgba_512);
    let _ = fs::write(png_path, &png_512);

    // 2. Render standard mip sizes
    let sizes = [16usize, 32, 48, 64, 128, 256];
    let mut png_frames: Vec<(usize, Vec<u8>)> = Vec::new();
    for &sz in &sizes {
        let rgba = render_icon_rgba(sz);
        let png = encode_png_rgba(sz as u32, sz as u32, &rgba);
        png_frames.push((sz, png));
    }

    // 3. Windows .ico file (16, 32, 48, 64, 256)
    let ico_sizes: Vec<&(usize, Vec<u8>)> = png_frames
        .iter()
        .filter(|(s, _)| matches!(s, 16 | 32 | 48 | 64 | 256))
        .collect();
    let ico_bytes = encode_ico_file(&ico_sizes);
    let _ = fs::write(ico_path, ico_bytes);

    // 4. macOS .icns file (ic07 = 128x128, ic08 = 256x256, ic09 = 512x512)
    let png_128 = &png_frames.iter().find(|(s, _)| *s == 128).unwrap().1;
    let png_256 = &png_frames.iter().find(|(s, _)| *s == 256).unwrap().1;
    let icns_bytes = encode_icns_file(&[
        (*b"ic07", png_128.as_slice()),
        (*b"ic08", png_256.as_slice()),
        (*b"ic09", png_512.as_slice()),
    ]);
    let _ = fs::write(icns_path, icns_bytes);
}

fn embed_windows_icon(ico_path: &Path) {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let rc_path = out_dir.join("app_icon.rc");
    // Escape backslashes for .rc syntax
    let ico_escaped = ico_path.display().to_string().replace('\\', "/");
    let rc_content = format!("1 ICON \"{}\"\n", ico_escaped);
    if fs::write(&rc_path, rc_content).is_err() {
        return;
    }

    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_env == "gnu" {
        let res_obj = out_dir.join("app_icon.o");
        let status = Command::new("windres")
            .args([
                rc_path.to_str().unwrap(),
                "-O",
                "coff",
                "-o",
                res_obj.to_str().unwrap(),
            ])
            .status();
        if let Ok(s) = status {
            if s.success() && res_obj.exists() {
                println!("cargo:rustc-link-arg-bins={}", res_obj.display());
            }
        }
    } else if target_env == "msvc" {
        let res_file = out_dir.join("app_icon.res");
        if write_windows_res_from_ico(ico_path, &res_file).is_ok() && res_file.exists() {
            println!("cargo:rustc-link-arg-bins={}", res_file.display());
        } else {
            println!("cargo:warning=Failed to generate Windows .res icon resource");
        }
    }
}

/// Write a standard 32-bit Windows `.res` resource file containing `RT_ICON` and `RT_GROUP_ICON`
/// directly from an `.ico` file, without requiring `rc.exe` in `PATH`.
fn write_windows_res_from_ico(ico_path: &Path, res_path: &Path) -> std::io::Result<()> {
    let ico = fs::read(ico_path)?;
    if ico.len() < 6 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "ICO file too short",
        ));
    }
    let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
    if ico.len() < 6 + count * 16 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "ICO directory truncated",
        ));
    }

    let mut out = Vec::new();
    // 32-byte empty .res header
    append_res_record(&mut out, 0, 0, 0, 0, 0, &[]);

    // Build RT_GROUP_ICON (type = 14, name = 1)
    let mut group_data = Vec::with_capacity(6 + count * 14);
    group_data.extend_from_slice(&ico[0..6]);

    for i in 0..count {
        let entry_off = 6 + i * 16;
        let entry = &ico[entry_off..entry_off + 16];
        let bytes_in_res =
            u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]) as usize;
        let img_offset =
            u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]) as usize;

        if img_offset + bytes_in_res > ico.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "ICO image out of bounds",
            ));
        }

        let icon_id = (i + 1) as u16;
        // RT_ICON = 3
        append_res_record(
            &mut out,
            3,
            icon_id,
            0x1010, // MOVEABLE | PURE
            0x0409, // en-US
            bytes_in_res as u32,
            &ico[img_offset..img_offset + bytes_in_res],
        );

        group_data.extend_from_slice(&entry[0..12]);
        group_data.extend_from_slice(&icon_id.to_le_bytes());
    }

    // RT_GROUP_ICON = 14, name = 1
    append_res_record(
        &mut out,
        14,
        1,
        0x1030, // MOVEABLE | PURE | DISCARDABLE
        0x0409,
        group_data.len() as u32,
        &group_data,
    );

    fs::write(res_path, out)
}

fn append_res_record(
    out: &mut Vec<u8>,
    res_type: u16,
    res_name: u16,
    mem_flags: u16,
    lang_id: u16,
    data_size: u32,
    data: &[u8],
) {
    out.extend_from_slice(&data_size.to_le_bytes());
    out.extend_from_slice(&32u32.to_le_bytes()); // HeaderSize = 32
    out.extend_from_slice(&0xFFFFu16.to_le_bytes());
    out.extend_from_slice(&res_type.to_le_bytes());
    out.extend_from_slice(&0xFFFFu16.to_le_bytes());
    out.extend_from_slice(&res_name.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // DataVersion
    out.extend_from_slice(&mem_flags.to_le_bytes());
    out.extend_from_slice(&lang_id.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // Version
    out.extend_from_slice(&0u32.to_le_bytes()); // Characteristics
    out.extend_from_slice(data);
    while out.len() % 4 != 0 {
        out.push(0);
    }
}

fn encode_ico_file(frames: &[&(usize, Vec<u8>)]) -> Vec<u8> {
    let count = frames.len() as u16;
    let mut out = Vec::new();
    // ICONDIR header
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type = 1 (ICON)
    out.extend_from_slice(&count.to_le_bytes());

    let mut data_offset = 6 + (frames.len() * 16) as u32;
    for &(sz, ref png) in frames {
        let dim_byte = if *sz >= 256 { 0u8 } else { *sz as u8 };
        out.push(dim_byte); // width
        out.push(dim_byte); // height
        out.push(0); // color count
        out.push(0); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // color planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        let len = png.len() as u32;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&data_offset.to_le_bytes());
        data_offset += len;
    }

    for &(_, ref png) in frames {
        out.extend_from_slice(png);
    }
    out
}

fn encode_icns_file(chunks: &[([u8; 4], &[u8])]) -> Vec<u8> {
    let mut total_len: u32 = 8;
    for &(_, data) in chunks {
        total_len += 8 + data.len() as u32;
    }
    let mut out = Vec::with_capacity(total_len as usize);
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&total_len.to_be_bytes());
    for &(ostype, data) in chunks {
        let chunk_len = 8 + data.len() as u32;
        out.extend_from_slice(&ostype);
        out.extend_from_slice(&chunk_len.to_be_bytes());
        out.extend_from_slice(data);
    }
    out
}

fn encode_png_rgba(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");

    // IHDR
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(6); // color type RGBA
    ihdr.push(0); // compression
    ihdr.push(0); // filter
    ihdr.push(0); // interlace
    write_png_chunk(&mut out, b"IHDR", &ihdr);

    // Raw filtered image rows (filter = 0 per scanline)
    let row_bytes = (width as usize) * 4;
    let mut raw_scanlines = Vec::with_capacity((row_bytes + 1) * (height as usize));
    for y in 0..(height as usize) {
        raw_scanlines.push(0u8);
        let start = y * row_bytes;
        raw_scanlines.extend_from_slice(&rgba[start..start + row_bytes]);
    }

    // Wrap in zlib stream with uncompressed DEFLATE blocks
    let mut zlib = Vec::with_capacity(raw_scanlines.len() + 64);
    zlib.push(0x78); // CMF
    zlib.push(0x01); // FLG

    let chunks: Vec<&[u8]> = raw_scanlines.chunks(65535).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        let is_last = i + 1 == chunks.len();
        zlib.push(if is_last { 0x01 } else { 0x00 });
        let len = chunk.len() as u16;
        let nlen = !len;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&nlen.to_le_bytes());
        zlib.extend_from_slice(chunk);
    }
    let adler = adler32(&raw_scanlines);
    zlib.extend_from_slice(&adler.to_be_bytes());
    write_png_chunk(&mut out, b"IDAT", &zlib);

    // IEND
    write_png_chunk(&mut out, b"IEND", &[]);
    out
}

fn write_png_chunk(out: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
    let len = data.len() as u32;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(chunk_type);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(chunk_type);
    crc_input.extend_from_slice(data);
    let crc = crc32(&crc_input);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            if (crc & 1) != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in bytes {
        a = (a + (byte as u32)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

// Same 2x2 supersampled SDF icon renderer as `src/icon.rs`
fn render_icon_rgba(size: usize) -> Vec<u8> {
    let mut buf = vec![0u8; size * size * 4];
    let inv_size = 1.0 / (size as f32);
    let offsets = [(-0.25_f32, -0.25_f32), (0.25, -0.25), (-0.25, 0.25), (0.25, 0.25)];

    for py in 0..size {
        for px in 0..size {
            let mut r_acc = 0.0_f32;
            let mut g_acc = 0.0_f32;
            let mut b_acc = 0.0_f32;
            let mut a_acc = 0.0_f32;

            for &(ox, oy) in &offsets {
                let u = ((px as f32 + 0.5 + ox) * inv_size) * 2.0 - 1.0;
                let v = ((py as f32 + 0.5 + oy) * inv_size) * 2.0 - 1.0;
                let (r, g, b, a) = sample_icon_sdf(u, v, size);
                r_acc += r * a;
                g_acc += g * a;
                b_acc += b * a;
                a_acc += a;
            }

            let a_final = (a_acc * 0.25).clamp(0.0, 1.0);
            let (r_final, g_final, b_final) = if a_acc > 1e-5 {
                (
                    (r_acc / a_acc).clamp(0.0, 1.0),
                    (g_acc / a_acc).clamp(0.0, 1.0),
                    (b_acc / a_acc).clamp(0.0, 1.0),
                )
            } else {
                (0.0, 0.0, 0.0)
            };

            let idx = (py * size + px) * 4;
            buf[idx] = (r_final * 255.0).round() as u8;
            buf[idx + 1] = (g_final * 255.0).round() as u8;
            buf[idx + 2] = (b_final * 255.0).round() as u8;
            buf[idx + 3] = (a_final * 255.0).round() as u8;
        }
    }
    buf
}

fn sample_icon_sdf(u: f32, v: f32, size: usize) -> (f32, f32, f32, f32) {
    let half_extent = 0.88_f32;
    let corner_radius = 0.36_f32;
    let d_badge = sd_rounded_box(u, v, half_extent, half_extent, corner_radius);

    let aa = (1.5 / size as f32).max(0.0055);
    let badge_alpha = smoothstep(aa, -aa, d_badge);
    if badge_alpha <= 0.0 {
        return (0.0, 0.0, 0.0, 0.0);
    }

    let t_diag = ((u + v) * 0.5 + 1.0) * 0.5;
    let mut bg_r = lerp(0.048, 0.082, t_diag);
    let mut bg_g = lerp(0.078, 0.132, t_diag);
    let mut bg_b = lerp(0.155, 0.245, t_diag);

    let hub_x = -0.40_f32;
    let hub_y = 0.40_f32;
    let dist_center = (u * u + v * v).sqrt();
    let center_glow = (1.0 - (dist_center / 1.15).clamp(0.0, 1.0)).powi(2);
    bg_r += center_glow * 0.035;
    bg_g += center_glow * 0.15;
    bg_b += center_glow * 0.22;

    let dist_hub = ((u - hub_x).powi(2) + (v - hub_y).powi(2)).sqrt();
    let hub_glow = (1.0 - (dist_hub / 0.85).clamp(0.0, 1.0)).powi(2);
    bg_r += hub_glow * 0.025;
    bg_g += hub_glow * 0.12;
    bg_b += hub_glow * 0.14;

    let border_dist = (d_badge + 0.016).abs() - 0.007;
    let border_mask = smoothstep(aa, -aa, border_dist) * smoothstep(0.0, -aa * 2.0, d_badge);
    let rim_t = (1.0 - (v + 1.0) * 0.5).clamp(0.0, 1.0);
    bg_r = lerp(bg_r, lerp(0.18, 0.30, rim_t), border_mask * 0.55);
    bg_g = lerp(bg_g, lerp(0.58, 0.84, rim_t), border_mask * 0.55);
    bg_b = lerp(bg_b, lerp(0.76, 0.96, rim_t), border_mask * 0.55);

    let dx = u - hub_x;
    let dy = v - hub_y;
    let r = (dx * dx + dy * dy).sqrt();

    let mut wave_sdf = 10.0_f32;
    let radii = [0.27_f32, 0.46_f32, 0.65_f32];
    let arc_thickness = 0.036_f32;
    let a_min = 0.16_f32;
    let a_max = std::f32::consts::FRAC_PI_2 - 0.16_f32;

    for &rad in &radii {
        let d_arc = sd_arc_segment(dx, dy, rad, a_min, a_max, arc_thickness);
        wave_sdf = wave_sdf.min(d_arc);
    }

    let node_hub = (hub_x, hub_y);
    let node_top = (0.24_f32, -0.24_f32);
    let node_mid = (0.44_f32, 0.08_f32);
    let node_top_left = (-0.08_f32, -0.44_f32);

    let d_line1 = sd_segment(u, v, node_hub.0, node_hub.1, node_top.0, node_top.1) - 0.020;
    let d_line2 = sd_segment(u, v, node_top.0, node_top.1, node_mid.0, node_mid.1) - 0.017;
    let d_line3 = sd_segment(u, v, node_top.0, node_top.1, node_top_left.0, node_top_left.1) - 0.017;
    let tree_lines_sdf = d_line1.min(d_line2).min(d_line3);

    let d_hub_circle = ((u - node_hub.0).powi(2) + (v - node_hub.1).powi(2)).sqrt() - 0.115;
    let d_top_circle = ((u - node_top.0).powi(2) + (v - node_top.1).powi(2)).sqrt() - 0.095;
    let d_mid_circle = ((u - node_mid.0).powi(2) + (v - node_mid.1).powi(2)).sqrt() - 0.070;
    let d_tl_circle = ((u - node_top_left.0).powi(2) + (v - node_top_left.1).powi(2)).sqrt() - 0.070;

    let nodes_sdf = d_hub_circle
        .min(d_top_circle)
        .min(d_mid_circle)
        .min(d_tl_circle);

    let line_mask = smoothstep(aa, -aa, tree_lines_sdf);
    bg_r = lerp(bg_r, 0.28, line_mask * 0.92);
    bg_g = lerp(bg_g, 0.86, line_mask * 0.92);
    bg_b = lerp(bg_b, 0.96, line_mask * 0.92);

    let wave_mask = smoothstep(aa, -aa, wave_sdf);
    let wave_grad = ((r - 0.24) / 0.45).clamp(0.0, 1.0);
    let wave_r = lerp(0.13, 0.20, wave_grad);
    let wave_g = lerp(0.83, 0.92, wave_grad);
    let wave_b = lerp(0.96, 0.65, wave_grad);
    bg_r = lerp(bg_r, wave_r, wave_mask);
    bg_g = lerp(bg_g, wave_g, wave_mask);
    bg_b = lerp(bg_b, wave_b, wave_mask);

    let node_mask = smoothstep(aa, -aa, nodes_sdf);
    let inner_cores = (d_hub_circle + 0.053)
        .min(d_top_circle + 0.045)
        .min(d_mid_circle + 0.034)
        .min(d_tl_circle + 0.034);
    let core_mask = smoothstep(aa, -aa, inner_cores);

    bg_r = lerp(bg_r, 0.18, node_mask);
    bg_g = lerp(bg_g, 0.90, node_mask);
    bg_b = lerp(bg_b, 0.78, node_mask);

    bg_r = lerp(bg_r, 0.97, core_mask);
    bg_g = lerp(bg_g, 0.99, core_mask);
    bg_b = lerp(bg_b, 1.00, core_mask);

    (bg_r, bg_g, bg_b, badge_alpha)
}

fn sd_rounded_box(px: f32, py: f32, bx: f32, by: f32, r: f32) -> f32 {
    let qx = px.abs() - bx + r;
    let qy = py.abs() - by + r;
    let out_dist = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    let in_dist = qx.max(qy).min(0.0);
    out_dist + in_dist - r
}

fn sd_segment(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let pax = px - ax;
    let pay = py - ay;
    let bax = bx - ax;
    let bay = by - ay;
    let h = ((pax * bax + pay * bay) / (bax * bax + bay * bay)).clamp(0.0, 1.0);
    let dx = pax - bax * h;
    let dy = pay - bay * h;
    (dx * dx + dy * dy).sqrt()
}

fn sd_arc_segment(
    dx: f32,
    dy: f32,
    radius: f32,
    angle_min: f32,
    angle_max: f32,
    half_thickness: f32,
) -> f32 {
    let ang = (-dy).atan2(dx);
    if ang >= angle_min && ang <= angle_max {
        let r = (dx * dx + dy * dy).sqrt();
        (r - radius).abs() - half_thickness
    } else {
        let p1x = radius * angle_min.cos();
        let p1y = -radius * angle_min.sin();
        let p2x = radius * angle_max.cos();
        let p2y = -radius * angle_max.sin();
        let d1 = ((dx - p1x).powi(2) + (dy - p1y).powi(2)).sqrt();
        let d2 = ((dx - p2x).powi(2) + (dy - p2y).powi(2)).sqrt();
        d1.min(d2) - half_thickness
    }
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
