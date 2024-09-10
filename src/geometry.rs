#[derive(Copy, Clone)]
pub struct Point {
    pub x: u32,
    pub y: u32,
}

#[derive(Copy, Clone)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

#[derive(Copy, Clone)]
pub struct Rect {
    pub pos: Point,
    pub size: Size,
}

pub fn to_screen_pos(p: Point, win_size: winit::dpi::PhysicalSize<u32>) -> [f32; 3] {
    let w = win_size.width as f32;
    let h = win_size.height as f32;

    let x = p.x as f32;
    let y = p.y as f32;

    // todo(chad): hack for now because macos is high dpi
    let dpi = 2.0;

    let x = (x - w / 2.0 * dpi) / w;
    let y = (y - h / 2.0 * dpi) / h;

    [x, y, 0.0]
}