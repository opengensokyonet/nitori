//! Making a driver: its tables from the walks of the composition, and its
//! root.
use alloc::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    vec::Vec,
};
use core::any::TypeId;

use frunk_core::indices::Here as I0;
use nitori_clock::MonotonicClock;

use super::{
    ClockOf, CompositionError, Driver, DriverComposition, NthPlace, Place, PlaceNth, PlaceOps,
    PlacePositionsAt, PlaceRt, PlacesOf, RequestsOf, Subscriber, Target, edit,
    wake::Shared,
    walk::{CallWalk, Cursor, PlaceWalk},
};
use crate::{
    call::{PartsOf, RootOf},
    event::Outbox,
    hooks::{ArgsOf, StartOf},
    structure::{Address, ChildAddresses, ChildList, End, LinkList, NodeKind, RemainsOf},
};

impl<Q: DriverComposition> Driver<Q>
where
    Cursor<I0>: CallWalk<Q> + PlaceWalk<Q>,
    PlacesOf<Q>: NthPlace<RootOf<Q>, I0, Item = Place<End>>,
    Q::Places: PlaceNth<Q, PlacesOf<Q>, I0>,
    Q: DriverComposition<Remains = RemainsOf<PartsOf<Q>, RootOf<Q>>>,
    PartsOf<Q>: crate::structure::RemainsList<RootOf<Q>>,
    <RootOf<Q> as NodeKind>::Children: ChildAddresses<PartsOf<Q>>,
    RequestsOf<Q>: edit::Deliver<Q, I0>,
{
    /// A driver whose root is built from `args` and `start`; it reads and
    /// sleeps on `clock`. The root's calls start woken.
    pub fn new(
        clock: ClockOf<Q>,
        args: ArgsOf<PartsOf<Q>>,
        mut start: StartOf<RootOf<Q>>,
    ) -> Result<Self, CompositionError> {
        let mut infos = Vec::new();
        <Cursor<I0> as PlaceWalk<Q>>::info(&mut infos);
        let mut slots = Vec::new();
        <Cursor<I0> as CallWalk<Q>>::info(&mut slots);
        let mut targets = Vec::new();
        <<RootOf<Q> as NodeKind>::Linked as LinkList<RootOf<Q>>>::targets(&mut targets);
        <<RootOf<Q> as NodeKind>::Children as ChildList>::link_targets(&mut targets);
        let mut places = Vec::new();
        for (p, info) in infos.into_iter().enumerate() {
            let calls: Vec<usize> = (0..slots.len())
                .filter(|&s| slots[s].place == info.path)
                .collect();
            let before: Vec<usize> = (0..info.comps.len())
                .filter(|&c| info.comps[c].acts_before_calls)
                .collect();
            if calls.len() + before.len() > 64 {
                return Err(CompositionError::TooManyPositions { place: p });
            }
            let mut handled = Vec::new();
            for (c, comp) in info.comps.iter().enumerate() {
                let h: Vec<TypeId> = comp
                    .subscriptions
                    .iter()
                    .filter(|s| s.handled)
                    .map(|s| s.event)
                    .collect();
                if h.len() > 64 {
                    return Err(CompositionError::TooManySubscriptions {
                        place: p,
                        component: c,
                    });
                }
                handled.push(h);
            }
            places.push(PlaceRt {
                link_target: targets.contains(&info.kind),
                calls,
                before,
                handled,
                children: Vec::new(),
                link: None,
                info,
            });
        }
        // a link goes to the root's child of the kind it names, under the
        // linking node's own key
        for q in 0..places.len() {
            let Some(&target) = places[q].info.links.first() else {
                continue;
            };
            places[q].link = places
                .iter()
                .position(|p| p.info.parent == Some(places[0].info.path) && p.info.kind == target);
        }
        for q in 0..places.len() {
            let (Some(parent), Some(i)) = (places[q].info.parent, places[q].info.index) else {
                continue;
            };
            if let Some(p) = places.iter().position(|p| p.info.path == parent) {
                let children = &mut places[p].children;
                if children.len() <= i {
                    children.resize(i + 1, None);
                }
                children[i] = Some(q);
            }
        }
        let mut subscribers = Vec::new();
        for (p, place) in places.iter().enumerate() {
            for (c, comp) in place.info.comps.iter().enumerate() {
                let mut bit = 0;
                for s in &comp.subscriptions {
                    let this = s.handled.then(|| {
                        bit += 1;
                        bit - 1
                    });
                    subscribers.push((
                        s.event,
                        Subscriber {
                            place: p,
                            target: Target::Comp(c),
                            bit: this,
                            scope: s.scope,
                        },
                    ));
                }
            }
            for (k, &s) in place.calls.iter().enumerate() {
                for sub in &slots[s].subscriptions {
                    subscribers.push((
                        sub.event,
                        Subscriber {
                            place: p,
                            target: Target::Call(k),
                            bit: None,
                            scope: sub.scope,
                        },
                    ));
                }
            }
        }
        subscribers.sort_by_key(|&(e, _)| e);
        let now = clock.now();
        let outbox = Outbox::at(now);
        // Invariant: the root's components have no needs at a parent
        // (`BuildAt` for `End`), so building it lends nothing.
        let root = <PlacePositionsAt<Q, I0> as PlaceOps<Q, Place<End>>>::build(
            None,
            &(),
            &(),
            &args,
            &mut start,
            &outbox,
        )
        .expect("building the root lends nothing");
        let mut driver = Self {
            root: Some(root),
            args,
            calls: Default::default(),
            nodes: Vec::new(),
            places,
            slots,
            subscribers,
            shared: Arc::new(Shared::default()),
            outer: None,
            epoch: 0,
            clock,
            sleep: None,
            timers: BTreeSet::new(),
            deadlines: BTreeMap::new(),
            outbox,
            pending: Vec::new(),
            deferred: Vec::new(),
            deferred_timers: Vec::new(),
            serial: 0,
            remains: None,
            ended: false,
            running: false,
            on_drop: Self::end_dropped,
        };
        // a hook that panics leaves `running` set, so the drop of the
        // driver on the way out ends it as after a panic
        driver.running = true;
        let id = driver.register(Address::here(), 0, None);
        let mut todo = Vec::new();
        driver.started(id, &mut todo);
        driver.settle(todo);
        driver.running = false;
        Ok(driver)
    }
}
