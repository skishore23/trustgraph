//! Shared reader-set information-flow algebra.
//!
//! Combining inputs may only narrow their allowed reader set. These functions
//! do not resolve identities, capture dependencies, or authorize an action:
//! callers must validate the complete evidence and the destination separately.
use std::collections::BTreeSet;

/// Readers of a value derived from both inputs. Empty means nobody, not public.
pub fn intersect<T: Ord + Clone>(left: &BTreeSet<T>, right: &BTreeSet<T>) -> BTreeSet<T> {
    left.intersection(right).cloned().collect()
}

/// Whether an already resolved destination can receive a value with `readers`.
/// A missing/unresolved destination must be rejected by the caller, not passed
/// here as an empty set.
pub fn can_flow<T: Ord>(readers: &BTreeSet<T>, recipients: &BTreeSet<T>) -> bool {
    recipients.is_subset(readers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(mask: u8) -> BTreeSet<u8> {
        (0..3).filter(|bit| mask & (1 << bit) != 0).collect()
    }

    #[test]
    fn finite_reader_algebra_laws() {
        for a in 0..8 {
            let a = set(a);
            assert_eq!(intersect(&a, &a), a);
            for b in 0..8 {
                let b = set(b);
                let ab = intersect(&a, &b);
                assert_eq!(ab, intersect(&b, &a));
                assert!(ab.is_subset(&a) && ab.is_subset(&b));
                for c in 0..8 {
                    let c = set(c);
                    assert_eq!(intersect(&ab, &c), intersect(&a, &intersect(&b, &c)));
                    assert_eq!(can_flow(&ab, &c), can_flow(&a, &c) && can_flow(&b, &c));
                }
            }
        }
    }
}

/// Bounded proofs execute the production functions above, not a second model.
/// The principal universe is {0,1,2}; each membership bit is symbolic.
#[cfg(kani)]
mod proofs {
    use super::*;

    fn arbitrary_readers() -> BTreeSet<u8> {
        let membership: [bool; 3] = kani::any();
        let mut readers = BTreeSet::new();
        for (principal, included) in membership.into_iter().enumerate() {
            if included {
                readers.insert(principal as u8);
            }
        }
        readers
    }

    #[kani::proof]
    #[kani::unwind(16)]
    fn intersection_never_widens() {
        let left = arbitrary_readers();
        let right = arbitrary_readers();
        let output = intersect(&left, &right);
        assert!(output.is_subset(&left));
        assert!(output.is_subset(&right));
    }

    #[kani::proof]
    #[kani::unwind(16)]
    fn composition_is_associative() {
        let a = arbitrary_readers();
        let b = arbitrary_readers();
        let c = arbitrary_readers();
        assert_eq!(
            intersect(&intersect(&a, &b), &c),
            intersect(&a, &intersect(&b, &c))
        );
    }

    #[kani::proof]
    #[kani::unwind(16)]
    fn duplicate_or_reordered_inputs_preserve_readers() {
        let a = arbitrary_readers();
        let b = arbitrary_readers();
        assert_eq!(intersect(&a, &a), a);
        assert_eq!(intersect(&a, &b), intersect(&b, &a));
    }

    #[kani::proof]
    #[kani::unwind(16)]
    fn recipient_must_be_permitted_by_every_input() {
        let a = arbitrary_readers();
        let b = arbitrary_readers();
        let destination = arbitrary_readers();
        assert_eq!(
            can_flow(&intersect(&a, &b), &destination),
            can_flow(&a, &destination) && can_flow(&b, &destination)
        );
    }
}
