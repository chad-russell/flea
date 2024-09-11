pub mod geometry;
pub mod layout;
pub mod quad_backend;
pub mod render;

use std::sync::{Arc, Mutex};

use floem_reactive::{create_effect, create_rw_signal, RwSignal};
use winit::event::ElementState;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowId},
};

use geometry::{Point, Rect, Size};
use layout::{DefaultLayouter, LayoutConstraint, Layouter, PaddedLayouter, RowLayouter, SizedBoxLayouter};
use quad_backend::QuadBackend;
use render::{DefaultRenderer, QuadRenderer, RenderContext, Renderer};

fn main() {
    pollster::block_on(run());
}

async fn run() {
    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App {
        state: None,
        window: None,
        keydown_callbacks: Default::default(),
    };
    event_loop.run_app(&mut app).unwrap();
}

pub struct State {
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub win_size: winit::dpi::PhysicalSize<u32>,
    pub widgets: Vec<usize>,
    pub children: Vec<Vec<usize>>,
    pub rects: Vec<Rect>,
    pub layouters: Vec<Box<dyn Layouter>>,
    pub renderers: Vec<Arc<Mutex<dyn Renderer>>>,
    pub quad_backend: QuadBackend,
    pub needs_render: RwSignal<Vec<usize>>,
}

impl State {
    pub async fn new(window: Arc<Window>) -> State {
        let size = window.inner_size();

        // The instance is a handle to our GPU
        // Backends::all => Vulkan + Metal + DX12 + Browser WebGPU
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            #[cfg(not(target_arch = "wasm32"))]
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });

        let surface = instance.create_surface(window).unwrap();

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .unwrap();

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    memory_hints: wgpu::MemoryHints::default(),
                    label: None,
                },
                None, // Trace path
            )
            .await
            .unwrap();

        let surface_caps = surface.get_capabilities(&adapter);
        // Shader code in this tutorial assumes an sRGB surface texture. Using a different
        // one will result in all the colors coming out darker. If you want to support non
        // sRGB surfaces, you'll need to account for that when drawing to the frame.
        let surface_format = surface_caps
            .formats
            .iter()
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

        let quad_backend = QuadBackend::new(&device, &config, size);

        surface.configure(&device, &config);

        Self {
            surface,
            device,
            queue,
            config,
            win_size: size,
            widgets: Vec::new(),
            children: Vec::new(),
            rects: Vec::new(),
            layouters: Vec::new(),
            renderers: Vec::new(),
            quad_backend,
            needs_render: create_rw_signal(Vec::new()),
        }
    }

    fn push_widget<L, R>(&mut self, layouter: L, renderer: Arc<Mutex<R>>) -> usize
    where
        L: Layouter + 'static,
        R: Renderer + 'static,
    {
        let id = self.widgets.len();

        self.widgets.push(id);
        self.children.push(vec![]);
        self.rects.push(Rect {
            pos: Point { x: 0, y: 0 },
            size: Size {
                width: 0,
                height: 0,
            },
        });

        self.layouters.push(Box::new(layouter));
        self.renderers.push(renderer);

        id
    }

    pub fn resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
        self.quad_backend.resize(new_size);

        if new_size.width > 0 && new_size.height > 0 {
            self.win_size = new_size;
            self.config.width = new_size.width;
            self.config.height = new_size.height;
            self.surface.configure(&self.device, &self.config);
        }

        for layouter in self.layouters.iter_mut() {
            layouter.prepare();
        }

        // Hardcode root rect to full window size
        self.rects[0] = Rect {
            pos: Point { x: 0, y: 0 },
            size: Size {
                width: self.win_size.width,
                height: self.win_size.height,
            },
        };
    }

    pub fn layout_root(&mut self) {
        for layouter in self.layouters.iter_mut() {
            layouter.prepare();
        }

        let constraint = LayoutConstraint {
            min_width: self.win_size.width,
            min_height: self.win_size.height,
            max_width: self.win_size.width,
            max_height: self.win_size.height,
        };

        self.layout_id(0, constraint);
    }

    pub fn layout_id(&mut self, id: usize, constraint: LayoutConstraint) {
        for child_id in self.children[id].clone() {
            let child_constraint = self.layouters[id].constrain_child(constraint);
            self.layout_id(child_id, child_constraint);
            self.layouters[id].child_sized(self.rects[child_id].size);
        }

        for child_id in self.children[id].clone() {
            let child_offset = self.layouters[id].position_child(self.rects[child_id].size);
            let base_pos = self.rects[id].pos;

            self.rects[child_id].pos = Point {
                x: base_pos.x + child_offset.x,
                y: base_pos.y + child_offset.y,
            }
        }

        self.rects[id].size = self.layouters[id].compute_size(constraint);
    }

    pub fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        self.quad_backend.clear();

        for (idx, renderer) in self.renderers.iter_mut().enumerate() {
            renderer.lock().unwrap().render(&mut RenderContext {
                rect: self.rects[idx],
                quad_backend: &mut self.quad_backend,
            });
        }

        let output = self.surface.get_current_texture()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Render Encoder"),
            });

        self.quad_backend.render(&mut encoder, &self.queue, &view);

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }
}

struct Widget<L, R> where L: Layouter, R: Renderer {
    layouter: L,
    renderer: R,
}

impl<L, R> Widget<L, R> where L: Layouter, R: Renderer {
    fn new(layouter: L, renderer: R) -> Self {
        Self { layouter, renderer }
    }
}

struct WidgetBuilder<L, R> where L: Layouter, R: Renderer {
    layouter: L,
    renderer: R,
}

impl WidgetBuilder<DefaultLayouter, DefaultRenderer> {
    fn new() -> Self {
        WidgetBuilder { layouter: DefaultLayouter{}, renderer: DefaultRenderer{} }
    }
}

impl<L, R> WidgetBuilder<L, R> where L: Layouter, R: Renderer {
    fn with_layouter<LL>(self, layouter: LL) -> WidgetBuilder<LL, R> where LL: Layouter {
        WidgetBuilder { layouter, renderer: self.renderer }
    }

    fn with_renderer<RR>(self, renderer: RR) -> WidgetBuilder<L, RR> where RR: Renderer {
        WidgetBuilder { layouter: self.layouter, renderer }
    }

    fn build(self) -> Widget<L, R> {
        Widget::new(self.layouter, self.renderer)
    }
}

struct App {
    state: Option<Arc<Mutex<State>>>,
    window: Option<Arc<Window>>,
    keydown_callbacks: Vec<Box<dyn Fn() + 'static>>,
}

impl App {
    fn on_keydown(&mut self, callback: impl Fn() + 'static) {
        self.keydown_callbacks.push(Box::new(callback));
    }

    fn push_widget<L, R>(&mut self, widget: Widget<L, R>) -> usize
    where
        L: Layouter + 'static,
        R: Renderer + 'static,
    {
        let renderer = Arc::new(Mutex::new(widget.renderer));

        let id = self
            .state
            .as_ref()
            .unwrap()
            .lock()
            .unwrap()
            .push_widget(widget.layouter, renderer.clone());

        let state = self.state.as_ref().unwrap().clone();
        create_effect(move |_| {
            println!("Effect - rendering {}", id);
            let mut state = state.lock().unwrap();
            renderer.lock().unwrap().render(&mut RenderContext {
                rect: state.rects[id],
                quad_backend: &mut state.quad_backend,
            });
            state.needs_render.update(|n| n.push(id));
        });

        id
    }

    fn push_child(&mut self, parent: usize, child: usize) {
        self.state.as_ref().unwrap().lock().unwrap().children[parent].push(child);
    }

    fn push_children(&mut self, parent: usize, children: &[usize]) {
        self.state.as_ref().unwrap().lock().unwrap().children[parent]
            .append(&mut children.to_vec());
    }

    fn setup(&mut self) {
        let c1 = create_rw_signal([0.0, 0.5, 0.4]);
        let c2 = create_rw_signal([0.0, 0.5, 0.4]);

        self.on_keydown(move || {
            if rand::random::<bool>() {
                c1.set(rand::random());
            } else {
                c2.set(rand::random());
            }
        });

        let root_widget = WidgetBuilder::new().with_layouter(PaddedLayouter::new(100, 100, 100, 100)).build();
        let root = self.push_widget(root_widget);

        let row = self.push_widget(WidgetBuilder::new().with_layouter(RowLayouter::default()).build());
        self.push_child(root, row);

        let c1 = self.push_widget(
            WidgetBuilder::new().with_layouter(SizedBoxLayouter::new(Size {
                width: 200,
                height: 200,
            })).with_renderer(
            QuadRenderer { color: c1 }).build(),
        );
        let c2 = self.push_widget(
            WidgetBuilder::new().with_layouter(SizedBoxLayouter::new(Size {
                width: 200,
                height: 200,
            })).with_renderer(
            QuadRenderer { color: c2 }).build(),
        );
        self.push_children(row, &[c1, c2]);
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window = event_loop
            .create_window(Window::default_attributes())
            .unwrap();
        let window = Arc::new(window);

        let state = Arc::new(Mutex::new(pollster::block_on(State::new(Arc::clone(
            &window,
        )))));

        self.window = Some(window);
        self.state = Some(state);

        self.setup();

        let needs_render = self
            .state
            .as_ref()
            .unwrap()
            .lock()
            .unwrap()
            .needs_render
            .clone();
        
        let window = self.window.as_ref().unwrap().clone();

        create_effect(move |_| {
            needs_render.get();
            window.request_redraw();
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                for id in self
                    .state
                    .as_ref()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .needs_render
                    .get()
                {
                    println!("Rendering {}", id);
                }

                if !self
                    .state
                    .as_ref()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .needs_render
                    .get()
                    .is_empty()
                {
                    self.state
                        .as_mut()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .render()
                        .unwrap();
                }

                self.state
                    .as_mut()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .needs_render
                    .update(|n| n.clear());
            }
            WindowEvent::Resized(new_size) => {
                self.state
                    .as_mut()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .resize(new_size);

                self.state.as_mut().unwrap().lock().unwrap().layout_root();

                self.state.as_mut().unwrap().lock().unwrap().render().unwrap();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                // self.color.set([0.8, 0.1, 0.0]);

                for keydown in self.keydown_callbacks.iter() {
                    keydown();
                }
            }
            _ => (),
        }
    }
}
