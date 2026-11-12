// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use p2poolv2_wallet::payout_address::PayoutAddress;
use std::collections::{BTreeSet, HashMap};
use std::hash::Hash;

const INITIAL_SIZE: usize = 1000;

/// Keys are indices into a vector of slots. Each slot holds the interned
/// value or `None` when the slot is free. A map from value to key answers
/// `key_for` with one lookup. A freed slot is reused lowest first, and a
/// new slot is appended when none is free.
///
/// The window interns payout addresses (the default) and share owners, each
/// in its own `AddressKeys`.
pub(super) struct AddressKeys<T = PayoutAddress> {
    /// Slot `key` holds the value interned at `key`, or `None` when it is free.
    slots: Vec<Option<T>>,
    /// The key of every interned value.
    keys: HashMap<T, usize>,
    /// The free slots, so the lowest is reused first.
    free: BTreeSet<usize>,
}

impl<T> Default for AddressKeys<T> {
    fn default() -> Self {
        Self {
            slots: Vec::with_capacity(INITIAL_SIZE),
            keys: HashMap::new(),
            free: BTreeSet::new(),
        }
    }
}

impl<T> AddressKeys<T> {
    /// Return the number of slots in the backing vector.
    pub(super) fn len(&self) -> usize {
        self.slots.len()
    }

    /// Get the value at the index
    pub(super) fn value_for(&self, index: usize) -> Option<&T> {
        self.slots.get(index)?.as_ref()
    }
}

impl<T: Clone + Eq + Hash> AddressKeys<T> {
    /// Returns the key for `value`, found with one lookup when it is already interned.
    ///
    /// Otherwise the lowest free slot is reused, or a new slot is appended when none is free.
    pub(super) fn key_for(&mut self, value: T) -> usize {
        if let Some(key) = self.keys.get(&value) {
            return *key;
        }
        let key = match self.free.pop_first() {
            Some(key) => {
                self.slots[key] = Some(value.clone());
                key
            }
            None => {
                self.slots.push(Some(value.clone()));
                self.slots.len() - 1
            }
        };
        self.keys.insert(value, key);
        key
    }

    /// Frees the slot at `index`, dropping its value from the map of keys.
    ///
    /// An already free slot is left unchanged, and an index past the end panics.
    pub(super) fn remove(&mut self, index: usize) {
        if let Some(value) = self.slots[index].take() {
            self.keys.remove(&value);
            self.free.insert(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::parse_address_from_string;

    #[test]
    fn key_for_assigns_sequential_keys() {
        let mut address_keys = AddressKeys::default();
        let addr_a = parse_address_from_string("bcrt1qe2qaq0e8qlp425pxytrakala7725dynwhknufr");
        let addr_b = parse_address_from_string("bcrt1qlk935ze2fsu86zjp395uvtegztrkaezawxx0wf");

        assert_eq!(address_keys.key_for(addr_a), 0);
        assert_eq!(address_keys.key_for(addr_b), 1);
    }

    #[test]
    fn key_for_returns_same_key_for_same_address() {
        let mut address_keys = AddressKeys::default();
        let addr = parse_address_from_string("bcrt1qe2qaq0e8qlp425pxytrakala7725dynwhknufr");

        let key_first = address_keys.key_for(addr.clone());
        let key_second = address_keys.key_for(addr);

        assert_eq!(key_first, key_second);
    }

    #[test]
    fn value_for_returns_inserted_address() {
        let mut address_keys = AddressKeys::default();
        let addr = parse_address_from_string("bcrt1qe2qaq0e8qlp425pxytrakala7725dynwhknufr");

        let key = address_keys.key_for(addr.clone());

        assert_eq!(address_keys.value_for(key), Some(&addr));
    }

    #[test]
    fn value_for_returns_none_for_out_of_bounds() {
        let address_keys: AddressKeys = AddressKeys::default();

        assert_eq!(address_keys.value_for(0), None);
        assert_eq!(address_keys.value_for(999), None);
    }

    #[test]
    fn remove_frees_slot() {
        let mut address_keys = AddressKeys::default();
        let addr = parse_address_from_string("bcrt1qe2qaq0e8qlp425pxytrakala7725dynwhknufr");

        let key = address_keys.key_for(addr);
        address_keys.remove(key);

        assert_eq!(address_keys.value_for(key), None);
    }

    #[test]
    fn removed_slot_is_reused() {
        let mut address_keys = AddressKeys::default();
        let addr_a = parse_address_from_string("bcrt1qe2qaq0e8qlp425pxytrakala7725dynwhknufr");
        let addr_b = parse_address_from_string("bcrt1qlk935ze2fsu86zjp395uvtegztrkaezawxx0wf");
        let addr_c = parse_address_from_string("bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080");

        let key_a = address_keys.key_for(addr_a);
        let _key_b = address_keys.key_for(addr_b);
        address_keys.remove(key_a);

        let key_c = address_keys.key_for(addr_c);
        assert_eq!(key_c, key_a);
    }

    #[test]
    fn different_addresses_get_different_keys() {
        let mut address_keys = AddressKeys::default();
        let addr_a = parse_address_from_string("bcrt1qe2qaq0e8qlp425pxytrakala7725dynwhknufr");
        let addr_b = parse_address_from_string("bcrt1qlk935ze2fsu86zjp395uvtegztrkaezawxx0wf");

        let key_a = address_keys.key_for(addr_a);
        let key_b = address_keys.key_for(addr_b);

        assert_ne!(key_a, key_b);
    }

    #[test]
    fn the_lowest_free_slot_is_reused_first() {
        let mut address_keys = AddressKeys::<u32>::default();

        assert_eq!(address_keys.key_for(10), 0, "value 10 expected key 0");
        assert_eq!(address_keys.key_for(20), 1, "value 20 expected key 1");
        assert_eq!(address_keys.key_for(30), 2, "value 30 expected key 2");
        assert_eq!(address_keys.key_for(40), 3, "value 40 expected key 3");
        address_keys.remove(2);
        address_keys.remove(0);

        assert_eq!(address_keys.key_for(50), 0, "value 50 expected key 0");
        assert_eq!(address_keys.key_for(60), 2, "value 60 expected key 2");
        assert_eq!(address_keys.key_for(70), 4, "value 70 expected key 4");
        assert_eq!(address_keys.len(), 5, "value 70 expected len 5");
    }

    #[test]
    fn a_removed_value_gets_a_new_key_when_it_returns() {
        let mut address_keys = AddressKeys::<u32>::default();

        assert_eq!(address_keys.key_for(10), 0, "value 10 expected key 0");
        assert_eq!(address_keys.key_for(20), 1, "value 20 expected key 1");
        address_keys.remove(0);

        assert_eq!(address_keys.key_for(30), 0, "value 30 expected key 0");
        assert_eq!(address_keys.key_for(10), 2, "value 10 expected key 2");
        assert_eq!(
            address_keys.value_for(0),
            Some(&30),
            "value 30 expected at key 0"
        );
    }

    #[test]
    fn removing_a_free_slot_changes_nothing() {
        let mut address_keys = AddressKeys::<u32>::default();

        assert_eq!(address_keys.key_for(10), 0, "value 10 expected key 0");
        assert_eq!(address_keys.key_for(20), 1, "value 20 expected key 1");
        address_keys.remove(0);
        address_keys.remove(0);

        assert_eq!(address_keys.key_for(30), 0, "value 30 expected key 0");
        assert_eq!(address_keys.key_for(40), 2, "value 40 expected key 2");
        assert_eq!(address_keys.len(), 3, "value 40 expected len 3");
    }

    #[test]
    fn many_values_keep_their_keys() {
        let mut address_keys = AddressKeys::<u32>::default();

        for value in 0..5_000 {
            assert_eq!(
                address_keys.key_for(value),
                value as usize,
                "value {value} expected key {value}"
            );
        }
        for value in 0..5_000 {
            assert_eq!(
                address_keys.key_for(value),
                value as usize,
                "value {value} expected key {value} on the second pass"
            );
        }
        assert_eq!(
            address_keys.len(),
            5_000,
            "values 0..5000 expected len 5000"
        );
    }
}
