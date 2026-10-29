use nitori_id::{Identity, UniqueId};
use std::{
    any::TypeId,
    cmp::Ordering,
    collections::{BTreeSet, HashSet},
    hash::{BuildHasher, Hash, RandomState},
    sync::Barrier,
    thread,
};

struct First;
struct Second;
trait Marker {}

const _: () = {
    const fn shared<T: Send + Sync>() {}
    shared::<UniqueId>();
    shared::<Identity>();
};

#[test]
fn allocations_differ() {
    let unique = UniqueId::allocate();
    assert_ne!(unique, UniqueId::allocate());
    assert_eq!(unique, unique.clone());

    let identity = Identity::new::<First>();
    assert_ne!(identity, Identity::new::<First>());
    assert_eq!(identity, identity.clone());
}

#[test]
fn any_type_parameter_allocates() {
    let identities = [
        Identity::new::<First>(),
        Identity::new::<First>(),
        Identity::new::<Second>(),
        Identity::new::<str>(),
        Identity::new::<[u8]>(),
        Identity::new::<dyn Marker>(),
    ];
    assert_eq!(
        identities.iter().collect::<HashSet<_>>().len(),
        identities.len()
    );
}

#[test]
fn type_id_is_the_type_parameter() {
    assert_eq!(
        Identity::new::<First>().owner_type_id(),
        TypeId::of::<First>()
    );
    assert_eq!(Identity::new::<str>().owner_type_id(), TypeId::of::<str>());
    assert_eq!(
        Identity::new::<dyn Marker>().owner_type_id(),
        TypeId::of::<dyn Marker>()
    );
}

fn assert_consistent<T: Ord + Hash>(values: &[T]) {
    let hasher = RandomState::new();
    for a in values {
        for b in values {
            let equal = a == b;
            assert_eq!(equal, a.cmp(b) == Ordering::Equal);
            assert_eq!(a.partial_cmp(b), Some(a.cmp(b)));
            assert_eq!(a.cmp(b), b.cmp(a).reverse());
            if equal {
                assert_eq!(hasher.hash_one(a), hasher.hash_one(b));
            }
            for c in values {
                if a < b && b < c {
                    assert!(a < c);
                }
            }
        }
    }
}

#[test]
fn eq_hash_and_ord_agree() {
    let mut uniques: Vec<_> = (0..8).map(|_| UniqueId::allocate()).collect();
    uniques.extend(uniques.clone());
    assert_consistent(&uniques);
    assert_eq!(uniques.iter().collect::<HashSet<_>>().len(), 8);
    assert_eq!(uniques.iter().collect::<BTreeSet<_>>().len(), 8);

    let mut identities = Vec::new();
    for _ in 0..4 {
        identities.push(Identity::new::<First>());
        identities.push(Identity::new::<Second>());
    }
    identities.extend(identities.clone());
    assert_consistent(&identities);
    assert_eq!(identities.iter().collect::<HashSet<_>>().len(), 8);
    assert_eq!(identities.iter().collect::<BTreeSet<_>>().len(), 8);
}

#[test]
fn concurrent_allocations_differ() {
    const THREADS: usize = 8;
    const PER_THREAD: usize = 2_000;
    let start = Barrier::new(THREADS);
    let identities: Vec<Vec<_>> = thread::scope(|scope| {
        let workers: Vec<_> = (0..THREADS)
            .map(|_| {
                scope.spawn(|| {
                    start.wait();
                    (0..PER_THREAD).map(|_| Identity::new::<First>()).collect()
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect()
    });
    let identities: HashSet<_> = identities.into_iter().flatten().collect();
    assert_eq!(identities.len(), THREADS * PER_THREAD);
}

#[test]
fn debug_shows_the_type_and_the_value() {
    let identity = format!("{:?}", Identity::new::<First>());
    assert!(identity.starts_with("Identity {"), "{identity}");
    assert!(identity.contains("type_id:"), "{identity}");
    assert!(identity.contains("UniqueId("), "{identity}");
}
