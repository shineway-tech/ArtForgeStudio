use std::env;
use std::fs;
use std::io::{Cursor, Write};
use std::path::PathBuf;

fn main() {
    // 1) Slint UI 编译
    let cfg = slint_build::CompilerConfiguration::new().with_style("fluent".to_string());
    slint_build::compile_with_config("../../ui/main.slint", cfg).expect("Slint build failed");

    let profile = env::var("PROFILE").unwrap_or_default();
    if profile != "release" {
        return;
    }

    // 2) 程序化生成 ArtAIT 图标 (256x256 → ICO)
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let ico_path = out_dir.join("artait.ico");

    let png_bytes = render_icon_png(256);
    let ico_bytes = wrap_png_in_ico(&png_bytes, 256);
    fs::write(&ico_path, &ico_bytes).expect("write ico");

    // 3) Windows 元信息 + 图标
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon(ico_path.to_str().expect("ico path"));
        res.set("FileDescription", "ArtForge Studio - AI 美术生产套件");
        res.set("ProductName", "ArtForge Studio");
        res.set("CompanyName", "ArtForge Studio");
        res.set("LegalCopyright", "© 2026 ArtForge Studio");
        res.set("OriginalFilename", "ArtForgeStudio.exe");
        res.set("InternalName", "ArtForgeStudio");
        res.set("FileVersion", "0.1.0.0");
        res.set("ProductVersion", "0.1.0.0");
        if let Err(e) = res.compile() {
            eprintln!("cargo:warning=winresource compile failed: {e}");
        }
    }
}

/// 256×256 ArtForge 图标：圆角紫蓝渐变方块 + 白色 "A"
fn render_icon_png(size: u32) -> Vec<u8> {
    use image::{ImageBuffer, Rgba};

    let mut img = ImageBuffer::<Rgba<u8>, Vec<u8>>::new(size, size);
    let radius = (size as f32) * 0.22;
    let s = size as f32;

    // 渐变填充 + 圆角
    for y in 0..size {
        for x in 0..size {
            let fx = x as f32;
            let fy = y as f32;

            let inside = is_inside_rounded_rect(fx, fy, s, s, radius);
            if !inside {
                img.put_pixel(x, y, Rgba([0, 0, 0, 0]));
                continue;
            }
            // 对角线渐变：左上 #4A90E2 → 右下 #5B5BD6
            let t = (fx + fy) / (2.0 * s);
            let t = t.clamp(0.0, 1.0);
            let r = lerp(0x4A, 0x5B, t);
            let g = lerp(0x90, 0x5B, t);
            let b = lerp(0xE2, 0xD6, t);
            img.put_pixel(x, y, Rgba([r, g, b, 255]));
        }
    }

    // 简化 "A" 字形：三角形外框 + 横杠
    draw_letter_a(&mut img, size);

    let mut out = Cursor::new(Vec::with_capacity(64 * 1024));
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("encode png");
    out.into_inner()
}

fn is_inside_rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> bool {
    let dx = (x - r).max(0.0).min(w - 2.0 * r) - (x - r);
    let dy = (y - r).max(0.0).min(h - 2.0 * r) - (y - r);
    let dx = dx.max(0.0);
    let dy = dy.max(0.0);
    if x >= r && x < w - r {
        return y >= 0.0 && y < h;
    }
    if y >= r && y < h - r {
        return x >= 0.0 && x < w;
    }
    let _ = (dx, dy);
    let cx = if x < r { r } else { w - r };
    let cy = if y < r { r } else { h - r };
    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy <= r * r
}

fn lerp(a: u8, b: u8, t: f32) -> u8 {
    let v = (a as f32) * (1.0 - t) + (b as f32) * t;
    v.round().clamp(0.0, 255.0) as u8
}

fn draw_letter_a(img: &mut image::ImageBuffer<image::Rgba<u8>, Vec<u8>>, size: u32) {
    use image::Rgba;
    let s = size as f32;
    let cx = s * 0.5;
    let top_y = s * 0.22;
    let bot_y = s * 0.78;
    let half_base = s * 0.22;
    let stroke = (s * 0.075).max(2.0);
    let bar_y = s * 0.58;
    let white = Rgba([255, 255, 255, 255]);

    for y in 0..size {
        for x in 0..size {
            let fx = x as f32;
            let fy = y as f32;
            if fy < top_y || fy > bot_y {
                continue;
            }
            // 左边斜线
            let t = (fy - top_y) / (bot_y - top_y);
            let left = cx - t * half_base;
            let right = cx + t * half_base;
            // 外框两道斜线
            let on_left = (fx - left).abs() < stroke / 2.0;
            let on_right = (fx - right).abs() < stroke / 2.0;
            // 横杠
            let on_bar = fy >= bar_y - stroke / 2.0
                && fy <= bar_y + stroke / 2.0
                && fx >= left
                && fx <= right;
            if on_left || on_right || on_bar {
                img.put_pixel(x, y, white);
            }
        }
    }
}

/// 把单张 PNG 包装成 ICO（PNG 直接嵌入式，Vista+ 支持）
fn wrap_png_in_ico(png: &[u8], size: u32) -> Vec<u8> {
    let mut buf = Vec::with_capacity(22 + png.len());
    // ICONDIR
    buf.write_all(&[0, 0]).unwrap(); // reserved
    buf.write_all(&[1, 0]).unwrap(); // type=1 (ICO)
    buf.write_all(&[1, 0]).unwrap(); // count=1

    // ICONDIRENTRY (16 bytes)
    let dim = if size >= 256 { 0u8 } else { size as u8 };
    buf.push(dim); // width
    buf.push(dim); // height
    buf.push(0); // color count
    buf.push(0); // reserved
    buf.write_all(&1u16.to_le_bytes()).unwrap(); // planes
    buf.write_all(&32u16.to_le_bytes()).unwrap(); // bpp
    buf.write_all(&(png.len() as u32).to_le_bytes()).unwrap();
    buf.write_all(&22u32.to_le_bytes()).unwrap(); // offset

    buf.write_all(png).unwrap();
    buf
}
