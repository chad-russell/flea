use anyhow::Result;
use petgraph::graph::{DiGraph, NodeIndex};
use std::any::Any;
use std::borrow::BorrowMut;
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
use winit::event::{ElementState, WindowEvent};
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

impl Constraints {
    fn clamp_size(&self, s: Size) -> Size {
        Size {
            width: s.width.max(self.min.width).min(self.max.width),
            height: s.height.max(self.min.height).min(self.max.height),
        }
    }
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

        let prev_child_index = tree.query_nth_child(NthChild {
            parent_index: index,
            child_n: ctx.child_n - 1,
        });

        let prev_child_size = tree.query_node_size(prev_child_index);
        let prev_child_position = tree.query_node_position(prev_child_index);

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

        let prev_child_index = tree.query_nth_child(NthChild {
            parent_index: index,
            child_n: ctx.child_n - 1,
        });

        let prev_child_size = tree.query_node_size(prev_child_index);
        let prev_child_position = tree.query_node_position(prev_child_index);

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

        let last_child_x = tree.query_node_position(last_child_index).x;

        let last_child_width = tree.query_node_size(last_child_index).width;

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
        _tree: &'static WidgetTree,
        _index: NodeIndex,
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
        _tree: &'static WidgetTree,
        _index: NodeIndex,
        _ctx: LayoutChildWasSizedCtx,
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
        let first_child = tree.query_nth_child(NthChild {
            parent_index: index,
            child_n: 0,
        });

        let first_child_size = tree.query_node_size(first_child);

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

struct WidgetTreeWeight {
    layouter: Box<dyn Layouter>,
    drawer: Option<Box<dyn Drawer>>,
}

#[derive(Clone, Copy, Hash, Debug, PartialEq, Eq)]
enum QueryDependency {
    NodePosition(NodeIndex),
    NodeConstraints(NodeIndex),
    NodeSize(NodeIndex),
    NthChild(NthChild),
    Signal(SignalId),
}

trait QueryKey: Clone + std::hash::Hash + std::fmt::Debug + PartialEq + Eq {
    type Output: Clone + std::fmt::Debug;

    fn execute(&self, tree: &'static WidgetTree) -> Self::Output;
}

#[derive(Clone, Copy, Hash, Debug, PartialEq, Eq)]
struct SignalId(usize);

#[derive(Clone, Copy, Hash, Debug, PartialEq, Eq)]
struct Signal<T> {
    id: SignalId,
    phantom: std::marker::PhantomData<T>,
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

        let parent_constraints = tree.query_node_constraints(parent);

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
        let constraints = tree.query_node_constraints(self.index);
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

#[derive(Clone, Copy, Hash, Debug, PartialEq, Eq)]
struct RevisionId(usize);

#[derive(Clone, Copy)]
struct Revision {
    last_changed: usize,
    valid_through: usize,
}

#[derive(Clone)]
struct CachedQueryOutput<T: Clone> {
    value: T,
    revision: Revision,
}

struct WidgetTree {
    size: RefCell<Size>,
    tree: RefCell<DiGraph<WidgetTreeWeight, ()>>,
    root: RefCell<Option<NodeIndex>>,

    revision: RefCell<usize>,

    signals: RefCell<HashMap<SignalId, Box<dyn Any>>>,
    query_stack: RefCell<Vec<QueryDependency>>,
    dependency_tree: RefCell<DiGraph<QueryDependency, ()>>,
    dependency_node_map: RefCell<HashMap<QueryDependency, NodeIndex>>,

    // Query caches
    node_position_query_cache: RefCell<HashMap<NodeIndex, CachedQueryOutput<Point>>>,
    node_size_query_cache: RefCell<HashMap<NodeIndex, CachedQueryOutput<Size>>>,
    node_constraints_query_cache: RefCell<HashMap<NodeIndex, CachedQueryOutput<Constraints>>>,
    nth_child_query_cache: RefCell<HashMap<NthChild, CachedQueryOutput<NodeIndex>>>,

    // Debug
    cache_ratio: RefCell<(u64, u64)>,
}

impl WidgetTree {
    pub fn new() -> Self {
        Self {
            size: RefCell::new(Size::ZERO),
            tree: RefCell::new(DiGraph::new()),
            root: RefCell::new(None),
            revision: RefCell::new(0),
            signals: RefCell::new(HashMap::new()),
            query_stack: RefCell::new(Vec::new()),
            dependency_tree: RefCell::new(DiGraph::new()),
            dependency_node_map: RefCell::new(HashMap::new()),
            node_position_query_cache: RefCell::new(HashMap::new()),
            node_size_query_cache: RefCell::new(HashMap::new()),
            node_constraints_query_cache: RefCell::new(HashMap::new()),
            nth_child_query_cache: RefCell::new(HashMap::new()),
            cache_ratio: RefCell::new((0, 1)),
        }
    }

    fn track_dependency(&'static self, dep: QueryDependency) {
        let Some(q) = self.query_stack.borrow().last().cloned() else {
            return;
        };

        let dep_node_index = self
            .dependency_node_map
            .borrow_mut()
            .entry(dep)
            .or_insert_with(|| self.dependency_tree.borrow_mut().add_node(dep))
            .clone();
        let q_node_index = self
            .dependency_node_map
            .borrow_mut()
            .entry(q)
            .or_insert_with(|| self.dependency_tree.borrow_mut().add_node(q))
            .clone();
        self.dependency_tree
            .borrow_mut()
            .add_edge(q_node_index, dep_node_index, ());
    }

    pub fn create_signal<T: Clone + 'static>(&'static self, value: T) -> Signal<T> {
        let mut signals = self.signals.borrow_mut();
        let id = SignalId(signals.len());
        signals.insert(id, Box::new(value));
        Signal {
            id,
            phantom: std::marker::PhantomData,
        }
    }

    pub fn get_signal<T: Clone + 'static>(&'static self, signal: Signal<T>) -> T {
        let signals = self.signals.borrow();
        let sig = signals
            .get(&signal.id)
            .unwrap()
            .downcast_ref::<T>()
            .unwrap()
            .clone();

        self.track_dependency(QueryDependency::Signal(signal.id));

        sig
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
    ) -> (NodeIndex, NodeIndex) {
        let parent_index = parent_index.into(self);
        let child_index = child_index.into(self);

        self.tree
            .borrow_mut()
            .add_edge(parent_index, child_index, ());

        (parent_index, child_index)
    }

    pub fn add_child_return_parent(
        &'static self,
        parent_index: impl IntoNodeIndex,
        child_index: impl IntoNodeIndex,
    ) -> NodeIndex {
        self.add_child(parent_index, child_index).0
    }

    pub fn add_child_return_child(
        &'static self,
        parent_index: impl IntoNodeIndex,
        child_index: impl IntoNodeIndex,
    ) -> NodeIndex {
        self.add_child(parent_index, child_index).1
    }

    pub fn draw_index(&'static self, index: NodeIndex, scene: &mut Scene, offset_pos: Point) {
        let position = {
            let weight = self.tree.borrow();
            let weight = weight.node_weight(index).unwrap();

            let mut position: Point = self.query_node_position(index);
            position.x += offset_pos.x;
            position.y += offset_pos.y;

            let size: Size = self.query_node_size(index);

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

    fn current_revision(&'static self) -> Revision {
        Revision {
            last_changed: *self.revision.borrow(),
            valid_through: *self.revision.borrow(),
        }
    }

    pub fn query_nth_child(&'static self, q: NthChild) -> NodeIndex {
        self.cache_ratio.borrow_mut().1 += 1;

        if let Some(cached_output) = self.nth_child_query_cache.borrow().get(&q) {
            if cached_output.revision.valid_through >= *self.revision.borrow() {
                self.cache_ratio.borrow_mut().0 += 1;
                return cached_output.value;
            }
        }

        //println!("Recomputing {:?}", QueryDependency::NthChild(q));

        self.track_dependency(QueryDependency::NthChild(q));
        self.query_stack
            .borrow_mut()
            .push(QueryDependency::NthChild(q));
        let output = q.execute(self);
        self.query_stack.borrow_mut().pop().unwrap();

        self.nth_child_query_cache.borrow_mut().insert(
            q,
            CachedQueryOutput {
                revision: self.current_revision(),
                value: output,
            },
        );
        output
    }

    pub fn query_node_constraints(&'static self, q: NodeIndex) -> Constraints {
        self.cache_ratio.borrow_mut().1 += 1;

        if let Some(cached_output) = self.node_constraints_query_cache.borrow().get(&q) {
            if cached_output.revision.valid_through >= *self.revision.borrow() {
                self.cache_ratio.borrow_mut().0 += 1;
                return cached_output.value;
            }
        }

        //println!("Recomputing {:?}", QueryDependency::NodeConstraints(q));

        self.track_dependency(QueryDependency::NodeConstraints(q));
        self.query_stack
            .borrow_mut()
            .push(QueryDependency::NodeConstraints(q));
        let output = NodeConstraints { index: q }.execute(self);
        self.query_stack.borrow_mut().pop().unwrap();

        self.node_constraints_query_cache.borrow_mut().insert(
            q,
            CachedQueryOutput {
                revision: self.current_revision(),
                value: output,
            },
        );
        output
    }

    pub fn query_node_size(&'static self, q: NodeIndex) -> Size {
        self.cache_ratio.borrow_mut().1 += 1;

        if let Some(cached_output) = self.node_size_query_cache.borrow().get(&q) {
            if cached_output.revision.valid_through >= *self.revision.borrow() {
                self.cache_ratio.borrow_mut().0 += 1;
                return cached_output.value;
            }
        }

        //println!("Recomputing {:?}", QueryDependency::NodeSize(q));

        self.track_dependency(QueryDependency::NodeSize(q));
        self.query_stack
            .borrow_mut()
            .push(QueryDependency::NodeSize(q));
        let output = NodeSize { index: q }.execute(self);
        self.query_stack.borrow_mut().pop().unwrap();

        self.node_size_query_cache.borrow_mut().insert(
            q,
            CachedQueryOutput {
                revision: self.current_revision(),
                value: output,
            },
        );
        output
    }

    pub fn query_node_position(&'static self, q: NodeIndex) -> Point {
        self.cache_ratio.borrow_mut().1 += 1;

        if let Some(cached_output) = self.node_position_query_cache.borrow().get(&q) {
            if cached_output.revision.valid_through >= *self.revision.borrow() {
                self.cache_ratio.borrow_mut().0 += 1;
                return cached_output.value;
            }
        }

        //println!("Recomputing {:?}", QueryDependency::NodePosition(q));

        self.track_dependency(QueryDependency::NodePosition(q));
        self.query_stack
            .borrow_mut()
            .push(QueryDependency::NodePosition(q));
        let output = NodePosition { index: q }.execute(self);
        self.query_stack.borrow_mut().pop().unwrap();

        self.node_position_query_cache.borrow_mut().insert(
            q,
            CachedQueryOutput {
                revision: self.current_revision(),
                value: output,
            },
        );
        output
    }

    pub fn invalidate(&'static self, q: QueryDependency) {
        let q_index = self.dependency_node_map.borrow().get(&q).unwrap().clone();

        let q_parents = self
            .dependency_tree
            .borrow()
            .neighbors_directed(q_index, petgraph::Direction::Incoming)
            .collect::<Vec<_>>();

        let mut to_invalidate = Vec::new();

        for p in q_parents {
            let p_dep = self
                .dependency_tree
                .borrow()
                .node_weight(p)
                .unwrap()
                .clone();

            match p_dep {
                QueryDependency::NodePosition(node_index) => {
                    let old_value = self
                        .node_position_query_cache
                        .borrow_mut()
                        .get(&node_index)
                        .cloned()
                        .unwrap();

                    let new_value = self.query_node_position(node_index);

                    if new_value == old_value.value {
                        self.node_position_query_cache.borrow_mut().insert(
                            node_index,
                            CachedQueryOutput {
                                value: new_value,
                                revision: Revision {
                                    last_changed: old_value.revision.last_changed,
                                    valid_through: *self.revision.borrow(),
                                },
                            },
                        );
                    } else {
                        to_invalidate.push(p_dep);
                    }
                }
                QueryDependency::NodeConstraints(node_index) => {
                    let old_value = self
                        .node_constraints_query_cache
                        .borrow_mut()
                        .get(&node_index)
                        .cloned()
                        .unwrap();

                    let new_value = self.query_node_constraints(node_index);

                    if new_value == old_value.value {
                        self.node_constraints_query_cache.borrow_mut().insert(
                            node_index,
                            CachedQueryOutput {
                                value: new_value,
                                revision: Revision {
                                    last_changed: old_value.revision.last_changed,
                                    valid_through: *self.revision.borrow(),
                                },
                            },
                        );
                    } else {
                        to_invalidate.push(p_dep);
                    }
                }
                QueryDependency::NodeSize(node_index) => {
                    let old_value = self
                        .node_size_query_cache
                        .borrow_mut()
                        .get(&node_index)
                        .cloned()
                        .unwrap();

                    let new_value = self.query_node_size(node_index);

                    if new_value == old_value.value {
                        self.node_size_query_cache.borrow_mut().insert(
                            node_index,
                            CachedQueryOutput {
                                value: new_value,
                                revision: Revision {
                                    last_changed: old_value.revision.last_changed,
                                    valid_through: *self.revision.borrow(),
                                },
                            },
                        );
                    } else {
                        to_invalidate.push(p_dep);
                    }
                }
                QueryDependency::NthChild(nth_child) => {
                    let old_value = self
                        .nth_child_query_cache
                        .borrow_mut()
                        .get(&nth_child)
                        .cloned()
                        .unwrap();

                    let new_value = self.query_nth_child(nth_child);

                    if new_value == old_value.value {
                        self.nth_child_query_cache.borrow_mut().insert(
                            nth_child,
                            CachedQueryOutput {
                                value: new_value,
                                revision: Revision {
                                    last_changed: old_value.revision.last_changed,
                                    valid_through: *self.revision.borrow(),
                                },
                            },
                        );
                    } else {
                        to_invalidate.push(p_dep);
                    }
                }
                QueryDependency::Signal(_) => {
                    panic!("A signal should never depend on another thing");
                }
            };
        }

        for dep in to_invalidate {
            self.invalidate(dep);
        }
    }

    pub fn reset(&'static self) {
        println!("******* Resetting!!");

        println!("==========================");
        println!("{:?}", self.dependency_tree.borrow());
        println!("==========================");

        *self.revision.borrow_mut() = 0;
        *self.cache_ratio.borrow_mut() = (0, 1);

        self.dependency_node_map.borrow_mut().clear();
        self.dependency_tree.borrow_mut().clear();

        self.node_position_query_cache.borrow_mut().clear();
        self.node_size_query_cache.borrow_mut().clear();
        self.node_constraints_query_cache.borrow_mut().clear();
        self.nth_child_query_cache.borrow_mut().clear();
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
        let render_state = match &mut self.state {
            RenderState::Active(state) if state.window.id() == window_id => state,
            _ => return,
        };

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(size) => {
                self.context
                    .resize_surface(&mut render_state.surface, size.width, size.height);
                *self.widget_tree.size.borrow_mut() =
                    Size::new(size.width as f64, size.height as f64);
            }

            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                ..
            } => {
                *self.widget_tree.revision.borrow_mut() += 1;

                self.widget_tree
                    .signals
                    .borrow_mut()
                    .get_mut(&SignalId(0))
                    .unwrap()
                    .downcast_mut::<Size>()
                    .unwrap()
                    .width += 10.0;
                self.widget_tree
                    .invalidate(QueryDependency::Signal(SignalId(0)));

                let RenderState::Active(state) = &mut self.state else {
                    return;
                };
                state.window.borrow_mut().request_redraw();
                //println!("========");
            }

            WindowEvent::RedrawRequested => {
                *self.widget_tree.revision.borrow_mut() += 1;
                //if *self.widget_tree.revision.borrow() > 100 {
                //    self.widget_tree.reset();
                //}

                 println!(
                     "Cache ratio: {:?}",
                     self.widget_tree.cache_ratio.borrow().0 as f64
                         / self.widget_tree.cache_ratio.borrow().1 as f64
                 );

                self.scene.reset();

                let surface = &render_state.surface;

                self.widget_tree.draw(&mut self.scene);

                let width = surface.config.width;
                let height = surface.config.height;

                let device_handle = &self.context.devices[surface.dev_id];

                let surface_texture = surface
                    .surface
                    .get_current_texture()
                    .expect("failed to get surface texture");

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

struct CenteredLayouter { }

impl Layouter for CenteredLayouter {
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
        tree: &'static WidgetTree,
        index: NodeIndex,
        ctx: LayoutChildWasSizedCtx,
    ) -> Point {
        let self_size = tree.query_node_size(index);
        let child_size = tree.query_node_size(tree.query_nth_child(NthChild {
            parent_index: index,
            child_n: ctx.child_n,
        }));
        Point::new(
            (self_size.width - child_size.width) / 2.0,
            (self_size.height - child_size.height) / 2.0,
        )
    }

    fn size_for_self(
        &self,
        tree: &'static WidgetTree,
        index: NodeIndex,
        ctx: LayouterSizeSelfCtx,
    ) -> Size {
        // For a centered layouter, the self size could be the size of its child
        // or it could be determined differently depending on context or other constraints.
        // Here, we use the maximum constraints as a default.
        let child_indices = tree
            .tree
            .borrow()
            .neighbors_directed(index, petgraph::Direction::Outgoing)
            .collect::<Vec<_>>();
        if child_indices.is_empty() {
            return ctx.constraints.max; //Handle case with no children
        }

        let child_index = child_indices[0];
        let child_size = tree.query_node_size(child_index);
        ctx.constraints.clamp_size(child_size)
    }
}

struct DynamicallySizedBoxLayouter {
    size: Signal<Size>,
}

impl Layouter for DynamicallySizedBoxLayouter {
    fn size_for_self(
        &self,
        tree: &'static WidgetTree,
        _index: NodeIndex,
        _ctx: LayouterSizeSelfCtx,
    ) -> Size {
        tree.get_signal(self.size)
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

// todo(chad):
// # GENERAL
// - Implement cache red/green algorithm
// - Interactivity (keyboard/mouse events)
// - Text widget
// - Builder widgets, regenerate subtree on change
// - Animation
//
// # LAYOUTERS
// - Align
// - AspectRatio
// - Center
// - Expanded
// - FractionallySized
// - Transform
// - Flow
// - Grid
// - List
// - Stack

fn main() -> Result<()> {
    let widget_tree = WidgetTree::new();
    let widget_tree = Box::leak(Box::new(widget_tree));

    let size = Size {
        width: 100.0,
        height: 100.0,
    };
    let dyn_size = widget_tree.create_signal(size);

    let root = widget_tree.add_node(Box::new(RowLayouter {}), None);
    for _ in 0..3 {
        widget_tree.add_child(
            root,
            widget_tree.add_child_return_parent(
                DynamicallySizedBoxLayouter { size: dyn_size },
                widget_tree.add_child_return_parent(
                    CenteredLayouter{}, 
                    (
                        SizedBoxLayouter { size },
                        SimpleQuadDrawer {
                            color: [0.6, 0.5, 0.4],
                        },
                    ),
                ),
            ),
        );
    }

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
