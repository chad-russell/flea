use floem_reactive::RwSignal;

use crate::{geometry::{Point, Rect, Size}, quad_backend::QuadBackend};

pub trait RenderContext {
    fn push_quad(&mut self, r: Rect, color: [f32; 3]);
    fn rect(&self) -> Rect;
}

pub struct DummyRenderContext {}

impl RenderContext for DummyRenderContext {
    fn push_quad(&mut self, _r: Rect, _color: [f32; 3]) {}
    fn rect(&self) -> Rect {
        Rect { pos: Point { x: 0, y: 0 }, size: Size { width: 0, height: 0 } }
    }
}

pub struct DefaultRenderContext<'a> {
    pub rect: Rect,
    pub quad_backend: &'a mut QuadBackend,
}

impl<'a> RenderContext for DefaultRenderContext<'a> {
    fn push_quad(&mut self, r: Rect, color: [f32; 3]) {
        self.quad_backend.push_quad(r, color);
    }

    fn rect(&self) -> Rect {
        self.rect
    }
}

pub trait Renderer {
    fn render(&mut self, ctx: &mut dyn RenderContext);
}

pub struct DefaultRenderer {}

impl Renderer for DefaultRenderer {
    fn render(&mut self, _ctx: &mut dyn RenderContext) { }
}

pub struct QuadRenderer {
    pub color: RwSignal<[f32; 3]>,
}

impl Renderer for QuadRenderer {
    fn render(&mut self, ctx: &mut dyn RenderContext) {
        ctx.push_quad(ctx.rect(), self.color.get());
    }
}