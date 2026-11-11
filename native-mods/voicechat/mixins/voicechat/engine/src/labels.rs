//! Labels above players in the world: which voice chat group each is in, as
//! the camera the last frame drew from sees them. The projection is the
//! waypoints mod's.

/// The camera: its position, yaw and pitch in degrees, vertical field of view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub fov: f32,
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// `Entity.calculateViewVector`: 0 yaw faces south, positive pitch down.
fn forward(yaw: f32, pitch: f32) -> [f64; 3] {
    let (y, p) = (f64::from(yaw).to_radians(), f64::from(pitch).to_radians());
    [-y.sin() * p.cos(), -p.sin(), y.cos() * p.cos()]
}

/// The camera's forward, right and up. Right comes from the yaw alone, so
/// looking straight up or down still has one.
fn basis(yaw: f32, pitch: f32) -> ([f64; 3], [f64; 3], [f64; 3]) {
    let f = forward(yaw, pitch);
    let y = f64::from(yaw).to_radians();
    let r = [-y.cos(), 0.0, -y.sin()];
    (f, r, cross(r, f))
}

/// `point` on a `size` screen, as the world's perspective projection draws
/// it; None behind the camera.
pub fn project(camera: &Camera, point: [f64; 3], size: [f32; 2]) -> Option<[f32; 2]> {
    let (f, r, u) = basis(camera.yaw, camera.pitch);
    let d = sub(point, camera.pos);
    let z = dot(d, f);
    if z < 0.05 || size[1] <= 0.0 {
        return None;
    }
    let t = (f64::from(camera.fov).to_radians() / 2.0).tan();
    let aspect = f64::from(size[0]) / f64::from(size[1]);
    let nx = dot(d, r) / (z * t * aspect);
    let ny = dot(d, u) / (z * t);
    Some([((nx + 1.0) / 2.0 * f64::from(size[0])) as f32, ((1.0 - ny) / 2.0 * f64::from(size[1])) as f32])
}

/// Labels show for players this close.
pub const RANGE: f64 = 64.0;
/// Above the eyes, clear of the name tag.
const ABOVE_EYES: f64 = 0.95;

/// A label's text centred at `at` (GUI pixels), in `rgb`.
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub text: String,
    pub at: [f32; 2],
    pub rgb: u32,
}

/// The labels for `players` (eye position, group name, whether it is the
/// player's own group), the farthest first so the nearest draws on top.
pub fn place(camera: &Camera, size: [f32; 2], players: &[([f64; 3], String, bool)]) -> Vec<Label> {
    let mut placed: Vec<(f64, Label)> = players
        .iter()
        .filter_map(|(eye, group, ours)| {
            let point = [eye[0], eye[1] + ABOVE_EYES, eye[2]];
            let d = sub(point, camera.pos);
            let distance = dot(d, d).sqrt();
            if distance > RANGE {
                return None;
            }
            let at = project(camera, point, size)?;
            let on_screen = (0.0..=size[0]).contains(&at[0]) && (0.0..=size[1]).contains(&at[1]);
            on_screen.then(|| {
                (distance, Label { text: format!("[{group}]"), at, rgb: if *ours { 0x55FF55 } else { 0xFFFF55 } })
            })
        })
        .collect();
    placed.sort_by(|a, b| b.0.total_cmp(&a.0));
    placed.into_iter().map(|(_, l)| l).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_player_ahead_gets_a_label_above_the_middle() {
        let camera = Camera { pos: [0.0, 65.62, 0.0], yaw: 0.0, pitch: 0.0, fov: 70.0 };
        let size = [400.0, 200.0];
        let players = vec![
            ([0.0, 65.62, 10.0], "Builders".to_owned(), false),
            ([0.0, 65.62, 5.0], "Mine".to_owned(), true),
            ([0.0, 65.62, -10.0], "Behind".to_owned(), false),
            ([0.0, 65.62, 100.0], "Far".to_owned(), false),
        ];
        let labels = place(&camera, size, &players);
        assert_eq!(labels.len(), 2);
        // The farther one first, the nearer drawn over it.
        assert_eq!(labels[0].text, "[Builders]");
        assert_eq!((labels[1].text.as_str(), labels[1].rgb), ("[Mine]", 0x55FF55));
        assert!((labels[0].at[0] - 200.0).abs() < 0.01 && labels[0].at[1] < 100.0);
    }
}
