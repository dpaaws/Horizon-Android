//! What the voice chat looks like: player heads and small pixel icons, as
//! rectangles the game's HUD and the menu both draw.

/// A player's face and the hat layer over it, one `0xRRGGBB` pixel each
/// (`0` in the hat where the skin has none): the game's `HeadSkin`.
pub type HeadPixels = ([[u32; 8]; 8], [[u32; 8]; 8]);

/// One drawn rectangle: `[x, y, width, height]` and its `0xAARRGGBB`.
pub type Rect = ([f32; 4], u32);

fn hash(text: &str) -> u32 {
    text.bytes().fold(2166136261u32, |h, b| (h ^ u32::from(b)).wrapping_mul(16777619))
}

/// A face for a player with no known skin (offline names): skin, hair and
/// eyes from the name, so each player keeps their own.
pub fn default_face(seed: &str) -> HeadPixels {
    const SKINS: [u32; 4] = [0xC69C6D, 0xE0AC84, 0x9A6B4F, 0xF1C9A5];
    const HAIRS: [u32; 5] = [0x3B2A1A, 0x1E1E1E, 0x7A4B1E, 0xB58B3A, 0x5B3A29];
    const EYES: [u32; 3] = [0x3F5FBF, 0x2E7D4F, 0x4A3A2A];
    let h = hash(seed);
    let skin = SKINS[(h % 4) as usize];
    let hair = HAIRS[((h >> 4) % 5) as usize];
    let eye = EYES[((h >> 8) % 3) as usize];
    let mut face = [[skin; 8]; 8];
    for x in 0..8 {
        face[0][x] = hair;
        face[1][x] = if x == 0 || x == 7 { hair } else { skin };
    }
    face[3][1] = 0xFFFFFF;
    face[3][2] = eye;
    face[3][5] = eye;
    face[3][6] = 0xFFFFFF;
    face[5][3] = 0x8A4B3A;
    face[5][4] = 0x8A4B3A;
    (face, [[0; 8]; 8])
}

/// The head as rectangles at the origin, one unit a skin pixel, same-coloured
/// neighbours joined.
pub fn head_rects(head: Option<&HeadPixels>, seed: &str) -> Vec<Rect> {
    let fallback;
    let (face, hat) = match head {
        Some(h) => h,
        None => {
            fallback = default_face(seed);
            &fallback
        }
    };
    let mut out = Vec::new();
    for y in 0..8 {
        let mut row = [0u32; 8];
        for x in 0..8 {
            row[x] = if hat[y][x] != 0 { hat[y][x] } else { face[y][x] };
        }
        let mut x = 0;
        while x < 8 {
            let start = x;
            while x < 8 && row[x] == row[start] {
                x += 1;
            }
            out.push(([start as f32, y as f32, (x - start) as f32, 1.0], 0xFF00_0000 | row[start]));
        }
    }
    out
}

/// `rows` of pixel art (`#` is a pixel) as rectangles at the origin.
pub fn icon_rects(rows: &[&str], rgb: u32) -> Vec<Rect> {
    let mut out = Vec::new();
    for (y, row) in rows.iter().enumerate() {
        let cells: Vec<char> = row.chars().collect();
        let mut x = 0;
        while x < cells.len() {
            if cells[x] == '#' {
                let start = x;
                while x < cells.len() && cells[x] == '#' {
                    x += 1;
                }
                out.push(([start as f32, y as f32, (x - start) as f32, 1.0], 0xFF00_0000 | rgb));
            } else {
                x += 1;
            }
        }
    }
    out
}

pub const MIC: [&str; 10] = [
    "..###..",
    ".#####.",
    ".#####.",
    ".#####.",
    "#.###.#",
    "#.###.#",
    ".#####.",
    "..###..",
    "...#...",
    "..###..",
];
pub const SLASH: [&str; 10] = [
    "#......",
    ".#.....",
    "..#....",
    "...#...",
    "....#..",
    ".....#.",
    "......#",
    ".......",
    ".......",
    ".......",
];
pub const SPEAKER: [&str; 8] = [
    "....#....",
    "...##..#.",
    "####.#..#",
    "####..#.#",
    "####..#.#",
    "####.#..#",
    "...##..#.",
    "....#....",
];
pub const HEADSET: [&str; 9] = [
    "..#####..",
    ".#.....#.",
    "#.......#",
    "#.......#",
    "##.....##",
    "##.....##",
    "##.....##",
    "##.....##",
    ".........",
];
pub const GROUP: [&str; 7] = [
    ".##.##...",
    "####.###.",
    ".##.##.##",
    "...#.#...",
    ".#######.",
    ".#######.",
    ".........",
];

/// The three bars of a talking voice, `phase` in seconds: each bar's height
/// from 1 to 7 pixels, moving like a level meter.
pub fn wave_heights(phase: f32, seed: u32) -> [f32; 3] {
    let mut out = [0.0; 3];
    for (i, h) in out.iter_mut().enumerate() {
        let t = phase * (7.0 + i as f32 * 2.3) + (seed % 17) as f32 + i as f32 * 1.7;
        *h = (3.5 + 2.8 * t.sin() + 0.7 * (t * 2.1).cos()).clamp(1.0, 7.0).round();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_head_is_eight_rows_and_the_hat_wins() {
        let mut face = [[0x112233; 8]; 8];
        let mut hat = [[0; 8]; 8];
        hat[0][3] = 0xFF0000;
        face[7][0] = 0x00FF00;
        let rects = head_rects(Some(&(face, hat)), "x");
        assert!(rects.iter().any(|(r, c)| r[1] == 0.0 && r[0] == 3.0 && *c == 0xFFFF0000));
        let area: f32 = rects.iter().map(|(r, _)| r[2] * r[3]).sum();
        assert_eq!(area, 64.0);
    }

    #[test]
    fn a_name_without_a_skin_keeps_its_own_face() {
        assert_eq!(default_face("Alex"), default_face("Alex"));
        assert_ne!(default_face("Alex"), default_face("Bea Z"));
        assert_eq!(icon_rects(&MIC, 0xFFFFFF).iter().map(|(r, _)| r[2]).sum::<f32>() as usize, MIC.iter().map(|r| r.matches('#').count()).sum());
    }
}
