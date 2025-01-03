use anyhow::Result;
use petgraph::graph::{DiGraph, NodeIndex};
use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::HashMap;
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

#[derive(Debug)]
pub struct ActiveRenderState<'s> {
    surface: RenderSurface<'s>,
    window: Arc<Window>,
}

enum RenderState<'s> {
    Active(ActiveRenderState<'s>),
    Suspended(Option<Arc<Window>>),
}

struct SimpleVelloApp<'s> {
    context: RenderContext,
    renderers: Vec<Option<Renderer>>,
    state: RenderState<'s>,
    scene: Scene,
    widget_tree: &'static WidgetTree,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Constraints {
    min: Size,
    max: Size,
}

impl std::hash::Hash for Constraints {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.min.width.to_bits().hash(state);
        self.min.height.to_bits().hash(state);
        self.max.width.to_bits().hash(state);
        self.max.height.to_bits().hash(state);
    }
}

impl std::cmp::Eq for Constraints {}

#[derive(Clone, Copy, Hash, Debug, PartialEq, Eq)]
struct LayouterConstrainChildrenCtx {
    child_n: usize,
    self_constraints: Constraints,
}

#[derive(Clone, Copy, Hash, Debug, PartialEq)]
struct LayoutChildWasSizedCtx {
    child_n: usize,
}

impl std::cmp::Eq for LayoutChildWasSizedCtx {}

struct LayouterSizeSelfCtx {
    constraints: Constraints,
}

trait Layouter {
    fn constraints_for_child(
        &self,
        tree: &'static WidgetTree,
        index: NodeIndex,
        ctx: LayouterConstrainChildrenCtx,
    ) -> Constraints;
    fn position_for_child(
        &self,
        tree: &'static WidgetTree,
        index: NodeIndex,
        ctx: LayoutChildWasSizedCtx,
    ) -> Point;
    fn size_for_self(
        &self,
        tree: &'static WidgetTree,
        index: NodeIndex,
        ctx: LayouterSizeSelfCtx,
    ) -> Size;
}

struct RowLayouter {}

impl Layouter for RowLayouter {
    fn constraints_for_child(
        &self,
        tree: &'static WidgetTree,
        index: NodeIndex,
        ctx: LayouterConstrainChildrenCtx,
    ) -> Constraints {
        if ctx.child_n == 0 {
            return ctx.self_constraints;
        }

        let prev_child_index = tree.get_cached_query_or_compute(NthChild {
            parent_index: index,
            child_n: ctx.child_n - 1,
        });

        let prev_child_size = tree.get_cached_query_or_compute(NodeSize {
            index: prev_child_index,
        });

        let prev_child_position = tree.get_cached_query_or_compute(NodePosition {
            index: prev_child_index,
        });

        let remaining_width =
            ctx.self_constraints.max.width - prev_child_position.x - prev_child_size.width;

        Constraints {
            min: ctx.self_constraints.min,
            max: Size {
                width: remaining_width,
                height: ctx.self_constraints.max.height,
            },
        }
    }

    fn position_for_child(
        &self,
        tree: &'static WidgetTree,
        index: NodeIndex,
        ctx: LayoutChildWasSizedCtx,
    ) -> Point {
        if ctx.child_n == 0 {
            return Point::ORIGIN;
        }

        let prev_child_index = tree.get_cached_query_or_compute(NthChild {
            parent_index: index,
            child_n: ctx.child_n - 1,
        });

        let prev_child_size = tree.get_cached_query_or_compute(NodeSize {
            index: prev_child_index,
        });

        let prev_child_position = tree.get_cached_query_or_compute(NodePosition {
            index: prev_child_index,
        });

        Point::new(
            prev_child_position.x + prev_child_size.width,
            prev_child_position.y,
        )
    }

    fn size_for_self(
        &self,
        tree: &'static WidgetTree,
        index: NodeIndex,
        ctx: LayouterSizeSelfCtx,
    ) -> Size {
        let child_indices = tree
            .tree
            .borrow()
            .neighbors_directed(index, petgraph::Direction::Outgoing)
            .collect::<Vec<_>>();
        let last_child_index = *child_indices.last().unwrap();

        let last_child_x = tree
            .get_cached_query_or_compute(NodePosition {
                index: last_child_index,
            })
            .x;

        let last_child_width = tree
            .get_cached_query_or_compute(NodeSize {
                index: last_child_index,
            })
            .width;

        return Size {
            width: last_child_x + last_child_width,
            height: ctx.constraints.max.height, // todo(chad): only need to be as tall as our tallest child
        };
    }
}

struct Padded {
    top: f64,
    bottom: f64,
    left: f64,
    right: f64,
}

impl Padded {
    fn uniform(p: f64) -> Self {
        return Padded {
            top: p,
            bottom: p,
            left: p,
            right: p,
        };
    }

    fn symmetric(vertical: f64, horizontal: f64) -> Self {
        return Padded {
            top: vertical,
            bottom: vertical,
            left: horizontal,
            right: horizontal,
        };
    }
}

impl Layouter for Padded {
    fn constraints_for_child(
        &self,
        tree: &'static WidgetTree,
        index: NodeIndex,
        ctx: LayouterConstrainChildrenCtx,
    ) -> Constraints {
        Constraints {
            min: ctx.self_constraints.min,
            max: Size {
                width: ctx.self_constraints.max.width - self.left - self.right,
                height: ctx.self_constraints.max.height - self.top - self.bottom,
            },
        }
    }

    fn position_for_child(
        &self,
        tree: &'static WidgetTree,
        index: NodeIndex,
        ctx: LayoutChildWasSizedCtx,
    ) -> Point {
        Point {
            x: self.left,
            y: self.top,
        }
    }

    fn size_for_self(
        &self,
        tree: &'static WidgetTree,
        index: NodeIndex,
        _ctx: LayouterSizeSelfCtx,
    ) -> Size {
        // todo(chad): compute largest child. For now, just assume one child and comput 0th child
        // OR, we can assert that this widget only has one child
        let first_child = tree.get_cached_query_or_compute(NthChild {
            parent_index: index,
            child_n: 0,
        });

        let first_child_size = tree.get_cached_query_or_compute(NodeSize { index: first_child });

        Size {
            width: first_child_size.width + self.left + self.right,
            height: first_child_size.height + self.top + self.bottom,
        }
    }
}

struct DrawerCtx<'a> {
    rect: Rect,
    scene: &'a mut Scene,
}

trait Drawer {
    fn draw(&self, ctx: DrawerCtx);
}

// struct Revision {
//     last_updated: usize,
//     valid_through: usize,
// }

struct WidgetTreeWeight {
    layouter: Box<dyn Layouter>,
    drawer: Option<Box<dyn Drawer>>,
}

trait QueryKey: Clone + std::hash::Hash + std::fmt::Debug + PartialEq + Eq {
    type Output: Clone + std::fmt::Debug;

    fn execute(&self, tree: &'static WidgetTree) -> Self::Output;
}

#[derive(Clone, Copy, Hash, Debug, PartialEq, Eq)]
struct NodePosition {
    index: NodeIndex,
}

impl QueryKey for NodePosition {
    type Output = Point;

    fn execute(&self, tree: &'static WidgetTree) -> Self::Output {
        if self.index == tree.root.borrow().unwrap() {
            return Point::ORIGIN;
        }

        let parent = tree
            .tree
            .borrow()
            .neighbors_directed(self.index, petgraph::Direction::Incoming)
            .next()
            .unwrap();

        // todo(chad): performance
        let child_n = tree
            .tree
            .borrow()
            .neighbors_directed(parent, petgraph::Direction::Outgoing)
            .position(|n| n == self.index)
            .unwrap();

        tree.tree
            .borrow()
            .node_weight(parent)
            .unwrap()
            .layouter
            .position_for_child(tree, parent, LayoutChildWasSizedCtx { child_n })
    }
}

#[derive(Clone, Copy, Hash, Debug, PartialEq, Eq)]
struct NodeConstraints {
    index: NodeIndex,
}

impl QueryKey for NodeConstraints {
    type Output = Constraints;

    fn execute(&self, tree: &'static WidgetTree) -> Self::Output {
        if self.index == tree.root.borrow().unwrap() {
            return Constraints {
                min: Size::ZERO,
                max: tree.size.borrow().clone(),
            };
        }

        let parent = tree
            .tree
            .borrow()
            .neighbors_directed(self.index, petgraph::Direction::Incoming)
            .next()
            .unwrap();

        let parent_constraints =
            tree.get_cached_query_or_compute(NodeConstraints { index: parent });

        // todo(chad): performance
        let child_n = tree
            .tree
            .borrow()
            .neighbors_directed(parent, petgraph::Direction::Outgoing)
            .position(|n| n == self.index)
            .unwrap();

        tree.tree
            .borrow()
            .node_weight(parent)
            .unwrap()
            .layouter
            .constraints_for_child(
                tree,
                parent,
                LayouterConstrainChildrenCtx {
                    child_n,
                    self_constraints: parent_constraints,
                },
            )
    }
}

#[derive(Clone, Copy, Hash, Debug, PartialEq, Eq)]
struct NodeSize {
    index: NodeIndex,
}

impl QueryKey for NodeSize {
    type Output = Size;

    fn execute(&self, tree: &'static WidgetTree) -> Self::Output {
        let constraints = tree.get_cached_query_or_compute(NodeConstraints { index: self.index });
        tree.tree
            .borrow()
            .node_weight(self.index)
            .unwrap()
            .layouter
            .size_for_self(tree, self.index, LayouterSizeSelfCtx { constraints })
    }
}

#[derive(Clone, Copy, Hash, Debug, PartialEq, Eq)]
struct NthChild {
    parent_index: NodeIndex,
    child_n: usize,
}

impl QueryKey for NthChild {
    type Output = NodeIndex;

    fn execute(&self, tree: &'static WidgetTree) -> Self::Output {
        let result = tree
            .tree
            .borrow()
            .neighbors_directed(self.parent_index, petgraph::Direction::Outgoing)
            .nth(self.child_n)
            .unwrap();

        result
    }
}

// struct CachedQueryOutput {
//     output: Box<dyn Any>,
//     revision: Revision,
// }

#[derive(Default)]
struct DebugContext {
    indent: usize,
}

struct WidgetTree {
    size: RefCell<Size>,
    tree: RefCell<DiGraph<WidgetTreeWeight, ()>>,
    root: RefCell<Option<NodeIndex>>,
    // revision: usize,
    // where here the Box<dyn Any> is a hashmap from input type to CachedQueryOutput for that input type
    query_cache: RefCell<HashMap<TypeId, Box<dyn Any>>>,
    debug_context: RefCell<DebugContext>,
}

impl WidgetTree {
    pub fn new() -> Self {
        Self {
            size: RefCell::new(Size::ZERO),
            tree: RefCell::new(DiGraph::<WidgetTreeWeight, ()>::new()),
            root: RefCell::new(None),
            // revision: 0,
            query_cache: RefCell::new(HashMap::new()),
            debug_context: RefCell::new(DebugContext::default()),
        }
    }

    pub fn add_node(
        &'static self,
        layouter: Box<dyn Layouter>,
        drawer: Option<Box<dyn Drawer>>,
    ) -> NodeIndex {
        let idx = self
            .tree
            .borrow_mut()
            .add_node(WidgetTreeWeight { layouter, drawer });

        if self.root.borrow().is_none() {
            *self.root.borrow_mut() = Some(idx)
        }

        idx
    }

    pub fn add_child(
        &'static self,
        parent_index: impl IntoNodeIndex,
        child_index: impl IntoNodeIndex,
    ) -> NodeIndex {
        let parent_index = parent_index.into(self);
        let child_index = child_index.into(self);

        self.tree
            .borrow_mut()
            .add_edge(parent_index, child_index, ());

        child_index
    }

    pub fn add_child_index(
        &'static self,
        parent_index: impl IntoNodeIndex,
        child_index: impl IntoNodeIndex,
    ) -> NodeIndex {
        let child_index = child_index.into(self);
        let parent_index = parent_index.into(self);

        self.tree
            .borrow_mut()
            .add_edge(parent_index, child_index, ());

        parent_index
    }

    pub fn draw_index(&'static self, index: NodeIndex, scene: &mut Scene, offset_pos: Point) {
        let position = {
            let weight = self.tree.borrow();
            let weight = weight.node_weight(index).unwrap();

            let mut position: Point = self.get_cached_query_or_compute(NodePosition { index });
            position.x += offset_pos.x;
            position.y += offset_pos.y;

            let size: Size = self.get_cached_query_or_compute(NodeSize { index });

            weight.drawer.as_ref().map(|d| {
                d.draw(DrawerCtx {
                    scene,
                    rect: Rect::from_origin_size(position, size),
                });
            });

            position
        };

        // todo(chad): performance
        let neighbors = self
            .tree
            .borrow()
            .neighbors_directed(index, petgraph::Direction::Outgoing)
            .collect::<Vec<_>>();
        for child in neighbors {
            let offset_pos = Point::new(offset_pos.x + position.x, offset_pos.y + position.y);
            self.draw_index(child, scene, offset_pos);
        }
    }

    pub fn draw(&'static self, scene: &mut Scene) {
        let Some(root) = *self.root.borrow() else {
            return;
        };
        self.draw_index(root, scene, Point::ORIGIN);
    }

    pub fn get_cached_query_or_compute<I: QueryKey + 'static>(
        &'static self,
        input: I,
    ) -> <I as QueryKey>::Output {
        println!(
            "{}Computing {:?}",
            "  ".repeat(self.debug_context.borrow().indent),
            input
        );

        if let Some(cached_output) = self.get_cached_query(&input) {
            println!(
                "{}Result {:?}",
                "  ".repeat(self.debug_context.borrow().indent),
                &cached_output
            );
            return cached_output;
        }

        self.debug_context.borrow_mut().indent += 1;

        let output = input.execute(self);
        self.cache_query(input, output.clone());

        self.debug_context.borrow_mut().indent -= 1;
        println!(
            "{}Result {:?}",
            "  ".repeat(self.debug_context.borrow().indent),
            output.clone()
        );

        output.clone()
    }

    pub fn cache_query<I: QueryKey + 'static, O: 'static>(&'static self, input: I, output: O) {
        let mut cache = self.query_cache.borrow_mut();
        let cache = cache
            .entry(TypeId::of::<I>())
            .or_insert_with(|| Box::new(HashMap::<I, O>::new()))
            .downcast_mut::<HashMap<I, O>>()
            .unwrap();
        cache.insert(input, output);
    }

    pub fn get_cached_query<I: QueryKey + 'static, O: Clone + 'static>(
        &'static self,
        input: &I,
    ) -> Option<O> {
        let qc = self.query_cache.borrow();
        let type_id = TypeId::of::<I>();
        let qc = qc.get(&type_id)?;
        let qc = qc.downcast_ref::<HashMap<I, O>>()?;
        qc.get(input).cloned()
    }
}

trait IntoNodeIndex {
    fn into(self, widget_tree: &'static WidgetTree) -> NodeIndex;
}

impl IntoNodeIndex for NodeIndex {
    fn into(self, _widget_tree: &'static WidgetTree) -> NodeIndex {
        self
    }
}

impl<L: Layouter + 'static> IntoNodeIndex for L {
    fn into(self, widget_tree: &'static WidgetTree) -> NodeIndex {
        widget_tree.add_node(Box::new(self), None)
    }
}

impl<L: Layouter + 'static, D: Drawer + 'static> IntoNodeIndex for (L, D) {
    fn into(self, widget_tree: &'static WidgetTree) -> NodeIndex {
        widget_tree.add_node(Box::new(self.0), Some(Box::new(self.1)))
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

        *self.widget_tree.size.borrow_mut() = Size::new(size.width as f64, size.height as f64);

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
                *self.widget_tree.size.borrow_mut() =
                    Size::new(size.width as f64, size.height as f64);
            }

            // This is where all the rendering happens
            WindowEvent::RedrawRequested => {
                // Empty the scene of objects to draw. You could create a new Scene each time, but in this case
                // the same Scene is reused so that the underlying memory allocation can also be reused.
                self.scene.reset();

                // Get the RenderSurface (surface + config)
                let surface = &render_state.surface;

                self.widget_tree.draw(&mut self.scene);
                println!("===================");

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

struct SimpleQuadDrawer {
    color: [f32; 3],
}

impl Drawer for SimpleQuadDrawer {
    fn draw(&self, ctx: DrawerCtx) {
        let [r, g, b] = self.color;
        let [dr, dg, db] = [(r + 1.0) / 2.0, (g + 1.0) / 2.0, (b + 1.0) / 2.0];

        // Draw an outlined rectangle
        let stroke = Stroke::new(6.0);
        let rect = RoundedRect::new(ctx.rect.x0, ctx.rect.y0, ctx.rect.x1, ctx.rect.y1, 20.0);
        let rect_stroke_color = Color::new([dr, dg, db, 1.]);
        let rect_fill_color = Color::new([r, g, b, 1.]);
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

struct SizedBoxLayouter {
    size: Size,
}

impl Layouter for SizedBoxLayouter {
    fn size_for_self(
        &self,
        _tree: &'static WidgetTree,
        _index: NodeIndex,
        _ctx: LayouterSizeSelfCtx,
    ) -> Size {
        self.size
    }

    fn constraints_for_child(
        &self,
        _tree: &'static WidgetTree,
        _index: NodeIndex,
        ctx: LayouterConstrainChildrenCtx,
    ) -> Constraints {
        ctx.self_constraints
    }

    fn position_for_child(
        &self,
        _tree: &'static WidgetTree,
        _index: NodeIndex,
        _ctx: LayoutChildWasSizedCtx,
    ) -> Point {
        Point::ORIGIN
    }
}

// fn padded(widget_tree: &'static WidgetTree, child_index: NodeIndex, p: Padded) -> NodeIndex {
//     let parent_index = widget_tree.add_node(Box::new(p), None);
//     widget_tree
//         .tree
//         .borrow_mut()
//         .add_edge(parent_index, child_index, ());
//     parent_index
// }

fn padded(
    widget_tree: &'static WidgetTree,
    child: impl IntoNodeIndex,
    p: Padded,
) -> impl IntoNodeIndex {
    widget_tree.add_child_index(p, child)
}

fn main() -> Result<()> {
    let widget_tree = WidgetTree::new();
    let widget_tree = Box::leak(Box::new(widget_tree));

    let root = widget_tree.add_node(Box::new(RowLayouter {}), None);
    widget_tree.add_child(
        root,
        padded(
            widget_tree,
            (
                SizedBoxLayouter {
                    size: Size::new(100.0, 100.0),
                },
                SimpleQuadDrawer {
                    color: [0.6, 0.5, 0.4],
                },
            ),
            Padded::uniform(15.0),
        ),
    );
    widget_tree.add_child(
        root,
        (
            SizedBoxLayouter {
                size: Size::new(100.0, 100.0),
            },
            SimpleQuadDrawer {
                color: [0.6, 0.5, 0.4],
            },
        ),
    );
    widget_tree.add_child(
        root,
        (
            SizedBoxLayouter {
                size: Size::new(100.0, 100.0),
            },
            SimpleQuadDrawer {
                color: [0.6, 0.5, 0.4],
            },
        ),
    );

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
