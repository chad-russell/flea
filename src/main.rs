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
use layout::{LayoutConstraint, Layouter, PaddedLayouter, RowLayouter, SizedBoxLayouter};
use quad_backend::QuadBackend;
use render::{DefaultRenderContext, DefaultRenderer, DummyRenderContext, QuadRenderer, Renderer};

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

    let mut app = App {
        state: None,
        window: None,
        keydown_callbacks: Default::default(),
        color: create_rw_signal([0.0, 0.5, 0.4]),
    };
    event_loop.run_app(&mut app).unwrap();
}

pub struct State<'a> {
    pub surface: wgpu::Surface<'a>,
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

impl<'a> State<'a> {
    pub async fn new(window: Arc<Window>) -> State<'a> {
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

    fn push_widget<L, R>(&mut self, layouter: L, renderer: R) -> usize
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

        let renderer = Arc::new(Mutex::new(renderer));
        self.renderers.push(renderer.clone());

        let renderer = renderer.clone();
        let needs_render = self.needs_render.clone();
        create_effect(move |_| {
            println!("Effect - rendering {}", id);
            renderer.lock().unwrap().render(&mut DummyRenderContext {});
            needs_render.update(|n| n.push(id));
        });

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
            renderer.lock().unwrap().render(&mut DefaultRenderContext {
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

        // submit will accept anything that implements IntoIter
        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }
}

struct App<'a> {
    state: Option<State<'a>>,
    window: Option<Arc<Window>>,
    keydown_callbacks: Vec<Box<dyn Fn() + 'static>>,
    color: RwSignal<[f32; 3]>,
}

impl<'a> App<'a> {
    fn on_keydown(&mut self, callback: impl Fn() + 'static) {
        self.keydown_callbacks.push(Box::new(callback));
    }
}

impl<'a> ApplicationHandler for App<'a> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window = event_loop
            .create_window(Window::default_attributes())
            .unwrap();
        let window = Arc::new(window);
        let mut state = pollster::block_on(State::new(Arc::clone(&window)));

        self.color = create_rw_signal([0.0, 0.5, 0.4]);

        let root = state.push_widget(PaddedLayouter::new(100, 100, 100, 100), DefaultRenderer {});
        let row = state.push_widget(RowLayouter::default(), DefaultRenderer {});
        let c1 = state.push_widget(
            SizedBoxLayouter::new(Size { width: 200, height: 200 }),
            QuadRenderer {
                color: self.color
            },
        );
        // let c2 = state.push_widget(SizedBoxLayouter::new(20, 50), QuadRenderer { color: [0.8, 0.3, 0.0] });

        let color = self.color.clone();
        self.on_keydown(move || {
            color.set([0.6, 0.0, 0.0]);
        });

        state.children[root].push(row);
        // state.children[row].append(&mut vec![c1, c2]);
        state.children[row].append(&mut vec![c1]);

        self.window = Some(window);
        self.state = Some(state);

        self.state.as_mut().unwrap().render().unwrap();
        self.window.as_ref().unwrap().request_redraw();

        let needs_render = self.state.as_ref().unwrap().needs_render.clone();
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
                for id in self.state.as_ref().unwrap().needs_render.get() {
                    println!("Rendering {}", id);
                }

                if !self.state.as_ref().unwrap().needs_render.get().is_empty() {
                    self.state.as_mut().unwrap().render().unwrap();
                }

                self.state.as_mut().unwrap().needs_render.set(Vec::new());
            }
            WindowEvent::Resized(new_size) => {
                self.state.as_mut().unwrap().resize(new_size);
                self.state.as_mut().unwrap().layout_root();
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
