//! v315: 内置背景纹理（Impress 纹理填充）。
//!
//! 24 张 256×256 调色板 PNG，已做无缝处理，文件名即纹理名。
//! 用 `include_bytes!` 编译期内嵌（先例：roe-writer/src/font_registry.rs）。
//! 一格的页面尺寸：按 96 DPI 换算，1 像素 = 0.75pt，
//! 即 `tile_scale = 1.0` 时一格 192pt，与 PowerPoint、DeckCraft 一致，
//! pptx 的 `<a:tile sx sy>` 写 100000 即可，读写无需换算。

/// 纹理图片的边长（像素）。
pub const TEXTURE_PIXEL_SIZE: u32 = 256;

/// `tile_scale = 1.0` 时一格的边长（pt）。
pub const TEXTURE_TILE_PT: f32 = TEXTURE_PIXEL_SIZE as f32 * 0.75;

static TEXTURES: &[(&str, &[u8])] = &[
    ("blue_tissue", include_bytes!("../assets/textures/blue_tissue.png")),
    ("bouquet", include_bytes!("../assets/textures/bouquet.png")),
    ("brown_marble", include_bytes!("../assets/textures/brown_marble.png")),
    ("canvas", include_bytes!("../assets/textures/canvas.png")),
    ("cork", include_bytes!("../assets/textures/cork.png")),
    ("denim", include_bytes!("../assets/textures/denim.png")),
    ("fish_fossil", include_bytes!("../assets/textures/fish_fossil.png")),
    ("granite", include_bytes!("../assets/textures/granite.png")),
    ("green_marble", include_bytes!("../assets/textures/green_marble.png")),
    ("medium_wood", include_bytes!("../assets/textures/medium_wood.png")),
    ("newsprint", include_bytes!("../assets/textures/newsprint.png")),
    ("oak", include_bytes!("../assets/textures/oak.png")),
    ("paper_bag", include_bytes!("../assets/textures/paper_bag.png")),
    ("papyrus", include_bytes!("../assets/textures/papyrus.png")),
    ("parchment", include_bytes!("../assets/textures/parchment.png")),
    ("pink_tissue", include_bytes!("../assets/textures/pink_tissue.png")),
    ("purple_mesh", include_bytes!("../assets/textures/purple_mesh.png")),
    ("recycled_paper", include_bytes!("../assets/textures/recycled_paper.png")),
    ("sand", include_bytes!("../assets/textures/sand.png")),
    ("stationery", include_bytes!("../assets/textures/stationery.png")),
    ("walnut", include_bytes!("../assets/textures/walnut.png")),
    ("water_droplets", include_bytes!("../assets/textures/water_droplets.png")),
    ("white_marble", include_bytes!("../assets/textures/white_marble.png")),
    ("woven_mat", include_bytes!("../assets/textures/woven_mat.png")),
];

/// 全部内置纹理名（按字母序）。
pub fn texture_names() -> Vec<&'static str> {
    TEXTURES.iter().map(|(n, _)| *n).collect()
}

/// 按 PNG 字节反查内置纹理名（字节完全一致才算）；用于读回时还原纹理名。
pub fn texture_name_for_bytes(data: &[u8]) -> Option<&'static str> {
    TEXTURES.iter().find(|(_, d)| *d == data).map(|(n, _)| *n)
}

/// 按名字取 PNG 字节；未知名返回 None。
pub fn texture_bytes(name: &str) -> Option<&'static [u8]> {
    TEXTURES.iter().find(|(n, _)| *n == name).map(|(_, d)| *d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texture_count_is_24() {
        assert_eq!(texture_names().len(), 24);
    }

    #[test]
    fn texture_names_unique() {
        let mut v = texture_names();
        v.sort();
        let n = v.len();
        v.dedup();
        assert_eq!(v.len(), n);
    }

    #[test]
    fn all_textures_are_png() {
        for n in texture_names() {
            let d = texture_bytes(n).unwrap();
            assert!(d.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]), "{n} 不是 PNG");
            assert!(d.len() > 1000, "{n} 过小");
        }
    }

    #[test]
    fn lookup_known_and_unknown() {
        assert!(texture_bytes("walnut").is_some());
        assert!(texture_bytes("purple_mesh").is_some());
        assert!(texture_bytes("water_droplets").is_some());
        assert!(texture_bytes("nope").is_none());
    }

    #[test]
    fn name_for_bytes_roundtrip() {
        for n in texture_names() {
            assert_eq!(texture_name_for_bytes(texture_bytes(n).unwrap()), Some(n));
        }
        assert_eq!(texture_name_for_bytes(&[1, 2, 3]), None);
    }

    #[test]
    fn tile_size_is_192pt() {
        assert_eq!(TEXTURE_TILE_PT, 192.0);
    }
}
