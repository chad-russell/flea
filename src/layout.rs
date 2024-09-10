use crate::geometry::{Point, Size};

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
pub struct RowLayouter {
    pub max_height: u32,
    pub total_width: u32,
    pub child_x: u32,
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

pub struct SizedBoxLayouter {
    pub size: Size,
}

impl SizedBoxLayouter {
    pub fn new(size: Size) -> Self {
        Self {
            size,
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

pub struct PaddedLayouter {
    pub left: u32,
    pub right: u32,
    pub top: u32,
    pub bottom: u32,

    child_size: Size,
}

impl PaddedLayouter {
    pub fn new(left: u32, right: u32, top: u32, bottom: u32) -> Self {
        Self { left, right, top, bottom, child_size: Size { width: 0, height: 0 } }
    }
}

impl Layouter for PaddedLayouter {
    fn prepare(&mut self) {
        self.child_size = Size { width: 0, height: 0 };
    }

    fn constrain_child(&mut self, constraint: LayoutConstraint) -> LayoutConstraint {
        LayoutConstraint {
            min_width: constraint.min_width + self.left + self.right,
            min_height: constraint.min_height + self.top + self.bottom,
            max_width: constraint.max_width - self.left - self.right,
            max_height: constraint.max_height - self.top - self.bottom,
        }
    }

    fn child_sized(&mut self, size: Size) {
        self.child_size = size;
    }

    fn position_child(&mut self, _size: Size) -> Point {
        Point {
            x: self.left,
            y: self.bottom,
        }
    }

    fn compute_size(&mut self, _constraint: LayoutConstraint) -> Size {
        Size {
            width: self.child_size.width + self.left + self.right,
            height: self.child_size.height + self.top + self.bottom,
        }
    }
}