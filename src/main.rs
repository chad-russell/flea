use std::sync::Arc;

use winit::{
    application::ApplicationHandler, event::WindowEvent, event_loop::{ActiveEventLoop, ControlFlow, EventLoop}, window::{Window, WindowId}
};

fn main() {
    pollster::block_on(run());
}

async fn run() {
    let event_loop = EventLoop::new().unwrap();

    // ControlFlow::Poll continuously runs the event loop, even if the OS hasn't
    // dispatched any events. This is ideal for games and similar applications.
    event_loop.set_control_flow(ControlFlow::Poll);

    // ControlFlow::Wait pauses the event loop if no events are available to process.
    // This is ideal for non-game applications that only update in response to user
    // input, and uses significantly less power/CPU time than ControlFlow::Poll.
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = App::default();
    event_loop.run_app(&mut app).unwrap();
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub color: [f32; 3],
}

impl Vertex {
    fn desc() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: std::mem::size_of::<[f32; 3]>() as wgpu::BufferAddress,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x3,
                }
            ]
        }
    }
}

#[derive(Copy, Clone)]
pub struct LayoutConstraint {
    pub min_width: u32,
    pub min_height: u32,
    pub max_width: u32,
    pub max_height: u32,
}

pub trait Layouter {
    fn prepare(&mut self) {}
    fn constrain_child(&mut self, constraint: LayoutConstraint) -> LayoutConstraint;
    fn child_sized(&mut self, _size: Size) {}
    fn position_child(&mut self, size: Size) -> Point;
    fn compute_size(&mut self, constraint: LayoutConstraint) -> Size;
}

pub struct RenderContext<'a> {
    pub rect: Rect,
    pub quad_backend: &'a mut QuadBackend,
}

pub trait Renderer {
    fn render(&mut self, ctx: RenderContext);
}

pub struct Tree {
    pub ids: Vec<usize>,
    pub children: Vec<Vec<usize>>,
}

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

pub struct DefaultLayouter {}

impl Layouter for DefaultLayouter {
    fn prepare(&mut self) {
        println!("prepare default");
    }

    fn constrain_child(&mut self, constraint: LayoutConstraint) -> LayoutConstraint {
        constraint
    }

    fn position_child(&mut self, _size: Size) -> Point {
        Point {
            x: 0,
            y: 0,
        }
    }

    fn compute_size(&mut self, constraint: LayoutConstraint) -> Size {
        Size { width: constraint.min_width, height: constraint.min_height }
    }
}

#[derive(Default, Copy, Clone)]
struct RowLayouter {
    max_height: u32,
    total_width: u32,
    child_x: u32,
}

impl Layouter for RowLayouter {
    fn prepare(&mut self) {
        self.max_height = 0;
        self.total_width = 0;
        self.child_x = 0;
    }
    
    fn constrain_child(&mut self, constraint: LayoutConstraint) -> LayoutConstraint {
        LayoutConstraint {
            min_width: constraint.min_width,
            min_height: constraint.min_height,
            max_width: constraint.max_width - self.total_width,
            max_height: constraint.max_height,
        }
    }

    fn child_sized(&mut self, size: Size) {
        self.max_height = self.max_height.max(size.height);
        self.total_width += size.width;
    }

    fn position_child(&mut self, size: Size) -> Point {
        self.child_x += size.width;
        Point {
            x: self.child_x - size.width,
            y: 0,
        }
    }

    fn compute_size(&mut self, _constraint: LayoutConstraint) -> Size {
        // todo(chad): what if this is outside _constraint?
        Size {
            width: self.total_width,
            height: self.max_height,
        }
    }
}

struct SizedBoxLayouter {
    size: Size,
}

impl SizedBoxLayouter {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            size: Size {
                width,
                height,
            }
        }
    }
}

impl Layouter for SizedBoxLayouter {
    fn constrain_child(&mut self, constraint: LayoutConstraint) -> LayoutConstraint {
        constraint
    }

    fn position_child(&mut self, _size: Size) -> Point {
        Point { x: 0, y: 0 }
    }

    fn compute_size(&mut self, _constraint: LayoutConstraint) -> Size {
        self.size
    }
}

struct DefaultRenderer {}

impl Renderer for DefaultRenderer {
    fn render(&mut self, _ctx: RenderContext) { }
}

pub struct QuadRenderer {
    pub color: [f32; 3],
}

impl Renderer for QuadRenderer {
    fn render(&mut self, ctx: RenderContext) {
        ctx.quad_backend.push_quad(ctx.rect, self.color);
    }
}

pub struct QuadBackend {
    pub render_pipeline: wgpu::RenderPipeline,
    pub vertex_buffer: wgpu::Buffer,
    pub vertex_buffer_data: Vec<Vertex>,
}

impl QuadBackend {
    pub fn new(device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) -> Self {
        let render_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Quad Backend Pipeline Layout"),
            bind_group_layouts: &[],
            push_constant_ranges: &[],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Render Pipeline"),
            layout: Some(&render_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[Vertex::desc()],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                // Setting this to anything other than Fill requires Features::NON_FILL_POLYGON_MODE
                polygon_mode: wgpu::PolygonMode::Fill,
                // Requires Features::DEPTH_CLIP_CONTROL
                unclipped_depth: false,
                // Requires Features::CONSERVATIVE_RASTERIZATION
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
            cache: None,
        });

        let vertex_buffer = device.create_buffer(
            &wgpu::BufferDescriptor {
                label: Some("Vertex Buffer"),
                size: 100 * std::mem::size_of::<Vertex>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }
        );

        let vertex_buffer_data=  Vec::<Vertex>::default();

        Self {
            render_pipeline,
            vertex_buffer,
            vertex_buffer_data,
        }
    }

    pub fn push_quad(&mut self, r: Rect, color: [f32; 3]) {
        let quad_data = vec![
            Vertex { position: [r.pos.x as f32 / 1000.0, r.pos.y as f32 / 1000.0, 0.0], color },
            Vertex { position: [(r.pos.x + r.size.width) as f32 / 1000.0, r.pos.y as f32 / 1000.0, 0.0], color },
            Vertex { position: [r.pos.x as f32 / 1000.0, (r.pos.y + r.size.height) as f32 / 1000.0, 0.0], color },
            Vertex { position: [(r.pos.x + r.size.width) as f32 / 1000.0, (r.pos.y + r.size.height) as f32 / 1000.0, 0.0], color },
        ];

        self.vertex_buffer_data.extend(&quad_data);
    }

    pub fn clear(&mut self) {
        self.vertex_buffer_data.clear();
    }

    pub fn render(&mut self, encoder: &mut wgpu::CommandEncoder, queue: &wgpu::Queue, view: &wgpu::TextureView) {
            let vertex_buffer_data = bytemuck::cast_slice(&self.vertex_buffer_data);
            queue.write_buffer(&self.vertex_buffer, 0, vertex_buffer_data);
                let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Render Pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.1,
                                g: 0.2,
                                b: 0.3,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    occlusion_query_set: None,
                    timestamp_writes: None,
                });

            render_pass.set_pipeline(&self.render_pipeline);
            render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            render_pass.draw(0..self.vertex_buffer_data.len() as u32, 0..1);
    }
}

pub struct State<'a> {
    pub surface: wgpu::Surface<'a>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub size: winit::dpi::PhysicalSize<u32>,
    pub tree: Tree,
    pub rects: Vec<Rect>,
    pub layouters: Vec<Box<dyn Layouter>>,
    pub renderers: Vec<Box<dyn Renderer>>,
    pub quad_backend: QuadBackend,
}

impl<'a> State<'a> {
    pub async fn new(window: Arc<Window>) -> State<'a> {
        let size = window.inner_size();

        // The instance is a handle to our GPU
        // Backends::all => Vulkan + Metal + DX12 + Browser WebGPU
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            #[cfg(not(target_arch="wasm32"))]
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });
        
        let surface = instance.create_surface(window).unwrap();

        let adapter = instance.request_adapter(
            &wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            },
        ).await.unwrap();

        let (device, queue) = adapter.request_device(
            &wgpu::DeviceDescriptor {
                required_features: wgpu::Features::empty(),
               required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
                label: None,
            },
            None, // Trace path
        ).await.unwrap();

        let surface_caps = surface.get_capabilities(&adapter);
        // Shader code in this tutorial assumes an sRGB surface texture. Using a different
        // one will result in all the colors coming out darker. If you want to support non
        // sRGB surfaces, you'll need to account for that when drawing to the frame.
        let surface_format = surface_caps.formats.iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(surface_caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width,
            height: size.height,
            present_mode: surface_caps.present_modes[0],
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };

        let quad_backend = QuadBackend::new(&device, &config);

        surface.configure(&device, &config);

        let tree = Tree {
            ids: Vec::new(),
            children: Vec::new(),
        };

        Self {
            surface,
            device,
            queue,
            config,
            size,
            tree,
            rects: Vec::new(),
            layouters: Vec::new(),
            renderers: Vec::new(),
            quad_backend,
        }
    }

    fn push_widget<L, R>(&mut self, layouter: L, renderer: R) -> usize where L: Layouter + 'static, R: Renderer + 'static {
        let id = self.tree.ids.len();

        self.tree.ids.push(id);
        self.tree.children.push(vec![]);
        self.rects.push(Rect { pos: Point { x: 0, y: 0 }, size: Size { width: 0, height: 0 } });

        self.layouters.push(Box::new(layouter));
        self.renderers.push(Box::new(renderer));

        id
    }

    pub fn resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
        if new_size.width > 0 && new_size.height > 0 {
            self.size = new_size;
            self.config.width = new_size.width;
            self.config.height = new_size.height;
            self.surface.configure(&self.device, &self.config);
        }

        let root = self.tree.ids[0];

        let constraint = LayoutConstraint {
            min_width: self.size.width,
            min_height: self.size.height,
            max_width: self.size.width,
            max_height: self.size.height,
        };

        for layouter in self.layouters.iter_mut() {
            layouter.prepare();
        }

        // Hardcode root rect to full window size
        self.rects[root] = Rect {
            pos: Point { x: 0, y: 0 },
            size: Size { width: self.size.width, height: self.size.height },
        };

        // todo(chad): make this fully recursive
        for child_id in self.tree.children[root].clone() {
            let child_constraint = self.layouters[root].constrain_child(constraint);
            self.rects[child_id].size = self.layouters[child_id].compute_size(child_constraint);
            self.layouters[root].child_sized(self.rects[child_id].size);
        }
        for child_id in self.tree.children[root].clone() {
            self.rects[child_id].pos = self.layouters[root].position_child(self.rects[child_id].size);
        }

        self.rects[root].size =  self.layouters[root as usize].compute_size(constraint);
    }

    pub fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        self.quad_backend.clear();
        
        for (idx, renderer) in self.renderers.iter_mut().enumerate() {
            renderer.render(RenderContext {
                rect: self.rects[idx],
                quad_backend: &mut self.quad_backend,
            });
        }

        let output = self.surface.get_current_texture()?;
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Render Encoder"),
        });

        self.quad_backend.render(&mut encoder, &self.queue, &view);
 
        // submit will accept anything that implements IntoIter
        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
    
        Ok(())
    }
}

#[derive(Default)]
struct App<'a> {
    state: Option<State<'a>>,
    window: Option<Arc<Window>>,
}

impl<'a> ApplicationHandler for App<'a> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        println!("Resumed!");

        let window = event_loop.create_window(Window::default_attributes()).unwrap();
        let window = Arc::new(window);
        let mut state = pollster::block_on(State::new(Arc::clone(&window)));

        let _root = state.push_widget(RowLayouter::default(), DefaultRenderer {});
        let c1 = state.push_widget(SizedBoxLayouter::new(50, 50), QuadRenderer { color: [0.0, 0.5, 0.4] });
        let c2 = state.push_widget(SizedBoxLayouter::new(20, 50), QuadRenderer { color: [0.8, 0.3, 0.0] });

        state.tree.children[0].append(&mut vec![c1, c2]);

        self.window = Some(window);
        self.state = Some(state);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        self.state.as_mut().unwrap().render().unwrap();
                self.window.as_ref().unwrap().request_redraw();

        match event {
            WindowEvent::CloseRequested => {
                println!("The close button was pressed; stopping");
                event_loop.exit();
            },
            WindowEvent::RedrawRequested => {
                // Redraw the application.
                //
                // It's preferable for applications that do not render continuously to render in
                // this event rather than in AboutToWait, since rendering in here allows
                // the program to gracefully handle redraws requested by the OS.

                // Draw.
                self.state.as_mut().unwrap().render().unwrap();

                // Queue a RedrawRequested event.
                //
                // You only need to call this if you've determined that you need to redraw in
                // applications which do not always need to. Applications that redraw continuously
                // can render here instead.
                self.window.as_ref().unwrap().request_redraw();
            }
            WindowEvent::Resized(new_size) => {
                self.state.as_mut().unwrap().resize(new_size);
            }
            _ => (),
        }
    }
}