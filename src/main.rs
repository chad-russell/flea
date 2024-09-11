pub mod geometry;
pub mod layout;
pub mod quad_backend;
pub mod render;

use std::sync::{Arc, Mutex, MutexGuard};

use floem_reactive::{create_effect, create_rw_signal, RwSignal};
use winit::event::ElementState;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowId},
};

use geometry::{Point, Rect, Size};
use layout::{
    DefaultLayouter, LayoutConstraint, Layouter, PaddedLayouter, RowLayouter, SizedBoxLayouter,
};
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
    pub win_size: RwSignal<winit::dpi::PhysicalSize<u32>>,
    pub widgets: Vec<usize>,
    pub children: Vec<Vec<usize>>,
    pub rects: Vec<Rect>,
    pub layouters: Vec<Arc<Mutex<dyn Layouter>>>,
    pub renderers: Vec<Arc<Mutex<dyn Renderer>>>,
    pub quad_backend: QuadBackend,
    pub needs_layout: RwSignal<Vec<usize>>,
    pub needs_render: RwSignal<Vec<usize>>,
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

        let size = create_rw_signal(size);

        let quad_backend = QuadBackend::new(&device, &config, size.get());

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
            needs_layout: create_rw_signal(Vec::new()),
            needs_render: create_rw_signal(Vec::new()),
        }
    }

    fn push_widget<L, R>(&mut self, layouter: Arc<Mutex<L>>, renderer: Arc<Mutex<R>>) -> usize
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

        self.layouters.push(layouter);
        self.renderers.push(renderer);

        id
    }

    pub fn resize(&mut self) {
        let new_size = self.win_size.get();

        if new_size.width > 0 && new_size.height > 0 {
            self.config.width = new_size.width;
            self.config.height = new_size.height;
            self.surface.configure(&self.device, &self.config);
        }

        self.rects[0] = Rect {
            pos: Point { x: 0, y: 0 },
            size: Size {
                width: self.win_size.get().width,
                height: self.win_size.get().height,
            },
        };
    }

    pub fn layout_root(&mut self) {
        let constraint = LayoutConstraint {
            min_width: self.win_size.get().width,
            min_height: self.win_size.get().height,
            max_width: self.win_size.get().width,
            max_height: self.win_size.get().height,
        };

        self.layout_id(0, constraint);
    }

    pub fn layout_id(&mut self, id: usize, constraint: LayoutConstraint) {
        let layouter = self.layouters[id].clone();
        let mut layouter = layouter.lock().unwrap();

        layouter.prepare();

        for child_id in self.children[id].clone() {
            let child_constraint = layouter.constrain_child(constraint);
            self.layout_id(child_id, child_constraint);
            layouter.child_sized(self.rects[child_id].size);
        }

        for child_id in self.children[id].clone() {
            let child_offset = layouter.position_child(self.rects[child_id].size);
            let base_pos = self.rects[id].pos;

            self.rects[child_id].pos = Point {
                x: base_pos.x + child_offset.x,
                y: base_pos.y + child_offset.y,
            }
        }

        self.rects[id].size = layouter.compute_size(constraint);
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

pub struct Widget<L, R>
where
    L: Layouter,
    R: Renderer,
{
    pub layouter: L,
    pub renderer: R,
}

impl<L, R> Widget<L, R>
where
    L: Layouter,
    R: Renderer,
{
    pub fn new(layouter: L, renderer: R) -> Self {
        Self { layouter, renderer }
    }
}

pub struct WidgetBuilder<L, R>
where
    L: Layouter,
    R: Renderer,
{
    pub layouter: L,
    pub renderer: R,
}

impl WidgetBuilder<DefaultLayouter, DefaultRenderer> {
    pub fn new() -> Self {
        WidgetBuilder {
            layouter: DefaultLayouter {},
            renderer: DefaultRenderer {},
        }
    }
}

impl<L> WidgetBuilder<L, DefaultRenderer> where L: Layouter + 'static {
    pub fn from_layouter(layouter: L) -> WidgetBuilder<L, DefaultRenderer> {
        WidgetBuilder {
            layouter,
            renderer: DefaultRenderer {},
        }
    }
}

impl<R> WidgetBuilder<DefaultLayouter, R> where R: Renderer + 'static {
    pub fn from_renderer(renderer: R) -> WidgetBuilder<DefaultLayouter, R> {
        WidgetBuilder {
            layouter: DefaultLayouter {},
            renderer,
        }
    }
}

impl<L, R> WidgetBuilder<L, R>
where
    L: Layouter,
    R: Renderer,
{
    pub fn with_layouter<LL>(self, layouter: LL) -> WidgetBuilder<LL, R>
    where
        LL: Layouter,
    {
        WidgetBuilder {
            layouter,
            renderer: self.renderer,
        }
    }

    pub fn with_renderer<RR>(self, renderer: RR) -> WidgetBuilder<L, RR>
    where
        RR: Renderer,
    {
        WidgetBuilder {
            layouter: self.layouter,
            renderer,
        }
    }

    pub fn build(self) -> Widget<L, R> {
        Widget::new(self.layouter, self.renderer)
    }
}

impl<L, R> Into<Widget<L, R>> for WidgetBuilder<L, R>
where
    L: Layouter + 'static,
    R: Renderer + 'static,
{
    fn into(self) -> Widget<L, R> {
        self.build()
    }
}

impl<L, R> Into<WidgetTreeBuilder<L, R>> for WidgetBuilder<L, R> where L: Layouter + 'static, R: Renderer + 'static {
    fn into(self) -> WidgetTreeBuilder<L, R> {
        WidgetTreeBuilder::with_root(self)
    }
}

pub struct WidgetTreeBuilder<L, R>
where
    L: Layouter + 'static,
    R: Renderer + 'static,
{
    pub widget: Widget<L, R>,
    pub children: Vec<Box<dyn FnOnce(&mut App) -> usize>>,
}

impl WidgetTreeBuilder<DefaultLayouter, DefaultRenderer> {
    pub fn new() -> WidgetTreeBuilder<DefaultLayouter, DefaultRenderer> {
        WidgetTreeBuilder {
            widget: Widget::new(DefaultLayouter {}, DefaultRenderer {}),
            children: Vec::new(),
        }
    }
}

impl<L, R> WidgetTreeBuilder<L, R>
where
    L: Layouter + 'static,
    R: Renderer + 'static,
{
    pub fn with_root(widget: impl Into<Widget<L, R>>) -> WidgetTreeBuilder<L, R> {
        WidgetTreeBuilder {
            widget: widget.into(),
            children: Vec::new(),
        }
    }

    pub fn with_widget<LL, RR>(self, widget: impl Into<Widget<LL, RR>>) -> WidgetTreeBuilder<LL, RR>
    where
        LL: Layouter + 'static,
        RR: Renderer + 'static,
    {
        WidgetTreeBuilder {
            widget: widget.into(),
            children: self.children,
        }
    }

    pub fn child<LL, RR>(mut self, child: impl Into<WidgetTreeBuilder<LL, RR>> + 'static) -> Self
    where
        LL: Layouter + 'static,
        RR: Renderer + 'static,
    {
        self.children.push(Box::new(move |app| child.into().build(app)));
        self
    }

    fn build(self, app: &mut App) -> usize {
        let id = app.push_widget(self.widget);

        let child_ids = self
            .children
            .into_iter()
            .map(|child| child(app))
            .collect::<Vec<_>>();
        app.push_children(id, &child_ids);

        id
    }
}

pub struct App {
    pub state: Option<Arc<Mutex<State>>>,
    pub window: Option<Arc<Window>>,
    pub keydown_callbacks: Vec<Box<dyn Fn() + 'static>>,
}

impl App {
    pub fn on_keydown(&mut self, callback: impl Fn() + 'static) {
        self.keydown_callbacks.push(Box::new(callback));
    }

    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state.as_ref().unwrap().lock().unwrap()
    }

    pub fn push_widget<L, R>(&mut self, widget: Widget<L, R>) -> usize
    where
        L: Layouter + 'static,
        R: Renderer + 'static,
    {
        let layouter = Arc::new(Mutex::new(widget.layouter));
        let renderer = Arc::new(Mutex::new(widget.renderer));

        let id = self
            .state()
            .push_widget(layouter.clone(), renderer.clone());

        // let state = self.state.clone();
    //     create_effect(move |_| {
    //         println!("Effect - layout {}", id);

    //         let mut state = state.lock().unwrap();

    //         layouter.lock().unwrap().prepare();
    // // fn prepare(&mut self) {}
    // // fn constrain_child(&mut self, constraint: LayoutConstraint) -> LayoutConstraint;
    // // fn child_sized(&mut self, _size: Size) {}
    // // fn position_child(&mut self, size: Size) -> Point;
    // // fn compute_size(&mut self, constraint: LayoutConstraint) -> Size;
    //     });

        let state = self.state.clone().unwrap();
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

    pub fn push_child(&mut self, parent: usize, child: usize) {
        self.state.as_ref().unwrap().lock().unwrap().children[parent].push(child);
    }

    pub fn push_children(&mut self, parent: usize, children: &[usize]) {
        self.state.as_ref().unwrap().lock().unwrap().children[parent]
            .append(&mut children.to_vec());
    }

    pub fn setup(&mut self) {
        let c1 = create_rw_signal([0.0, 0.5, 0.4]);
        let c2 = create_rw_signal([0.0, 0.5, 0.4]);

        self.on_keydown(move || {
            if rand::random::<bool>() {
                c1.set(rand::random());
            } else {
                c2.set(rand::random());
            }
        });

        let row_of_boxes = 
            WidgetTreeBuilder::with_root(
                WidgetBuilder::from_layouter(RowLayouter::default()),
            )
            .child(
                WidgetBuilder::new()
                    .with_layouter(SizedBoxLayouter::new(Size {
                        width: 200,
                        height: 200,
                    }))
                    .with_renderer(QuadRenderer { color: c1 }),
            )
            .child(
                // child 2
                WidgetBuilder::new()
                    .with_layouter(SizedBoxLayouter::new(Size {
                        width: 200,
                        height: 200,
                    }))
                    .with_renderer(QuadRenderer { color: c2 }),
            );

        WidgetTreeBuilder::with_root(
            WidgetBuilder::from_layouter(PaddedLayouter::new(100, 100, 100, 100)),
        )
        .child(row_of_boxes)
        .build(self);

        // {
        //     let root_widget = WidgetBuilder::new().with_layouter(PaddedLayouter::new(100, 100, 100, 100)).build();
        //     let root = self.push_widget(root_widget);

        //     let row = self.push_widget(WidgetBuilder::new().with_layouter(RowLayouter::default()).build());
        //     self.push_child(root, row);

        //     let c1 = self.push_widget(
        //         WidgetBuilder::new().with_layouter(SizedBoxLayouter::new(Size {
        //             width: 200,
        //             height: 200,
        //         })).with_renderer(
        //         QuadRenderer { color: c1 }).build(),
        //     );
        //     let c2 = self.push_widget(
        //         WidgetBuilder::new().with_layouter(SizedBoxLayouter::new(Size {
        //             width: 200,
        //             height: 200,
        //         })).with_renderer(
        //         QuadRenderer { color: c2 }).build(),
        //     );
        //     self.push_children(row, &[c1, c2]);
        // }
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

        let win_size = self.state().win_size.clone();
        let needs_layout = self.state().needs_layout.clone();
        let needs_render = self.state().needs_render.clone();
        create_effect(move |_| {
            win_size.get();
            needs_layout.update(|n| n.push(0));
            needs_render.update(|n| n.push(0));
        });

        let needs_render = self.state().needs_render.clone();
        let window = self.window.as_ref().unwrap().clone();
        create_effect(move |_| {
            needs_render.get();
            window.request_redraw();
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if !self.state().needs_layout.get().is_empty() {
            self.state().layout_root();
            self.state().needs_layout.update(|n| n.clear());
        }

        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                for id in self.state().needs_render.get()
                {
                    println!("Draw - Rendering {}", id);
                }

                if !self.state().needs_render.get().is_empty()
                {
                    self.state().render().unwrap();
                    self.state().needs_render.update(|n| n.clear());
                }
            }
            WindowEvent::Resized(new_size) => {
                self.state().win_size.set(new_size);
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                for keydown in self.keydown_callbacks.iter() {
                    keydown();
                }
            }
            _ => (),
        }
    }
}
