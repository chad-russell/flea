pub mod geometry;
pub mod layout;
pub mod quad_backend;
pub mod render;
pub mod tree;

use std::sync::{Arc, Mutex, MutexGuard};

use floem_reactive::{create_effect, create_rw_signal, RwSignal};
use geometry::Size;
use tree::{Padded, SizedBox, Tree};
use winit::event::ElementState;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowId},
};

use quad_backend::QuadBackend;

fn main() {
    pollster::block_on(run());
}

async fn run() {
    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App {
        state: None,
        window: None,
        key: create_rw_signal(0),
    };
    event_loop.run_app(&mut app).unwrap();
}

pub struct State {
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub win_size: winit::dpi::PhysicalSize<u32>,
    pub layout_tree: Tree,
    pub quad_backend: QuadBackend,
}

impl State {
    pub async fn new(window: Arc<Window>) -> State {
        let size = window.inner_size();

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

        let mut layout_tree = Tree::new();
        layout_tree.push_with_child(
            Padded::new(50, 50, 50, 50), 
            SizedBox::new(Size { width: 200, height: 200 })
        );

        Self {
            surface,
            device,
            queue,
            config,
            quad_backend,
            win_size: size,
            layout_tree,
        } 
    }

    pub fn resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
        if new_size.width > 0 && new_size.height > 0 {
            self.config.width = new_size.width;
            self.config.height = new_size.height;
            self.surface.configure(&self.device, &self.config);

            self.quad_backend.win_size = new_size;
        }
    }

    pub fn layout(&mut self) {
        self.layout_tree.layout(Size { width: self.win_size.width, height: self.win_size.height });
    }

    pub fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        self.quad_backend.clear();

        for rect in self.layout_tree.rects.iter() {
            self.quad_backend.push_quad(*rect, rand::random());
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

pub struct App {
    pub state: Option<Arc<Mutex<State>>>,
    pub window: Option<Arc<Window>>,
    pub key: RwSignal<i32>,
}

impl App {
    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state.as_ref().unwrap().lock().unwrap()
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

        self.window = Some(window.clone());
        self.state = Some(state);

        let key = self.key.clone();
        create_effect(move |_| {
            key.get();
            window.request_redraw();
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                println!("Redraw requested");
                self.state().layout();
                self.state().render().unwrap();
            }
            WindowEvent::Resized(new_size) => {
                self.state().resize(new_size);
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                self.key.set((self.key.get() + 1) % 100);
            }
            _ => (),
        }
    }
}
