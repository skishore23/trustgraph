//! Shared reader-set information-flow algebra.
//!
//! Combining inputs may only narrow their allowed reader set. These functions
//! do not resolve identities, capture dependencies, or authorize an action:
//! callers must validate the complete evidence and the destination separately.
use std::collections::BTreeSet;

/// Visit common items from strictly increasing iterators without allocating.
/// BTreeSet supplies the sorted/unique precondition in the public adapter.
fn visit_intersection<T: Ord>(
    left: impl Iterator<Item = T>,
    right: impl Iterator<Item = T>,
    mut emit: impl FnMut(T),
) {
    let mut left = left.peekable();
    let mut right = right.peekable();
    while let (Some(a), Some(b)) = (left.peek(), right.peek()) {
        match a.cmp(b) {
            std::cmp::Ordering::Less => {
                left.next();
            }
            std::cmp::Ordering::Greater => {
                right.next();
            }
            std::cmp::Ordering::Equal => {
                if let Some(item) = left.next() {
                    emit(item);
                }
                right.next();
            }
        }
    }
}

/// Check inclusion of sorted, unique recipients in sorted, unique readers.
/// No allocation and at most a linear scan through both iterators.
fn sorted_subset<T: Ord>(
    readers: impl Iterator<Item = T>,
    recipients: impl Iterator<Item = T>,
) -> bool {
    let mut readers = readers.peekable();
    for recipient in recipients {
        while readers.peek().is_some_and(|reader| reader < &recipient) {
            readers.next();
        }
        if readers.peek() != Some(&recipient) {
            return false;
        }
    }
    true
}

/// Readers of a value derived from both inputs. Empty means nobody, not public.
pub fn intersect<T: Ord + Clone>(left: &BTreeSet<T>, right: &BTreeSet<T>) -> BTreeSet<T> {
    let mut result = Vec::with_capacity(left.len().min(right.len()));
    visit_intersection(left.iter(), right.iter(), |reader| {
        result.push(reader.clone())
    });
    result.into_iter().collect()
}

/// Whether an already resolved destination can receive a value with `readers`.
/// A missing/unresolved destination must be rejected by the caller, not passed
/// here as an empty set.
pub fn can_flow<T: Ord>(readers: &BTreeSet<T>, recipients: &BTreeSet<T>) -> bool {
    sorted_subset(readers.iter(), recipients.iter())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(mask: u8) -> BTreeSet<u8> {
        (0..3).filter(|bit| mask & (1 << bit) != 0).collect()
    }

    proptest::proptest! {
        #[test]
        fn adapters_match_standard_sets(
            left in proptest::collection::vec(proptest::prelude::any::<u16>(), 0..64),
            right in proptest::collection::vec(proptest::prelude::any::<u16>(), 0..64),
        ) {
            let left: BTreeSet<_> = left.into_iter().collect();
            let right: BTreeSet<_> = right.into_iter().collect();
            proptest::prop_assert_eq!(intersect(&left, &right), left.intersection(&right).copied().collect());
            proptest::prop_assert_eq!(can_flow(&left, &right), right.is_subset(&left));
        }
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

/// Kani checks the production allocation-free kernels. BTreeSet allocation,
/// iteration, and collection are adapters covered by ordinary differential tests.
#[cfg(kani)]
mod proofs {
    use super::*;

    struct Readers {
        items: [u8; 3],
        len: usize,
    }
    impl Readers {
        fn arbitrary() -> Self {
            let membership: [bool; 3] = kani::any();
            let mut result = Self {
                items: [0; 3],
                len: 0,
            };
            for (principal, included) in membership.into_iter().enumerate() {
                if included {
                    result.items[result.len] = principal as u8;
                    result.len += 1;
                }
            }
            result
        }
        fn slice(&self) -> &[u8] {
            &self.items[..self.len]
        }
        fn intersect(&self, other: &Self) -> Self {
            let mut result = Self {
                items: [0; 3],
                len: 0,
            };
            visit_intersection(self.slice().iter(), other.slice().iter(), |reader| {
                result.items[result.len] = *reader;
                result.len += 1;
            });
            result
        }
        fn allows(&self, recipients: &Self) -> bool {
            sorted_subset(self.slice().iter(), recipients.slice().iter())
        }
    }

    // Arbitrary u8 keys exercise comparisons beyond the fixed three-principal
    // universe. Canonical sorted inputs are the kernel's explicit precondition.
    #[kani::proof]
    #[kani::unwind(8)]
    fn arbitrary_keys_match_set_specification() {
        let a: [u8; 3] = kani::any();
        let b: [u8; 3] = kani::any();
        let alen: usize = kani::any();
        let blen: usize = kani::any();
        kani::assume(alen <= 3 && blen <= 3);
        let a = &a[..alen];
        let b = &b[..blen];
        kani::assume(a.windows(2).all(|pair| pair[0] < pair[1]));
        kani::assume(b.windows(2).all(|pair| pair[0] < pair[1]));
        let mut output = [0u8; 3];
        let mut len = 0;
        visit_intersection(a.iter(), b.iter(), |item| {
            output[len] = *item;
            len += 1;
        });
        let output = &output[..len];
        assert!(output.iter().all(|p| a.contains(p) && b.contains(p)));
        assert!(
            a.iter()
                .filter(|p| b.contains(p))
                .all(|p| output.contains(p))
        );
        assert!(output.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(
            sorted_subset(a.iter(), b.iter()),
            b.iter().all(|p| a.contains(p))
        );
    }

    #[kani::proof]
    #[kani::unwind(5)]
    fn intersection_kernel_matches_membership() {
        let a = Readers::arbitrary();
        let b = Readers::arbitrary();
        let output = a.intersect(&b);
        for principal in 0..3 {
            assert_eq!(
                output.slice().contains(&principal),
                a.slice().contains(&principal) && b.slice().contains(&principal)
            );
        }
        assert!(output.slice().windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[kani::proof]
    #[kani::unwind(5)]
    fn subset_kernel_matches_membership() {
        let readers = Readers::arbitrary();
        let recipients = Readers::arbitrary();
        assert_eq!(
            readers.allows(&recipients),
            recipients
                .slice()
                .iter()
                .all(|p| readers.slice().contains(p))
        );
    }

    #[kani::proof]
    #[kani::unwind(5)]
    fn composition_is_associative() {
        let a = Readers::arbitrary();
        let b = Readers::arbitrary();
        let c = Readers::arbitrary();
        assert_eq!(
            a.intersect(&b).intersect(&c).slice(),
            a.intersect(&b.intersect(&c)).slice()
        );
    }

    #[kani::proof]
    #[kani::unwind(5)]
    fn duplicate_or_reordered_inputs_preserve_readers() {
        let a = Readers::arbitrary();
        let b = Readers::arbitrary();
        assert_eq!(a.intersect(&a).slice(), a.slice());
        assert_eq!(a.intersect(&b).slice(), b.intersect(&a).slice());
    }

    #[kani::proof]
    #[kani::unwind(5)]
    fn recipient_must_be_permitted_by_every_input() {
        let a = Readers::arbitrary();
        let b = Readers::arbitrary();
        let destination = Readers::arbitrary();
        assert_eq!(
            a.intersect(&b).allows(&destination),
            a.allows(&destination) && b.allows(&destination)
        );
    }
}
