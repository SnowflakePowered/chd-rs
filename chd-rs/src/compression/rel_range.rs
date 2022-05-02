use std::ops::Range;

pub struct RelativeRange<Idx> {
    start: Idx
}

impl RelativeRange<usize> {
    pub const fn from(start: usize) -> RelativeRange<usize> {
        RelativeRange { start }
    }

    pub const fn to(self, len: usize) -> Range<usize> {
        self.start..(self.start + len)
    }
}
