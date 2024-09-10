use floem_reactive::RwSignal;

use crate::{geometry::Rect, quad_backend::QuadBackend};

pub struct RenderContext<'a> {
    pub rect: Rect,
    pub quad_backend: &'a mut QuadBackend,
}

pub trait Renderer {
    fn render(&mut self, ctx: RenderContext);
}

pub struct DefaultRenderer {}

impl Renderer for DefaultRenderer {
    fn render(&mut self, _ctx: RenderContext) { }
}

pub struct QuadRenderer {
    pub color: RwSignal<[f32; 3]>,
}

impl Renderer for QuadRenderer {
    fn render(&mut self, ctx: RenderContext) {
        ctx.quad_backend.push_quad(ctx.rect, self.color.get());
    }
}