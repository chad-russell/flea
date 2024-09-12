use std::{cell::RefCell, rc::Rc};

use crate::{geometry::{Point, Size, Rect}, layout::LayoutConstraint};

pub type NodeId = usize;

pub trait Layout {
    fn prepare_layout(&mut self, _child_count: usize) {}
    fn constrain_child(&mut self, child_index: usize, constraint: LayoutConstraint) -> LayoutConstraint;
    fn on_child_sized(&mut self, _child_index: usize, _size: Size) {}
    fn position_child(&mut self, child_index: usize, child_size: Size) -> Point;
    fn compute_self_size(&mut self, constraint: LayoutConstraint) -> Size;
}

pub struct Tree {
    pub children: Vec<Vec<NodeId>>,
    pub layouts: Vec<Rc<RefCell<dyn Layout>>>,
    pub rects: Vec<Rect>,
}

impl Tree {
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            layouts: Vec::new(),
            rects: Vec::new(),
        }
    }

    pub fn push(&mut self, layout: impl Layout + 'static) -> NodeId {
        let id = self.layouts.len();

        self.layouts.push(Rc::new(RefCell::new(layout)));
        self.children.push(Vec::new());
        self.rects.push(Rect {
            pos: Point { x: 0, y: 0 },
            size: Size {
                width: 0,
                height: 0,
            },
        });

        id
    }

    pub fn push_with_child(&mut self, parent: impl Layout + 'static, child: impl Layout + 'static) {
        let parent = self.push(parent);
        let child = self.push(child);
        self.push_child(parent, child);
    }

    pub fn push_child(&mut self, parent: NodeId, child: NodeId) {
        self.children[parent].push(child);
    }


    pub fn layout(&mut self, size: Size) {
        let constraint = LayoutConstraint {
            min_width: size.width,
            min_height: size.height,
            max_width: size.width,
            max_height: size.height,
        };

        self.layout_id(0, constraint);
    }

    pub fn layout_id(&mut self, id: usize, constraint: LayoutConstraint) {
        let layouter = self.layouts[id].clone();
        let mut layouter = layouter.borrow_mut();

        layouter.prepare_layout(self.children[id].len());

        for (child_index, &child_id) in self.children[id].clone().iter().enumerate() {
            let child_constraint = layouter.constrain_child(child_index, constraint);
            self.layout_id(child_id, child_constraint);
            layouter.on_child_sized(child_index, self.rects[child_id].size);
        }

        for (child_index, &child_id) in self.children[id].clone().iter().enumerate() {
            let child_offset = layouter.position_child(child_index, self.rects[child_id].size);
            let base_pos = self.rects[id].pos;

            self.rects[child_id].pos = Point {
                x: base_pos.x + child_offset.x,
                y: base_pos.y + child_offset.y,
            }
        }

        self.rects[id].size = layouter.compute_self_size(constraint);
    }
}

pub struct SizedBox {
    pub size: Size,
}

impl SizedBox {
    pub fn new(size: Size) -> Self {
        Self {
            size,
        }
    }
}

impl Layout for SizedBox {
    fn constrain_child(&mut self, _child_index: usize, constraint: LayoutConstraint) -> LayoutConstraint {
        constraint
    }
    
    fn position_child(&mut self, _child_index: usize, _child_size: Size) -> Point {
        Point { x: 0, y: 0 }
    }
    
    fn compute_self_size(&mut self, _constraint: LayoutConstraint) -> Size {
        self.size
    }
}

pub struct Padded {
    pub left: u32,
    pub right: u32,
    pub top: u32,
    pub bottom: u32,

    child_size: Size,
}

impl Padded {
    pub fn new(left: u32, right: u32, top: u32, bottom: u32) -> Self {
        Self { left, right, top, bottom, child_size: Size { width: 0, height: 0 } }
    }
}

impl Layout for Padded {
    fn prepare_layout(&mut self, child_count: usize) {
        assert!(child_count == 1);
        self.child_size = Size { width: 0, height: 0 };
    }

    fn constrain_child(&mut self, _child_index: usize, constraint: LayoutConstraint) -> LayoutConstraint {
        LayoutConstraint {
            min_width: constraint.min_width + self.left + self.right,
            min_height: constraint.min_height + self.top + self.bottom,
            max_width: constraint.max_width - self.left - self.right,
            max_height: constraint.max_height - self.top - self.bottom,
        }
    }

    fn on_child_sized(&mut self, _child_index: usize, size: Size) {
        self.child_size = size;
    }
    
    fn position_child(&mut self, _child_index: usize, _child_size: Size) -> Point {
        Point {
            x: self.left,
            y: self.bottom,
        }
    }
    
    fn compute_self_size(&mut self, _constraint: LayoutConstraint) -> Size {
        Size {
            width: self.child_size.width + self.left + self.right,
            height: self.child_size.height + self.top + self.bottom,
        }
    }
}