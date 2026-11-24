//! What a lending keeps of the rows it takes: each node's place in the
//! walk ([`Span`]), and the lists of rows, kept in place while they have
//! at most one ([`Rows`]), so that a lending along one route does not
//! allocate.
use alloc::vec::Vec;
use core::{
    ops::{Deref, DerefMut, Range},
    option, slice,
};

/// The place of a node in the walk of one lending: its number, and the
/// number after the last node below it.
///
/// The nodes of one kind are never below one another, so the rows of one
/// kind, in the order of the walk, are in the order of both numbers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    /// The node's number.
    pub start: usize,
    /// The number after the last node below it.
    pub end: usize,
}

/// A list kept in place while it has at most one item.
pub enum Rows<T> {
    /// No item, or one.
    One(Option<T>),
    /// More than one.
    Many(Vec<T>),
}
impl<T> Default for Rows<T> {
    fn default() -> Self {
        Self::One(None)
    }
}
impl<T> Rows<T> {
    /// Add `item` at the end.
    pub fn push(&mut self, item: T) {
        match self {
            Self::One(slot) => match slot.take() {
                None => *slot = Some(item),
                Some(first) => {
                    let mut v = Vec::with_capacity(4);
                    v.push(first);
                    v.push(item);
                    *self = Self::Many(v);
                }
            },
            Self::Many(v) => v.push(item),
        }
    }
}
impl<T> Deref for Rows<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        match self {
            Self::One(slot) => slot.as_slice(),
            Self::Many(v) => v,
        }
    }
}
impl<T> DerefMut for Rows<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        match self {
            Self::One(slot) => slot.as_mut_slice(),
            Self::Many(v) => v,
        }
    }
}
impl<T> FromIterator<T> for Rows<T> {
    fn from_iter<I: IntoIterator<Item = T>>(items: I) -> Self {
        let mut items = items.into_iter();
        let Some(first) = items.next() else {
            return Self::One(None);
        };
        let Some(second) = items.next() else {
            return Self::One(Some(first));
        };
        let mut v = Vec::with_capacity(items.size_hint().0 + 2);
        v.push(first);
        v.push(second);
        v.extend(items);
        Self::Many(v)
    }
}
impl<T> IntoIterator for Rows<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;
    fn into_iter(self) -> IntoIter<T> {
        match self {
            Self::One(slot) => IntoIter::One(slot.into_iter()),
            Self::Many(v) => IntoIter::Many(v.into_iter()),
        }
    }
}
impl<'r, T> IntoIterator for &'r mut Rows<T> {
    type Item = &'r mut T;
    type IntoIter = slice::IterMut<'r, T>;
    fn into_iter(self) -> slice::IterMut<'r, T> {
        self.iter_mut()
    }
}

/// The items of a [`Rows`], by value.
pub enum IntoIter<T> {
    /// Of one kept in place.
    One(option::IntoIter<T>),
    /// Of more than one.
    Many(alloc::vec::IntoIter<T>),
}
impl<T> Iterator for IntoIter<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        match self {
            Self::One(i) => i.next(),
            Self::Many(i) => i.next(),
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::One(i) => i.size_hint(),
            Self::Many(i) => i.size_hint(),
        }
    }
}

/// The rows of one kind, in the order of the walk, that belong to the
/// member at `member`: the one above it, or those at or below it.
///
/// `from` is where the rows that end before the member end; it moves on,
/// so that the members, in the order of the walk, are each given their
/// rows in one pass.
pub fn member_rows(
    spans: impl Fn(usize) -> Span,
    len: usize,
    from: &mut usize,
    member: Span,
) -> Range<usize> {
    while *from < len && spans(*from).end <= member.start {
        *from += 1;
    }
    let mut to = *from;
    while to < len && spans(to).start < member.end {
        to += 1;
    }
    *from..to
}

/// [`member_rows`] for one member, found by halves rather than in a pass.
pub fn member_rows_of<T>(rows: &[T], span: impl Fn(&T) -> Span, member: Span) -> Range<usize> {
    let from = rows.partition_point(|r| span(r).end <= member.start);
    let to = rows.partition_point(|r| span(r).start < member.end);
    from..to.max(from)
}
