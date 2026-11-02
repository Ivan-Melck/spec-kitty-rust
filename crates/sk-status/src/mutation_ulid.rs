//! ULID minting for status rows.
//!
//! The pinned baseline mints every `event_id` with the `ulid` package
//! (`emit._generate_ulid`, `lifecycle_events._generate_event_id`). The native
//! writer keeps the same wire identity — a 26-character Crockford base32 ULID —
//! without adding a dependency to this crate (the coordinator owns workspace
//! manifests). The encoding is the ULID spec: 48-bit big-endian millisecond
//! timestamp followed by 80 bits of entropy, rendered most-significant first.
//!
//! [`UlidFactory`] is **monotonic within a millisecond**: a second ID minted in
//! the same millisecond increments the previous random component instead of
//! drawing fresh entropy. That matters for the batch writer, whose members can
//! share one `at` timestamp; the diary orders equal-`at` rows by `event_id`, so
//! monotonic IDs preserve the caller's sequence.
//!
//! Entropy comes from `/dev/urandom` on Unix and from a std-only
//! `RandomState`/counter/pid/time mix elsewhere. This is identifier entropy, not
//! a cryptographic secret; a forged ULID has no authority in the status model.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

/// The Crockford base32 alphabet (no `I`, `L`, `O`, `U`).
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Length of a canonical ULID string.
pub const ULID_LENGTH: usize = 26;

static FALLBACK_COUNTER: AtomicU64 = AtomicU64::new(0);
static SYSTEM_FACTORY: Mutex<Option<UlidState>> = Mutex::new(None);

/// True when `value` is the canonical 26-character Crockford ULID shape.
///
/// This is the baseline `ULID_PATTERN` (`^[0-9A-HJKMNP-TV-Z]{26}$`), used to
/// validate an injected event id before it is written.
pub fn is_ulid(value: &str) -> bool {
    // Uppercase-only, exactly the baseline `ULID_PATTERN`
    // (`^[0-9A-HJKMNP-TV-Z]{26}$`): a lowercase id would not round-trip the
    // canonical shape the readers validate.
    value.len() == ULID_LENGTH && value.bytes().all(|byte| ALPHABET.contains(&byte))
}

/// Mint a new ULID from the system clock and system entropy.
pub fn new_ulid() -> String {
    let mut guard = lock_state(&SYSTEM_FACTORY);
    let state = guard.get_or_insert_with(UlidState::system);
    state.mint(current_millis())
}

/// A deterministic, monotonic ULID source for tests and fixtures.
///
/// `UlidFactory::seeded(time_ms, seed)` produces a reproducible sequence: the
/// first ID encodes `seed`, each following ID encodes the previous random
/// component plus one. It is the writer's injection seam for differential
/// fixtures and concurrency tests that must not depend on wall-clock entropy.
#[derive(Debug, Clone)]
pub struct UlidFactory {
    state: UlidState,
    time_ms: u64,
}

impl UlidFactory {
    /// A factory pinned to `time_ms` whose first random component is `seed`.
    pub fn seeded(time_ms: u64, seed: u128) -> Self {
        let mut random = [0_u8; 10];
        let bytes = seed.to_be_bytes();
        random.copy_from_slice(&bytes[6..16]);
        Self {
            state: UlidState {
                last_ms: time_ms,
                last_random: random,
            },
            time_ms,
        }
    }

    /// The next ID in the sequence.
    pub fn mint(&mut self) -> String {
        self.state.mint(self.time_ms)
    }

    /// The fixed millisecond timestamp every ID in this factory encodes.
    pub fn time_ms(&self) -> u64 {
        self.time_ms
    }
}

#[derive(Debug, Clone)]
struct UlidState {
    last_ms: u64,
    last_random: [u8; 10],
}

impl UlidState {
    fn system() -> Self {
        Self {
            last_ms: 0,
            last_random: [0_u8; 10],
        }
    }

    fn mint(&mut self, now_ms: u64) -> String {
        if now_ms > self.last_ms {
            self.last_ms = now_ms;
            self.last_random = system_entropy();
        } else {
            // Same millisecond (or a clock that stepped backwards): increment
            // the previous random component so the sequence stays ordered.
            increment_be(&mut self.last_random);
        }
        encode(self.last_ms, &self.last_random)
    }
}

fn lock_state(state: &Mutex<Option<UlidState>>) -> MutexGuard<'_, Option<UlidState>> {
    match state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn current_millis() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis() as u64,
        Err(error) => 0_u64.saturating_sub(error.duration().as_millis() as u64),
    }
}

fn increment_be(random: &mut [u8; 10]) {
    for byte in random.iter_mut().rev() {
        let (next, overflow) = byte.overflowing_add(1);
        *byte = next;
        if !overflow {
            return;
        }
    }
}

fn system_entropy() -> [u8; 10] {
    let mut random = [0_u8; 10];
    #[cfg(unix)]
    {
        if let Ok(mut handle) = std::fs::File::open("/dev/urandom")
            && handle.read_exact(&mut random).is_ok()
        {
            return random;
        }
    }
    fallback_entropy(&mut random);
    random
}

fn fallback_entropy(random: &mut [u8; 10]) {
    let counter = FALLBACK_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos() as u64)
        .unwrap_or(0);
    let hasher_state = RandomState::new();
    let mut first = hasher_state.build_hasher();
    first.write_u64(counter);
    first.write_u64(std::process::id() as u64);
    first.write_u64(nanos);
    let mut second = hasher_state.build_hasher();
    second.write_u128(counter as u128 ^ ((std::process::id() as u128) << 64));
    second.write_u64(current_millis());
    random[..8].copy_from_slice(&first.finish().to_be_bytes());
    random[8..].copy_from_slice(&(second.finish() as u16).to_be_bytes());
}

fn encode(time_ms: u64, random: &[u8; 10]) -> String {
    let mut bytes = [0_u8; 16];
    // The ULID value is 48 bits of millisecond timestamp followed by 80 bits of
    // entropy, most-significant first.
    bytes[..6].copy_from_slice(&time_ms.to_be_bytes()[2..8]);
    bytes[6..].copy_from_slice(random);

    let value = u128::from_be_bytes(bytes);
    let mut out = [0_u8; ULID_LENGTH];
    // 26 base32 characters cover 130 bits, so the leading character encodes
    // only the top 3 bits (0-7) and the remaining 25 encode 5 bits each.
    out[0] = ALPHABET[((value >> 125) as usize) & 0x1f];
    for (index, slot) in out.iter_mut().enumerate().skip(1) {
        let shift = 125 - 5 * index;
        *slot = ALPHABET[((value >> shift) as usize) & 0x1f];
    }
    String::from_utf8(out.to_vec()).expect("the ULID alphabet is ASCII")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_matches_the_ulid_spec_example() {
        // The ULID specification's canonical example timestamp.
        assert_eq!(&encode(1_469_918_176_385, &[0_u8; 10])[..10], "01ARYZ6S41");
    }

    #[test]
    fn minted_ids_are_canonical_ulids() {
        let id = new_ulid();
        assert_eq!(id.len(), 26);
        assert!(is_ulid(&id), "{id}");
        assert!(!is_ulid("01ARZ3NDEKTSV4RRFFQ69G5FA"), "25 characters");
        assert!(
            !is_ulid("01ARZ3NDEKTSV4RRFFQ69G5FAO"),
            "O is not in the alphabet"
        );
        assert!(
            !is_ulid("01arz3ndektsv4rrffq69g5fav"),
            "lowercase is not canonical"
        );
    }

    #[test]
    fn the_timestamp_prefix_is_monotonic_through_time() {
        let mut factory = UlidFactory::seeded(1_700_000_000_000, 7);
        let first = factory.mint();
        let second = factory.mint();
        assert!(first < second, "{first} < {second}");
        assert_eq!(&first[..10], &second[..10]);
    }

    #[test]
    fn seeded_factories_reproduce_their_sequence() {
        let mut left = UlidFactory::seeded(1_700_000_000_000, 42);
        let mut right = UlidFactory::seeded(1_700_000_000_000, 42);
        for _ in 0..8 {
            assert_eq!(left.mint(), right.mint());
        }
    }

    #[test]
    fn minting_many_ids_does_not_repeat() {
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..512 {
            assert!(seen.insert(new_ulid()));
        }
    }
}
