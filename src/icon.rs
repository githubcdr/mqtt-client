//! High-resolution anti-aliased procedural icon renderer for Linux, macOS, and Windows.
//!
//! Renders a modern macOS/Windows/Linux squircle badge featuring:
//! - Deep navy-indigo radial/linear gradient background with a subtle cyan rim light
//! - Glowing MQTT broadcast signal arcs (cyan -> emerald gradient)
//! - Connected topic-tree nodes representing hierarchical MQTT pub/sub routing

use eframe::egui::IconData;

/// Generate `egui::IconData` (256x256 RGBA) for the native window, taskbar, and dock.
pub fn build_window_icon() -> IconData {
    let size = 256;
    let rgba = render_icon_rgba(size);
    IconData {
        rgba,
        width: size as u32,
        height: size as u32,
    }
}

/// Render the icon at arbitrary resolution `size x size` using 2x2 supersampled SDF anti-aliasing.
pub fn render_icon_rgba(size: usize) -> Vec<u8> {
    let mut buf = vec![0u8; size * size * 4];
    let inv_size = 1.0 / (size as f32);

    // 2x2 supersampling offsets
    let offsets = [(-0.25_f32, -0.25_f32), (0.25, -0.25), (-0.25, 0.25), (0.25, 0.25)];

    for py in 0..size {
        for px in 0..size {
            let mut r_acc = 0.0_f32;
            let mut g_acc = 0.0_f32;
            let mut b_acc = 0.0_f32;
            let mut a_acc = 0.0_f32;

            for &(ox, oy) in &offsets {
                // Normalized coordinates in [-1.0, 1.0]
                let u = ((px as f32 + 0.5 + ox) * inv_size) * 2.0 - 1.0;
                let v = ((py as f32 + 0.5 + oy) * inv_size) * 2.0 - 1.0;
                let (r, g, b, a) = sample_icon_sdf(u, v);
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

fn sample_icon_sdf(u: f32, v: f32) -> (f32, f32, f32, f32) {
    // 1. Rounded Squircle Badge SDF
    let half_extent = 0.88_f32;
    let corner_radius = 0.36_f32;
    let d_badge = sd_rounded_box(u, v, half_extent, half_extent, corner_radius);

    let aa = 0.012_f32;
    let badge_alpha = smoothstep(aa, -aa, d_badge);
    if badge_alpha <= 0.0 {
        return (0.0, 0.0, 0.0, 0.0);
    }

    // Background rich dark-indigo / slate-teal gradient
    let t_diag = ((u + v) * 0.5 + 1.0) * 0.5;
    let mut bg_r = lerp(0.055, 0.095, t_diag);
    let mut bg_g = lerp(0.085, 0.145, t_diag);
    let mut bg_b = lerp(0.165, 0.265, t_diag);

    // Subtle radial ambient glow around the center-left hub
    let hub_x = -0.34_f32;
    let hub_y = 0.34_f32;
    let dist_hub = ((u - hub_x).powi(2) + (v - hub_y).powi(2)).sqrt();
    let ambient = (1.0 - (dist_hub / 1.35).clamp(0.0, 1.0)).powi(2);
    bg_r += ambient * 0.04;
    bg_g += ambient * 0.16;
    bg_b += ambient * 0.24;

    // Subtle inner rim border highlight
    let border_dist = (d_badge + 0.032).abs() - 0.014;
    let border_mask = smoothstep(aa, -aa, border_dist) * smoothstep(0.0, -aa * 2.0, d_badge);
    let rim_t = (1.0 - (v + 1.0) * 0.5).clamp(0.0, 1.0);
    bg_r = lerp(bg_r, lerp(0.18, 0.35, rim_t), border_mask * 0.65);
    bg_g = lerp(bg_g, lerp(0.55, 0.82, rim_t), border_mask * 0.65);
    bg_b = lerp(bg_b, lerp(0.75, 0.98, rim_t), border_mask * 0.65);

    // 2. MQTT Broadcast Signal Waves (3 concentric arcs radiating from bottom-left hub toward top-right)
    let dx = u - hub_x;
    let dy = v - hub_y;
    let r = (dx * dx + dy * dy).sqrt();
    // Angle in radians: top-right quadrant from hub means dx > 0 and dy < 0
    let angle = (-dy).atan2(dx); // 0 is right (+x), PI/2 is up (-y)

    let mut wave_sdf = 10.0_f32;
    let radii = [0.34_f32, 0.62_f32, 0.90_f32];
    let arc_thickness = 0.062_f32;

    for &rad in &radii {
        let a_min = 0.14_f32;
        let a_max = std::f32::consts::FRAC_PI_2 - 0.14_f32;
        let d_arc = sd_arc_segment(dx, dy, rad, a_min, a_max, arc_thickness);
        wave_sdf = wave_sdf.min(d_arc);
    }

    // 3. Topic Tree Graph Overlay (Hub node + 2 branch nodes connected by lines)
    let node_hub = (hub_x, hub_y);
    let node_top = (0.36_f32, -0.36_f32);
    let node_mid = (0.44_f32, 0.12_f32);
    let node_top_left = (-0.12_f32, -0.44_f32);

    let d_line1 = sd_segment(u, v, node_hub.0, node_hub.1, node_top.0, node_top.1) - 0.028;
    let d_line2 = sd_segment(u, v, node_top.0, node_top.1, node_mid.0, node_mid.1) - 0.024;
    let d_line3 = sd_segment(u, v, node_top.0, node_top.1, node_top_left.0, node_top_left.1) - 0.024;
    let tree_lines_sdf = d_line1.min(d_line2).min(d_line3);

    let d_hub_circle = ((u - node_hub.0).powi(2) + (v - node_hub.1).powi(2)).sqrt() - 0.135;
    let d_top_circle = ((u - node_top.0).powi(2) + (v - node_top.1).powi(2)).sqrt() - 0.105;
    let d_mid_circle = ((u - node_mid.0).powi(2) + (v - node_mid.1).powi(2)).sqrt() - 0.075;
    let d_tl_circle = ((u - node_top_left.0).powi(2) + (v - node_top_left.1).powi(2)).sqrt() - 0.075;

    let nodes_sdf = d_hub_circle
        .min(d_top_circle)
        .min(d_mid_circle)
        .min(d_tl_circle);

    // Soft outer glow around waves & nodes
    let combined_fg_sdf = wave_sdf.min(nodes_sdf).min(tree_lines_sdf);
    let glow = (-combined_fg_sdf.max(0.0) * 5.5).exp() * 0.42;
    bg_r += glow * 0.10;
    bg_g += glow * 0.75;
    bg_b += glow * 0.95;

    // Draw connecting tree lines (bright cyan)
    let line_mask = smoothstep(aa, -aa, tree_lines_sdf);
    bg_r = lerp(bg_r, 0.30, line_mask * 0.9);
    bg_g = lerp(bg_g, 0.88, line_mask * 0.9);
    bg_b = lerp(bg_b, 0.98, line_mask * 0.9);

    // Draw MQTT broadcast arcs (gradient from cyan #22D3EE to emerald #34D399)
    let wave_mask = smoothstep(aa, -aa, wave_sdf);
    let wave_grad = ((r - 0.25) / 0.75).clamp(0.0, 1.0);
    let wave_r = lerp(0.14, 0.20, wave_grad);
    let wave_g = lerp(0.84, 0.95, wave_grad);
    let wave_b = lerp(0.98, 0.68, wave_grad);
    bg_r = lerp(bg_r, wave_r, wave_mask);
    bg_g = lerp(bg_g, wave_g, wave_mask);
    bg_b = lerp(bg_b, wave_b, wave_mask);

    // Draw nodes (crisp white-cyan core with emerald/cyan ring)
    let node_mask = smoothstep(aa, -aa, nodes_sdf);
    let inner_cores = (d_hub_circle + 0.055)
        .min(d_top_circle + 0.045)
        .min(d_mid_circle + 0.032)
        .min(d_tl_circle + 0.032);
    let core_mask = smoothstep(aa, -aa, inner_cores);

    let _ = angle;
    bg_r = lerp(bg_r, 0.18, node_mask);
    bg_g = lerp(bg_g, 0.92, node_mask);
    bg_b = lerp(bg_b, 0.82, node_mask);

    bg_r = lerp(bg_r, 0.96, core_mask);
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
