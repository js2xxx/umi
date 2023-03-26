#![cfg_attr(not(feature = "test"), no_std)]
#![feature(type_alias_impl_trait)]

extern crate alloc;

use alloc::collections::{
    btree_map::{
        Entry::{Occupied, Vacant},
        VacantEntry,
    },
    BTreeMap,
};
use core::{alloc::Layout, borrow::Borrow, ops::Range};

use rand_riscv::rand_core::RngCore;

pub struct RangeMap<K, V> {
    range: Range<K>,
    map: BTreeMap<K, (K, V)>,
}

impl<K, V> RangeMap<K, V> {
    pub const fn new(range: Range<K>) -> Self {
        RangeMap {
            range,
            map: BTreeMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

impl<K: Ord, V> RangeMap<K, V> {
    pub fn try_insert(&mut self, key: Range<K>, value: V) -> Result<(), V> {
        if key.start >= key.end {
            return Err(value);
        }
        if !(self.range.start <= key.start && key.end <= self.range.end) {
            return Err(value);
        }

        let prev = self.map.range(..&key.end).last();
        if let Some((_, (prev_end, _))) = prev {
            if prev_end > &key.start {
                return Err(value);
            }
        }
        let old = self.map.insert(key.start, (key.end, value));
        debug_assert!(old.is_none());
        Ok(())
    }

    pub fn try_entry(&mut self, key: Range<K>) -> Option<Entry<'_, K, V>> {
        if key.start >= key.end {
            return None;
        }
        if !(self.range.start <= key.start && key.end <= self.range.end) {
            return None;
        }

        let prev = self.map.range(..&key.end).last();
        if let Some((_, (prev_end, _))) = prev {
            if prev_end > &key.start {
                return None;
            }
        }
        Some(match self.map.entry(key.start) {
            Vacant(entry) => Entry {
                entry,
                end: key.end,
            },
            Occupied(_) => unreachable!(),
        })
    }
}

pub enum FindResult<K> {
    Found(Range<K>),
    /// Also means retry if returned after a round has finished.
    Next,
    NotFound,
}

impl<K> FindResult<K> {
    pub fn map<Q>(self, mut map: impl FnMut(K) -> Q) -> FindResult<Q> {
        match self {
            FindResult::Found(range) => FindResult::Found(map(range.start)..map(range.end)),
            FindResult::Next => FindResult::Next,
            FindResult::NotFound => FindResult::NotFound,
        }
    }
}

pub struct Entry<'a, K, V> {
    entry: VacantEntry<'a, K, (K, V)>,
    end: K,
}

impl<'a, K: Ord, V> Entry<'a, K, V> {
    pub fn key(&self) -> Range<&K> {
        self.entry.key()..&self.end
    }

    pub fn into_key(self) -> Range<K> {
        self.entry.into_key()..self.end
    }

    pub fn insert(self, value: V) -> &'a mut V {
        &mut self.entry.insert((self.end, value)).1
    }
}

impl<K: Ord, V> RangeMap<K, V> {
    fn find_gap<F>(&self, mut predicate: F) -> FindResult<K>
    where
        F: FnMut(Option<Range<&K>>) -> FindResult<K>,
    {
        let mut start = &self.range.start;
        for (base, (end, _)) in &self.map {
            if start < base {
                match predicate(Some(start..base)) {
                    FindResult::Next => {}
                    ret => return ret,
                }
            }
            start = end;
        }
        if start < &self.range.end {
            match predicate(Some(start..&self.range.end)) {
                FindResult::Next => {}
                ret => return ret,
            }
        }
        predicate(None)
    }

    /// Allocate a range with the given `predicate`.
    ///
    /// # Arguments
    ///
    /// - `predicate` - receives ranges in one round, or a `None` when a round
    ///   finished, and returns whether the round should continue, retry or break
    ///   with or without a result.
    pub fn allocate_with<F>(&mut self, mut predicate: F) -> Option<Entry<'_, K, V>>
    where
        F: FnMut(Option<Range<&K>>) -> FindResult<K>,
    {
        let key = loop {
            match self.find_gap(&mut predicate) {
                FindResult::Found(key) => break key,
                FindResult::Next => {}
                FindResult::NotFound => return None,
            }
        };

        Some(match self.map.entry(key.start) {
            Vacant(entry) => Entry {
                entry,
                end: key.end,
            },
            Occupied(_) => unreachable!(),
        })
    }

    pub fn allocate_with_aslr<R, C>(
        &mut self,
        mut aslr_key: AslrKey<R>,
        convert: C,
    ) -> Option<Entry<'_, K, V>>
    where
        R: RngCore,
        C: Fn(&K) -> usize,
        K: From<usize>,
    {
        self.allocate_with(move |key| {
            let key = key.map(|key| convert(key.start)..convert(key.end));
            aslr_key.find_key_usize(key).map(From::from)
        })
    }
}

impl<K: Ord, V> RangeMap<K, V> {
    pub fn get<Q>(&self, start: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.map.get(start).map(|(_, value)| value)
    }

    pub fn get_mut<Q>(&mut self, start: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.map.get_mut(start).map(|(_, value)| value)
    }

    pub fn get_key_value<'a, Q>(&'a self, start: &'a Q) -> Option<(Range<&K>, &V)>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.map
            .get_key_value(start)
            .map(move |(start, (end, value))| (start..end, value))
    }

    pub fn remove<Q>(&mut self, start: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.map.remove(start).map(|(_, value)| value)
    }

    pub fn remove_entry<Q>(&mut self, start: &Q) -> Option<(Range<K>, V)>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.map
            .remove_entry(start)
            .map(|(start, (end, value))| (start..end, value))
    }

    pub fn iter(&self) -> impl Iterator<Item = (Range<&K>, &V)> + '_ {
        let iter = self.map.iter();
        iter.map(|(start, (end, value))| (start..end, value))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (Range<&K>, &mut V)> + '_ {
        let iter = self.map.iter_mut();
        iter.map(|(start, (end, value))| (start..&*end, value))
    }

    /// Returns all the references of entries that consists in the given
    /// `range`.
    pub fn range<'a, Q>(&'a self, range: Range<Q>) -> impl Iterator<Item = (Range<&K>, &V)> + 'a
    where
        K: Borrow<Q>,
        Q: Ord + 'a,
    {
        let iter = self.map.range(range.start.borrow()..);
        iter.take_while(move |(_, (end, _))| end.borrow() <= &range.end)
            .map(|(start, (end, value))| (start..end, value))
    }

    /// Returns all the references of entries that consists in the given
    /// `range`.
    pub fn range_mut<'a, Q>(
        &'a mut self,
        range: Range<Q>,
    ) -> impl Iterator<Item = (Range<&K>, &mut V)> + 'a
    where
        K: Borrow<Q>,
        Q: Ord + 'a,
    {
        let iter = self.map.range_mut(range.start.borrow()..);
        iter.take_while(move |(_, (end, _))| end.borrow() <= &range.end)
            .map(|(start, (end, value))| (start..&*end, value))
    }

    /// Returns all the references of entries that intersects with the given
    /// `range`.
    ///
    /// Due to algorithm reasons, the iterator starts from the end of the range.
    pub fn intersection<'a, Q>(
        &'a self,
        range: Range<Q>,
    ) -> impl Iterator<Item = (Range<&K>, &V)> + 'a
    where
        K: Borrow<Q>,
        Q: Ord + 'a,
    {
        let iter = self.map.range(..range.end.borrow()).rev();
        iter.take_while(move |(_, (end, _))| &range.start < end.borrow())
            .map(|(start, (end, value))| (start..end, value))
    }

    pub fn intersects<'a, Q>(&self, range: Range<Q>) -> bool
    where
        K: Borrow<Q>,
        Q: Ord + 'a,
    {
        self.intersection(range).next().is_some()
    }

    /// Returns all the mutable references of entries that intersects with the
    /// given `range`.
    ///
    /// Due to algorithm reasons, the iterator starts from the end of the range.
    pub fn intersection_mut<'a, Q>(
        &'a mut self,
        range: Range<Q>,
    ) -> impl Iterator<Item = (Range<&K>, &mut V)> + 'a
    where
        K: Borrow<Q>,
        Q: Ord + 'a,
    {
        let iter = self.map.range_mut(..range.end.borrow()).rev();
        iter.take_while(move |(_, (end, _))| &range.start < end.borrow())
            .map(|(start, (end, value))| (start..&*end, value))
    }

    /// Remove entries consisting in the given `range`, retaining the rest.
    ///
    /// Returns the iterator of the removed entries. It's an eager operation,
    /// which means it'll always remove these entries even if the iterator
    /// is not used.
    pub fn drain<Q>(&mut self, range: Range<Q>) -> impl Iterator<Item = (Range<K>, V)>
    where
        K: Borrow<Q>,
        Q: Ord,
    {
        let mut ret = self.map.split_off(range.start.borrow());
        let mut suffix = ret.split_off(range.end.borrow());
        self.map.append(&mut suffix);
        if let Some(entry) = ret.last_entry() {
            let (end, _) = entry.get();
            if &range.end < end.borrow() {
                let (k, v) = entry.remove_entry();
                self.map.insert(k, v);
            }
        }
        ret.into_iter()
            .map(|(start, (end, value))| (start..end, value))
    }
}

impl<K: Ord, V> IntoIterator for RangeMap<K, V> {
    type Item = (Range<K>, V);

    type IntoIter = impl Iterator<Item = (Range<K>, V)>;

    fn into_iter(self) -> Self::IntoIter {
        let iter = self.map.into_iter();
        iter.map(|(start, (end, value))| (start..end, value))
    }
}

pub struct AslrKey<R> {
    rng: R,
    retried: bool,
    entropy: usize,
    count: usize,
    layout: Layout,
}

impl<R: RngCore> AslrKey<R> {
    pub fn new(aslr_bit: u32, mut rng: R, layout: Layout) -> Self {
        let mask = (1 << aslr_bit) - 1;
        let entropy = rng.next_u64() as usize & mask;
        AslrKey {
            rng,
            retried: false,
            entropy,
            count: 0,
            layout,
        }
    }

    pub fn find_key_usize(&mut self, key: Option<Range<usize>>) -> FindResult<usize> {
        let Some(key) = key else {
            // A round has finished.
            return if self.retried || self.count == 0 {
                // Really can't found any.
                FindResult::NotFound
            } else {
                // Retry using the entropy from the last trial.
                self.retried = true;
                self.entropy = self.rng.next_u64() as usize % self.count;
                self.count = 0;
                FindResult::Next
            }
        };

        // Equals to log2(align) because it must be a power of 2.
        let shift = self.layout.align().trailing_zeros();
        let mask = self.layout.align() - 1;

        // Round the gap to the aligned addresses.
        let base = (key.start + mask) & !mask;
        let end = key.end & !mask;

        // Check if the gap has enough space.
        if self.layout.size() <= end.saturating_sub(base) {
            // Get the number of positions the result range can lie in.
            let nr_positions = ((end - base - self.layout.size()) >> shift) + 1;
            // Add to the current position count.
            self.count += nr_positions;
            // Check if the remaining entropy is not enough.
            if self.entropy < nr_positions {
                // Get the actual position.
                let base = base + (self.entropy << shift);
                return FindResult::Found(base..(base + self.layout.size()));
            }
            // Reduce the current entropy.
            self.entropy -= nr_positions;
        }
        FindResult::Next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic() {
        let mut map = RangeMap::new(0..100);

        assert!(map.try_insert(1..1000, "z").is_err());
        assert!(map.try_insert(1..10, "a").is_ok());
        assert_eq!(map.try_insert(2..12, "b"), Err("b"));
        map.try_insert(13..20, "c").unwrap();
        map.try_insert(24..30, "d").unwrap();
        map.try_insert(32..40, "e").unwrap();
        map.try_insert(44..50, "f").unwrap();

        let intersections = map
            .intersection(17..37)
            .map(|(_, &v)| v)
            .collect::<Vec<_>>();
        assert_eq!(intersections, ["e", "d", "c"]);

        let range = map.range(17..46).map(|(_, &v)| v).collect::<Vec<_>>();
        assert_eq!(range, ["d", "e"]);

        let drain = map.drain(7..36).map(|(_, v)| v).collect::<Vec<_>>();
        assert_eq!(drain, ["c", "d"]);

        let rem = map.into_iter().map(|(_, v)| v).collect::<Vec<_>>();
        assert_eq!(rem, ["a", "e", "f"]);
    }

    #[test]
    fn test_border_conditions() {
        let mut map = RangeMap::new(0..100);

        map.try_insert(0..33, "x").unwrap();
        map.try_insert(33..67, "y").unwrap();
        map.try_insert(67..100, "z").unwrap();

        let intersection = map
            .intersection(33..67)
            .map(|(_, &v)| v)
            .collect::<Vec<_>>();
        assert_eq!(intersection, ["y"]);

        let range = map.range(33..67).map(|(_, &v)| v).collect::<Vec<_>>();
        assert_eq!(range, ["y"]);

        let drain = map.drain(33..67).map(|(_, v)| v).collect::<Vec<_>>();
        assert_eq!(drain, ["y"]);

        let rem = map.into_iter().map(|(_, v)| v).collect::<Vec<_>>();
        assert_eq!(rem, ["x", "z"]);
    }

    #[test]
    fn test_aslr_key_aligned() {
        const L: Layout = Layout::new::<u8>();

        // Repeating test because it involves random.
        for _ in 0..100 {
            let mut map = RangeMap::new(0..20);
            map.try_insert(0..2, "x").unwrap();
            map.try_insert(10..13, "y").unwrap();
            map.try_insert(17..19, "z").unwrap();

            let ak = AslrKey::new(3, rand::thread_rng(), L);
            let ret = map.allocate_with_aslr(ak, Clone::clone);
            let ret = ret.unwrap();
            assert!(*ret.key().start < 10);

            let ak = AslrKey::new(4, rand::thread_rng(), L);
            let ret = map.allocate_with_aslr(ak, Clone::clone);
            let ret = ret.unwrap();
            assert!(*ret.key().start < 20)
        }
    }
}
