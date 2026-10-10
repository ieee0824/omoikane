use super::run_test;
use crate::{Rooted, WeakMap, force_collect, has_weak_maps};

#[test]
fn weak_map_basic() {
    run_test(|| {
        let key1 = Rooted::new(String::from("key1"));
        let key2 = Rooted::new(String::from("key2"));
        let key3 = Rooted::new(String::from("key3"));

        assert!(!has_weak_maps());

        let mut map = WeakMap::new();

        assert!(has_weak_maps());

        map.insert(&key1, ());
        map.insert(&key2, ());
        map.insert(&key3, ());

        force_collect();
        assert!(has_weak_maps());

        assert!(map.contains_key(&key1));
        assert!(map.contains_key(&key2));
        assert!(map.contains_key(&key3));

        drop(key1);

        force_collect();
        assert!(has_weak_maps());

        assert!(map.contains_key(&key2));
        assert!(map.contains_key(&key3));

        drop(key2);

        force_collect();
        assert!(has_weak_maps());

        assert!(map.contains_key(&key3));
        assert!(has_weak_maps());

        drop(key3);

        assert!(has_weak_maps());

        force_collect();
        assert!(has_weak_maps());

        drop(map);

        force_collect();
        assert!(!has_weak_maps());
    });
}

#[test]
fn weak_map_multiple() {
    run_test(|| {
        let key1 = Rooted::new(String::from("key1"));
        let key2 = Rooted::new(String::from("key2"));
        let key3 = Rooted::new(String::from("key3"));

        assert!(!has_weak_maps());

        let mut map_1 = WeakMap::new();
        let mut map_2 = WeakMap::new();

        assert!(has_weak_maps());

        map_1.insert(&key1, ());
        map_1.insert(&key2, ());
        map_2.insert(&key3, ());

        force_collect();
        assert!(has_weak_maps());

        assert!(map_1.contains_key(&key1));
        assert!(map_1.contains_key(&key2));
        assert!(!map_1.contains_key(&key3));
        assert!(!map_2.contains_key(&key1));
        assert!(!map_2.contains_key(&key2));
        assert!(map_2.contains_key(&key3));

        force_collect();
        assert!(has_weak_maps());

        drop(key1);
        drop(key2);

        force_collect();
        assert!(has_weak_maps());

        assert!(!map_1.contains_key(&key3));
        assert!(map_2.contains_key(&key3));

        drop(key3);

        force_collect();
        assert!(has_weak_maps());

        drop(map_1);

        force_collect();
        assert!(has_weak_maps());

        drop(map_2);

        force_collect();
        assert!(!has_weak_maps());
    });
}

#[test]
fn weak_map_key_live() {
    run_test(|| {
        let key = Rooted::new(String::from("key"));
        let key_copy = key.clone();

        let mut map = WeakMap::new();

        map.insert(&key, ());

        assert!(map.contains_key(&key));
        assert!(map.contains_key(&key_copy));

        assert_eq!(map.remove(&key), Some(()));

        map.insert(&key, ());

        drop(key);

        force_collect();

        assert!(map.contains_key(&key_copy));
    });
}

struct PresenceCloneSpy {
    value: u32,
    clones: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl Clone for PresenceCloneSpy {
    fn clone(&self) -> Self {
        self.clones
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self {
            value: self.value,
            clones: std::sync::Arc::clone(&self.clones),
        }
    }
}

impl crate::Finalize for PresenceCloneSpy {}

// SAFETY: the payload and its non-GC atomic counter contain no GC edges.
unsafe impl crate::Trace for PresenceCloneSpy {
    crate::empty_trace!();
}

#[test]
fn weak_map_presence_does_not_clone_values_but_get_still_does() {
    run_test(|| {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let clones = Arc::new(AtomicUsize::new(0));
        let key = Rooted::new(7_u32);
        let alias = key.clone();
        let different_key = Rooted::new(7_u32);
        let mut map = WeakMap::new();
        map.insert(
            &key,
            PresenceCloneSpy {
                value: 11,
                clones: Arc::clone(&clones),
            },
        );

        assert!(map.contains_key(&key));
        assert!(map.contains_key(&alias));
        assert!(!map.contains_key(&different_key));
        assert_eq!(clones.load(Ordering::Relaxed), 0);

        drop(key);
        force_collect();
        assert!(map.contains_key(&alias));
        assert_eq!(clones.load(Ordering::Relaxed), 0);

        let owned = map.get(&alias).expect("a rooted alias retains the entry");
        assert_eq!(owned.value, 11);
        assert_eq!(clones.load(Ordering::Relaxed), 1);
    });
}

#[test]
fn weak_map_gc_cleanup_does_not_clone_values_and_reclaims_dead_keys() {
    run_test(|| {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        let clones = Arc::new(AtomicUsize::new(0));
        let live = Rooted::new(1_u32);
        let dead = Rooted::new(2_u32);
        let mut map = WeakMap::new();
        map.insert(
            &live,
            PresenceCloneSpy {
                value: 11,
                clones: Arc::clone(&clones),
            },
        );
        map.insert(
            &dead,
            PresenceCloneSpy {
                value: 22,
                clones: Arc::clone(&clones),
            },
        );
        drop(dead);

        crate::force_minor_collect();
        assert_eq!(map.inner.borrow().len(), 1);
        assert_eq!(clones.load(Ordering::Relaxed), 0);
        let owned = map
            .get(&live)
            .expect("the live key keeps its original value");
        assert_eq!(owned.value, 11);
        assert_eq!(clones.load(Ordering::Relaxed), 1);

        force_collect();
        assert_eq!(map.inner.borrow().len(), 1);
        assert_eq!(clones.load(Ordering::Relaxed), 1);
        drop(live);
        force_collect();
        assert_eq!(map.inner.borrow().len(), 0);
        assert_eq!(clones.load(Ordering::Relaxed), 1);

        drop(map);
        force_collect();
        assert!(!has_weak_maps());
    });
}
