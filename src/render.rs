use floem_reactive::RwSignal;

use crate::{geometry::Rect, quad_backend::QuadBackend};

pub struct RenderContext<'a> {
    pub rect: Rect,
    pub quad_backend: &'a mut QuadBackend,
}

impl<'a> RenderContext<'a> {
    fn push_quad(&mut self, r: Rect, color: [f32; 3]) {
        self.quad_backend.push_quad(r, color);
    }

    fn rect(&self) -> Rect {
        self.rect
    }
}

pub trait Renderer {
    fn render(&mut self, ctx: &mut RenderContext);
}

pub struct DefaultRenderer {}

impl Renderer for DefaultRenderer {
    fn render(&mut self, _ctx: &mut RenderContext) { }
}

pub struct QuadRenderer {
    pub color: RwSignal<[f32; 3]>,
}

impl Renderer for QuadRenderer {
    fn render(&mut self, ctx: &mut RenderContext) {
        ctx.push_quad(ctx.rect(), self.color.get());
    }
}