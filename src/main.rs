// Copyright 2024 the Vello Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Simple example.

use anyhow::Result;
use petgraph::graph::{DiGraph, NodeIndex};
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::num::NonZeroUsize;
use std::sync::Arc;
use vello::kurbo::{Affine, Point, Rect, RoundedRect, Size, Stroke};
use vello::peniko::color::palette;
use vello::peniko::Color;
use vello::util::{RenderContext, RenderSurface};
use vello::wgpu;
use vello::{AaConfig, Renderer, RendererOptions, Scene};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::Window;

/// Simple struct to hold the state of the renderer
#[derive(Debug)]
pub struct ActiveRenderState<'s> {
    // The fields MUST be in this order, so that the surface is dropped before the window
    surface: RenderSurface<'s>,
    window: Arc<Window>,
}

enum RenderState<'s> {
    Active(ActiveRenderState<'s>),
    // Cache a window so that it can be reused when the app is resumed after being suspended
    Suspended(Option<Arc<Window>>),
}

struct SimpleVelloApp<'s> {
    // The vello RenderContext which is a global context that lasts for the
    // lifetime of the application
    context: RenderContext,

    // An array of renderers, one per wgpu device
    renderers: Vec<Option<Renderer>>,

    // State for our example where we store the winit Window and the wgpu Surface
    state: RenderState<'s>,

    // A vello Scene which is a data structure which allows one to build up a
    // description a scene to be drawn (with paths, fills, images, text, etc)
    // which is then passed to a renderer for rendering
    scene: Scene,

    widget_tree: WidgetTree,
}

#[derive(Clone, Copy)]
struct Constraints {
    min: Size,
    max: Size,
}

#[derive(Clone, Copy)]
struct LayouterConstrainChildrenCtx {
    constraints: Constraints,
}

#[derive(Clone, Copy)]
struct LayoutChildWasSizedCtx {
    child_size: Size,
}

struct LayouterSizeSelfCtx<'a> {
    constraints: Constraints,
    dependencies: &'a mut Vec<NodeIndex>,
}

trait Layouter {
    fn callback_begin_layout(&mut self);
    fn constrain_child(&mut self, ctx: LayouterConstrainChildrenCtx) -> Constraints;
    fn child_was_sized_compute_position(&mut self, ctx: LayoutChildWasSizedCtx) -> Point;
    fn size_self(&mut self, signal_values: &SignalValues, ctx: LayouterSizeSelfCtx) -> Size;
    //fn callback_end_layout(&mut self);
}

struct RowLayouter {
    child_sizes: Vec<Size>,
}

impl Layouter for RowLayouter {
    fn callback_begin_layout(&mut self) {
        println!("RowLayouter::callback_begin_layout");
        self.child_sizes.clear();
    }

    fn constrain_child(&mut self, ctx: LayouterConstrainChildrenCtx) -> Constraints {
        println!("RowLayouter::constrain_child");
        Constraints {
            min: ctx.constraints.min,
            max: Size {
                width: ctx.constraints.max.width
                    - self
                        .child_sizes
                        .iter()
                        .fold(0.0, |acc, size| acc + size.width),
                height: ctx.constraints.max.height,
            },
        }
    }

    fn child_was_sized_compute_position(&mut self, ctx: LayoutChildWasSizedCtx) -> Point {
        println!("RowLayouter::child_was_sized: {:?}", ctx.child_size);
        self.child_sizes.push(ctx.child_size);
        Point::new(
            self.child_sizes
                .iter()
                .fold(0.0, |acc, size| acc + size.width),
            0.0,
        )
    }

    fn size_self(&mut self, _signal_values: &SignalValues, _ctx: LayouterSizeSelfCtx) -> Size {
        println!("RowLayouter::size_self");
        self.child_sizes
            .iter()
            .fold(Size::new(0.0, 0.0), |acc, size| acc + *size)
    }
}

struct DrawerCtx<'a> {
    rect: Rect,
    scene: &'a mut Scene,
}

trait Drawer {
    fn draw(&mut self, ctx: DrawerCtx);
}

struct Revision {
    last_updated: usize,
    valid_through: usize,
}

struct WidgetTreeWeight {
    layouter: Box<dyn Layouter>,
    drawer: Option<Box<dyn Drawer>>,
    position: Point,
    size: Size,
    layout_revision: Revision,
    draw_revision: Revision,
    layout_dependencies: Vec<NodeIndex>,
    draw_dependencies: Vec<NodeIndex>,
}

type SignalValues = RefCell<Vec<Box<RefCell<dyn Any>>>>;

struct WidgetTree {
    t: RefCell<DiGraph<RefCell<WidgetTreeWeight>, ()>>,
    root: Option<NodeIndex>,
    revision: usize,
    signal_values: SignalValues,
}

impl WidgetTree {
    pub fn new() -> Self {
        Self {
            t: RefCell::new(DiGraph::<RefCell<WidgetTreeWeight>, ()>::new()),
            root: None,
            revision: 0,
            signal_values: RefCell::new(Vec::new()),
        }
    }

    pub fn add_node(
        &mut self,
        layouter: Box<dyn Layouter>,
        drawer: Option<Box<dyn Drawer>>,
    ) -> NodeIndex {
        let idx = self.t.borrow_mut().add_node(RefCell::new(WidgetTreeWeight {
            layouter,
            drawer,
            position: Point::ORIGIN,
            size: Size::ZERO,
            layout_revision: Revision {
                last_updated: self.revision,
                valid_through: self.revision,
            },
            layout_dependencies: vec![],
            draw_revision: Revision {
                last_updated: self.revision,
                valid_through: self.revision,
            },
            draw_dependencies: vec![],
        }));

        if self.root.is_none() {
            self.root = Some(idx)
        }

        idx
    }

    pub fn layout(&mut self, constraints: Constraints) {
        let Some(root) = self.root else { return };
        self.layout_index(root, constraints);
    }

    fn layout_index(&mut self, index: NodeIndex, constraints: Constraints) -> Size {
        {
            let mut weight = self.t.borrow_mut();
            let mut weight = weight.node_weight_mut(index).unwrap().borrow_mut();
            weight.layouter.callback_begin_layout();
        }

        // todo(chad): performance
        let children = self
            .t
            .borrow()
            .neighbors_directed(index, petgraph::Direction::Outgoing)
            .collect::<Vec<_>>();

        for child in children {
            let child_constraints = {
                let mut child_weight = self.t.borrow_mut();
                let mut child_weight = child_weight.node_weight_mut(index).unwrap().borrow_mut();
                child_weight
                    .layouter
                    .constrain_child(LayouterConstrainChildrenCtx { constraints })
            };

            let child_size = self.layout_index(child, child_constraints);
            self.t
                .borrow_mut()
                .node_weight_mut(child)
                .unwrap()
                .borrow_mut()
                .size = child_size;

            {
                let mut child_weight = self.t.borrow_mut();
                let mut child_weight = child_weight.node_weight_mut(index).unwrap().borrow_mut();
                let child_position = child_weight
                    .layouter
                    .child_was_sized_compute_position(LayoutChildWasSizedCtx { child_size });
                child_weight.position = child_position;
            }
        }

        let mut weight = self.t.borrow_mut();
        let mut weight = weight.node_weight_mut(index).unwrap().borrow_mut();
        weight.layouter.size_self(
            &self.signal_values,
            LayouterSizeSelfCtx {
                constraints,
                // dependencies: &mut weight.borrow_mut().layout_dependencies,
                dependencies: &mut Vec::new(),
            },
        )
        // Size::ZERO
    }

    pub fn draw_index(&mut self, index: NodeIndex, scene: &mut Scene, offset_pos: Point) {
        let position = {
            let mut weight = self.t.borrow_mut();
            let mut weight = weight.node_weight_mut(index).unwrap().borrow_mut();
            let position = weight.position;
            let size = weight.size;
            weight.drawer.as_mut().map(|d| {
                d.draw(DrawerCtx {
                    scene,
                    rect: Rect::from_origin_size(position, size),
                });
            });
            position
        };

        // todo(chad): performance
        let neighbors = self
            .t
            .borrow()
            .neighbors_directed(index, petgraph::Direction::Outgoing)
            .collect::<Vec<_>>();
        for child in neighbors {
            let offset_pos = Point::new(offset_pos.x + position.x, offset_pos.y + position.y);
            self.draw_index(child, scene, offset_pos);
        }
    }

    pub fn draw(&mut self, scene: &mut Scene) {
        let Some(root) = self.root else { return };
        self.draw_index(root, scene, Point::ORIGIN);
    }

    fn create_signal(&self, value: Size) -> Signal<Size> {
        let id = self.signal_values.borrow().len();
        self.signal_values
            .borrow_mut()
            .push(Box::new(RefCell::new(value)));
        Signal {
            id: SignalId(id),
            ty: PhantomData,
        }
    }
}

impl ApplicationHandler for SimpleVelloApp<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let RenderState::Suspended(cached_window) = &mut self.state else {
            return;
        };

        // Get the winit window cached in a previous Suspended event or else create a new window
        let window = cached_window
            .take()
            .unwrap_or_else(|| create_winit_window(event_loop));

        // Create a vello Surface
        let size = window.inner_size();
        let surface_future = self.context.create_surface(
            window.clone(),
            size.width,
            size.height,
            wgpu::PresentMode::AutoVsync,
        );
        let surface = pollster::block_on(surface_future).expect("Error creating surface");

        // Create a vello Renderer for the surface (using its device id)
        self.renderers
            .resize_with(self.context.devices.len(), || None);
        self.renderers[surface.dev_id]
            .get_or_insert_with(|| create_vello_renderer(&self.context, &surface));

        // Save the Window and Surface to a state variable
        self.state = RenderState::Active(ActiveRenderState { window, surface });
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        if let RenderState::Active(state) = &self.state {
            self.state = RenderState::Suspended(Some(state.window.clone()));
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        // Ignore the event (return from the function) if
        //   - we have no render_state
        //   - OR the window id of the event doesn't match the window id of our render_state
        //
        // Else extract a mutable reference to the render state from its containing option for use below
        let render_state = match &mut self.state {
            RenderState::Active(state) if state.window.id() == window_id => state,
            _ => return,
        };

        match event {
            // Exit the event loop when a close is requested (e.g. window's close button is pressed)
            WindowEvent::CloseRequested => event_loop.exit(),

            // Resize the surface when the window is resized
            WindowEvent::Resized(size) => {
                self.context
                    .resize_surface(&mut render_state.surface, size.width, size.height);
            }

            // This is where all the rendering happens
            WindowEvent::RedrawRequested => {
                // Empty the scene of objects to draw. You could create a new Scene each time, but in this case
                // the same Scene is reused so that the underlying memory allocation can also be reused.
                self.scene.reset();

                // Get the RenderSurface (surface + config)
                let surface = &render_state.surface;

                // Re-add the objects to draw to the scene.
                // add_shapes_to_scene(&mut self.scene);
                self.widget_tree.layout(Constraints {
                    min: Size::new(0.0, 0.0),
                    max: Size::new(surface.config.width as f64, surface.config.height as f64),
                });
                self.widget_tree.draw(&mut self.scene);

                // Get the window size
                let width = surface.config.width;
                let height = surface.config.height;

                // Get a handle to the device
                let device_handle = &self.context.devices[surface.dev_id];

                // Get the surface's texture
                let surface_texture = surface
                    .surface
                    .get_current_texture()
                    .expect("failed to get surface texture");

                // Render to the surface's texture
                self.renderers[surface.dev_id]
                    .as_mut()
                    .unwrap()
                    .render_to_surface(
                        &device_handle.device,
                        &device_handle.queue,
                        &self.scene,
                        &surface_texture,
                        &vello::RenderParams {
                            base_color: palette::css::BLACK, // Background color
                            width,
                            height,
                            antialiasing_method: AaConfig::Msaa16,
                        },
                    )
                    .expect("failed to render to surface");

                // Queue the texture to be presented on the surface
                surface_texture.present();

                device_handle.device.poll(wgpu::Maintain::Poll);
            }
            _ => {}
        }
    }
}

struct SimpleQuadDrawer {}

impl Drawer for SimpleQuadDrawer {
    fn draw(&mut self, ctx: DrawerCtx) {
        // Draw an outlined rectangle
        let stroke = Stroke::new(6.0);
        let rect = RoundedRect::new(ctx.rect.x0, ctx.rect.y0, ctx.rect.x1, ctx.rect.y1, 20.0);
        let rect_stroke_color = Color::new([0.9804, 0.702, 0.5294, 1.]);
        let rect_fill_color = Color::new([0.6, 0.5, 0.3, 1.]);
        ctx.scene.fill(
            vello::peniko::Fill::NonZero,
            Affine::IDENTITY,
            rect_fill_color,
            None,
            &rect,
        );
        ctx.scene
            .stroke(&stroke, Affine::IDENTITY, rect_stroke_color, None, &rect);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct SignalId(usize);

#[derive(Clone, Copy)]
struct Signal<T> {
    id: SignalId,
    ty: PhantomData<T>,
}

impl<T> Signal<T>
where
    T: Clone + 'static,
{
    fn get(&self, signal_values: &SignalValues) -> T {
        let value = &signal_values.borrow()[self.id.0];
        let value = value.borrow();
        value.downcast_ref::<T>().unwrap().clone()
    }
}

struct ReactiveSizedBoxLayouter {
    size: Signal<Size>,
}

impl ReactiveSizedBoxLayouter {
    fn new(tree: &mut WidgetTree, size: Size) -> Self {
        Self {
            size: tree.create_signal(size),
        }
    }
}

impl Layouter for ReactiveSizedBoxLayouter {
    fn size_self(&mut self, signal_values: &SignalValues, _ctx: LayouterSizeSelfCtx) -> Size {
        self.size.get(signal_values)
    }

    fn callback_begin_layout(&mut self) {}

    fn constrain_child(&mut self, ctx: LayouterConstrainChildrenCtx) -> Constraints {
        ctx.constraints
    }

    fn child_was_sized_compute_position(&mut self, _ctx: LayoutChildWasSizedCtx) -> Point {
        Point::ORIGIN
    }
}

struct SizedBoxLayouter {
    size: Size,
}

impl Layouter for SizedBoxLayouter {
    fn size_self(&mut self, _signal_values: &SignalValues, _ctx: LayouterSizeSelfCtx) -> Size {
        self.size
    }

    fn callback_begin_layout(&mut self) {}

    fn constrain_child(&mut self, ctx: LayouterConstrainChildrenCtx) -> Constraints {
        ctx.constraints
    }

    fn child_was_sized_compute_position(&mut self, _ctx: LayoutChildWasSizedCtx) -> Point {
        Point::ORIGIN
    }
}

fn main() -> Result<()> {
    let mut widget_tree = WidgetTree::new();
    let size_signal = widget_tree.create_signal(Size::new(100.0, 100.0));
    let root = widget_tree.add_node(
        Box::new(RowLayouter {
            child_sizes: vec![],
        }),
        None,
    );
    let child1 = widget_tree.add_node(
        Box::new(ReactiveSizedBoxLayouter { size: size_signal }),
        Some(Box::new(SimpleQuadDrawer {})),
    );
    let child2 = widget_tree.add_node(
        Box::new(ReactiveSizedBoxLayouter { size: size_signal }),
        Some(Box::new(SimpleQuadDrawer {})),
    );
    let child3 = widget_tree.add_node(
        Box::new(ReactiveSizedBoxLayouter { size: size_signal }),
        Some(Box::new(SimpleQuadDrawer {})),
    );
    widget_tree.t.borrow_mut().add_edge(root, child1, ());
    widget_tree.t.borrow_mut().add_edge(root, child2, ());
    widget_tree.t.borrow_mut().add_edge(child1, child3, ());

    let mut app = SimpleVelloApp {
        context: RenderContext::new(),
        renderers: vec![],
        state: RenderState::Suspended(None),
        scene: Scene::new(),
        widget_tree,
    };

    let event_loop = EventLoop::new()?;
    event_loop
        .run_app(&mut app)
        .expect("Couldn't run event loop");
    Ok(())
}

/// Helper function that creates a Winit window and returns it (wrapped in an Arc for sharing between threads)
fn create_winit_window(event_loop: &ActiveEventLoop) -> Arc<Window> {
    let attr = Window::default_attributes()
        .with_inner_size(LogicalSize::new(1044, 800))
        .with_resizable(true)
        .with_title("Vello Shapes");
    Arc::new(event_loop.create_window(attr).unwrap())
}

/// Helper function that creates a vello `Renderer` for a given `RenderContext` and `RenderSurface`
fn create_vello_renderer(render_cx: &RenderContext, surface: &RenderSurface<'_>) -> Renderer {
    Renderer::new(
        &render_cx.devices[surface.dev_id].device,
        RendererOptions {
            surface_format: Some(surface.format),
            use_cpu: false,
            antialiasing_support: vello::AaSupport::all(),
            num_init_threads: NonZeroUsize::new(1),
        },
    )
    .expect("Couldn't create renderer")
}

/// Add shapes to a vello scene. This does not actually render the shapes, but adds them
/// to the Scene data structure which represents a set of objects to draw.
fn add_shapes_to_scene(scene: &mut Scene) {
    // Draw an outlined rectangle
    let stroke = Stroke::new(6.0);
    let rect = RoundedRect::new(4.0, 4.0, 240.0, 240.0, 20.0);
    let rect_stroke_color = Color::new([0.9804, 0.702, 0.5294, 1.]);
    let rect_fill_color = Color::new([0.6, 0.5, 0.3, 1.]);
    scene.fill(
        vello::peniko::Fill::NonZero,
        Affine::IDENTITY,
        rect_fill_color,
        None,
        &rect,
    );
    scene.stroke(&stroke, Affine::IDENTITY, rect_stroke_color, None, &rect);

    //// Draw a filled circle
    //let circle = Circle::new((420.0, 200.0), 120.0);
    //let circle_fill_color = Color::new([0.9529, 0.5451, 0.6588, 1.]);
    //scene.fill(
    //    vello::peniko::Fill::NonZero,
    //    Affine::IDENTITY,
    //    circle_fill_color,
    //    None,
    //    &circle,
    //);
    //
    //// Draw a filled ellipse
    //let ellipse = Ellipse::new((250.0, 420.0), (100.0, 160.0), -90.0);
    //let ellipse_fill_color = Color::new([0.7961, 0.651, 0.9686, 1.]);
    //scene.fill(
    //    vello::peniko::Fill::NonZero,
    //    Affine::IDENTITY,
    //    ellipse_fill_color,
    //    None,
    //    &ellipse,
    //);
    //
    //// Draw a straight line
    //let line = Line::new((260.0, 20.0), (620.0, 100.0));
    //let line_stroke_color = Color::new([0.5373, 0.7059, 0.9804, 1.]);
    //scene.stroke(&stroke, Affine::IDENTITY, line_stroke_color, None, &line);
}
