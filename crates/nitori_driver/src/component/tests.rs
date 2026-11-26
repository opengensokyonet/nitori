//! A component reads its own part's arguments, and another part's by the
//! part's number.
use frunk_core::{HList, hlist, hlist::HNil};

use super::{ArgsOf, Build, Component, FindArgs, Part, PartList, part_args};
use crate::{
    id::Numbered,
    need::{EmptyPos, Pos, View},
    structure::NodeKind,
};

struct Root;
impl NodeKind for Root {
    type Children = HNil;
}

#[derive(Numbered)]
struct Base;
impl<L> Part<L> for Base {
    type Args = u32;
}
#[derive(Numbered)]
struct Extra;
impl<L> Part<L> for Extra {
    type Args = &'static str;
}
#[derive(Numbered)]
struct Other;
impl<L> Part<L> for Other {
    type Args = ();
}

/// A component of `Base`: its own arguments, and those of `Extra` when
/// the list has it.
struct Reads {
    own: u32,
    extra: Option<&'static str>,
}
impl<L, K: NodeKind> Build<Base, L, K> for Reads
where
    L: PartList<L> + FindArgs<Extra, L>,
{
    fn build<P: Pos>(_: &mut K::Start, own: &u32, args: &ArgsOf<L>, _: View<'_, HNil, P>) -> Self {
        Reads {
            own: *own,
            extra: part_args::<Extra, L>(args).copied(),
        }
    }
}

impl<L, K: NodeKind> Component<Base, L, K> for Reads where L: PartList<L> + FindArgs<Extra, L> {}

#[test]
fn own_arguments() {
    type L = HList![Other, Base, Extra];
    let args = hlist![(), 7, "x"];
    let r = <Reads as Build<Base, L, Root>>::build::<EmptyPos>(&mut (), &7, &args, HNil);
    assert_eq!((r.own, r.extra), (7, Some("x")));
}

#[test]
fn another_part_found_or_not() {
    type With = HList![Base, Other, Extra];
    let with = hlist![1, (), "e"];
    assert_eq!(part_args::<Extra, With>(&with), Some(&"e"));
    assert_eq!(part_args::<Base, With>(&with), Some(&1));
    assert_eq!(part_args::<Other, With>(&with), Some(&()));
    type Without = HList![Base, Other];
    let without = hlist![1, ()];
    assert_eq!(part_args::<Extra, Without>(&without), None);
    assert_eq!(part_args::<Base, HList![]>(&HNil), None);
}
