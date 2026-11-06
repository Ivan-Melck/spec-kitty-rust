//! CPython 3.11.15 list-sort engine (natural runs, binary insertion, powersort).
//!
//! `list.sort()` / `sorted()` first count a natural run, reverse a strictly
//! descending one, extend a short run to `minrun` with binary insertion, and
//! merge pending runs with CPython 3.11's *powersort* policy (a merge tree whose
//! node "power" comes from the ideal binary tree; galloping merges once one side
//! wins `MIN_GALLOP` times in a row). That merge policy is what decides the
//! comparison and error sequence for a non-transitive author such as `NaN`, so a
//! plain insertion sort is not equivalent even for short inputs.
//!
//! This module reproduces the algorithm, not CPython's C text: an original safe
//! Rust implementation over the tuple `<` predicate owned by [`super::order`].
//! Keys are computed once by [`python_json_stable_sort_by_key`]; the engine sorts
//! a permutation of key indices, so comparison order and stability match the
//! reference and a refusal leaves the caller's items untouched (`sorted()`
//! discards its own partial list, the input iterable is unchanged).
//!
//! Algorithm reference studied from the pinned reference:
//! `https://raw.githubusercontent.com/python/cpython/v3.11.15/Objects/listobject.c`
//! (tag `v3.11.15`, commit `2340a037f7450e70fccfe411e6531afb4d57a312`,
//! sha256 `ae3b2ad9b993af5666c5e007319ba896d92dc6bcc5c1e4a1af7f97a7558001a2`,
//! PSF-2.0/MIT-licensed CPython distribution; studied only, no C text copied).

use std::marker::PhantomData;

use super::order::python_json_tuple_lt;
use super::{PythonJsonCompareError, PythonJsonValue};

/// CPython's threshold for entering/leaving galloping merge mode.
const MIN_GALLOP: isize = 7;

/// Runs shorter than CPython's minimum are extended by binary insertion.
const MIN_RUN: usize = 64;

#[derive(Clone, Copy, Debug)]
struct Run {
    start: usize,
    len: usize,
    power: usize,
}

/// One pending run stack plus its immutable keys and movable index permutation.
struct SortState<'a, K, E, F>
where
    F: Fn(&K, &K) -> Result<bool, E>,
{
    keys: &'a [K],
    less: F,
    order: Vec<usize>,
    left: Vec<usize>,
    right: Vec<usize>,
    pending: Vec<Run>,
    min_gallop: isize,
    error: PhantomData<fn() -> E>,
}

impl<K, E, F> SortState<'_, K, E, F>
where
    F: Fn(&K, &K) -> Result<bool, E>,
{
    /// One Python `<` comparison between two keys.
    fn lt(&self, left: usize, right: usize) -> Result<bool, E> {
        (self.less)(&self.keys[left], &self.keys[right])
    }

    fn run(&mut self) -> Result<(), E> {
        let length = self.keys.len();
        if length < 2 {
            return Ok(());
        }
        let minrun = merge_compute_minrun(length);
        let mut lo = 0usize;
        let mut remaining = length;
        loop {
            let (mut run_len, descending) = self.count_run(lo, lo + remaining)?;
            if descending {
                self.order[lo..lo + run_len].reverse();
            }
            if run_len < minrun {
                let force = remaining.min(minrun);
                self.binarysort(lo, lo + force, lo + run_len)?;
                run_len = force;
            }
            self.found_new_run(run_len)?;
            self.pending.push(Run {
                start: lo,
                len: run_len,
                power: 0,
            });
            lo += run_len;
            remaining -= run_len;
            if remaining == 0 {
                break;
            }
        }
        self.merge_force_collapse()
    }

    /// Length of the run starting at `lo`, and whether it is strictly descending.
    fn count_run(&self, lo: usize, hi: usize) -> Result<(usize, bool), E> {
        let mut cursor = lo + 1;
        if cursor == hi {
            return Ok((1, false));
        }
        let mut length = 2usize;
        if self.lt(cursor, cursor - 1)? {
            cursor += 1;
            while cursor < hi {
                if !self.lt(cursor, cursor - 1)? {
                    break;
                }
                cursor += 1;
                length += 1;
            }
            Ok((length, true))
        } else {
            cursor += 1;
            while cursor < hi {
                if self.lt(cursor, cursor - 1)? {
                    break;
                }
                cursor += 1;
                length += 1;
            }
            Ok((length, false))
        }
    }

    /// Stable binary insertion of `[lo, start)`'s following elements into it.
    fn binarysort(&mut self, lo: usize, hi: usize, start: usize) -> Result<(), E> {
        let mut start = start;
        if lo == start {
            start += 1;
        }
        while start < hi {
            let mut left = lo;
            let mut right = start;
            let pivot = self.order[start];
            while left < right {
                let middle = left + ((right - left) >> 1);
                if self.lt(pivot, self.order[middle])? {
                    right = middle;
                } else {
                    left = middle + 1;
                }
            }
            self.order[left..=start].rotate_right(1);
            start += 1;
        }
        Ok(())
    }

    /// Merge run `i` with run `i + 1` after trimming already-ordered prefixes.
    fn merge_at(&mut self, i: usize) -> Result<(), E> {
        let a_start = self.pending[i].start;
        let mut na = self.pending[i].len;
        let b_start = self.pending[i + 1].start;
        let mut nb = self.pending[i + 1].len;
        self.pending[i].len = na + nb;
        if i + 3 == self.pending.len() {
            let last = self.pending.len() - 1;
            self.pending[i + 1] = self.pending[last];
        }
        self.pending.pop();

        // Where does B start in A?  A's prefix before it is already in place.
        let k = self.gallop_right(
            self.order[b_start],
            &self.order[a_start..a_start + na],
            na,
            0,
        )?;
        let a_start = a_start + k;
        na -= k;
        if na == 0 {
            return Ok(());
        }
        // Where does A end in B?  B's suffix after it is already in place.
        let k = self.gallop_left(
            self.order[a_start + na - 1],
            &self.order[b_start..b_start + nb],
            nb,
            nb - 1,
        )?;
        nb = k;
        if nb == 0 {
            return Ok(());
        }
        if na <= nb {
            self.merge_lo(a_start, na, b_start, nb)?;
        } else {
            self.merge_hi(a_start, na, b_start, nb)?;
        }
        #[cfg(debug_assertions)]
        self.debug_check_order();
        Ok(())
    }

    #[cfg(debug_assertions)]
    fn debug_check_order(&self) {
        let mut seen = vec![false; self.order.len()];
        for (position, index) in self.order.iter().enumerate() {
            if *index >= seen.len() || seen[*index] {
                panic!("sort engine produced a non-permutation at {position}: {index}");
            }
            seen[*index] = true;
        }
    }

    /// Merge adjacent runs forward from scratch copies of both runs.
    fn merge_lo(
        &mut self,
        a_start: usize,
        mut na: usize,
        b_start: usize,
        mut nb: usize,
    ) -> Result<(), E> {
        self.left.clear();
        self.left
            .extend_from_slice(&self.order[a_start..a_start + na]);
        self.right.clear();
        self.right
            .extend_from_slice(&self.order[b_start..b_start + nb]);
        let mut ai = 0usize;
        let mut bi = 0usize;
        let mut dest = a_start;

        self.order[dest] = self.right[bi];
        dest += 1;
        bi += 1;
        nb -= 1;
        if nb == 0 {
            self.copy_remaining_a(dest, ai, na);
            return Ok(());
        }
        if na == 1 {
            self.copy_b(dest, bi, nb, self.left[ai]);
            return Ok(());
        }

        let mut min_gallop = self.min_gallop;
        loop {
            let mut acount = 0isize;
            let mut bcount = 0isize;
            loop {
                if self.lt(self.right[bi], self.left[ai])? {
                    self.order[dest] = self.right[bi];
                    dest += 1;
                    bi += 1;
                    bcount += 1;
                    acount = 0;
                    nb -= 1;
                    if nb == 0 {
                        self.copy_remaining_a(dest, ai, na);
                        return Ok(());
                    }
                    if bcount >= min_gallop {
                        break;
                    }
                } else {
                    self.order[dest] = self.left[ai];
                    dest += 1;
                    ai += 1;
                    acount += 1;
                    bcount = 0;
                    na -= 1;
                    if na == 1 {
                        self.copy_b(dest, bi, nb, self.left[ai]);
                        return Ok(());
                    }
                    if acount >= min_gallop {
                        break;
                    }
                }
            }

            min_gallop += 1;
            loop {
                if min_gallop > 1 {
                    min_gallop -= 1;
                }
                self.min_gallop = min_gallop;
                let k = self.gallop_right(self.right[bi], &self.left[ai..ai + na], na, 0)?;
                acount = k as isize;
                if k > 0 {
                    self.order[dest..dest + k].copy_from_slice(&self.left[ai..ai + k]);
                    dest += k;
                    ai += k;
                    na -= k;
                    if na == 1 {
                        self.copy_b(dest, bi, nb, self.left[ai]);
                        return Ok(());
                    }
                    if na == 0 {
                        return Ok(());
                    }
                }
                self.order[dest] = self.right[bi];
                dest += 1;
                bi += 1;
                nb -= 1;
                if nb == 0 {
                    self.copy_remaining_a(dest, ai, na);
                    return Ok(());
                }

                let k = self.gallop_left(self.left[ai], &self.right[bi..bi + nb], nb, 0)?;
                bcount = k as isize;
                if k > 0 {
                    self.order[dest..dest + k].copy_from_slice(&self.right[bi..bi + k]);
                    dest += k;
                    bi += k;
                    nb -= k;
                    if nb == 0 {
                        self.copy_remaining_a(dest, ai, na);
                        return Ok(());
                    }
                }
                self.order[dest] = self.left[ai];
                dest += 1;
                ai += 1;
                na -= 1;
                if na == 1 {
                    self.copy_b(dest, bi, nb, self.left[ai]);
                    return Ok(());
                }
                if acount >= MIN_GALLOP || bcount >= MIN_GALLOP {
                    continue;
                }
                min_gallop += 1;
                self.min_gallop = min_gallop;
                break;
            }
        }
    }

    /// Merge adjacent runs backward from scratch copies of both runs.
    fn merge_hi(
        &mut self,
        a_start: usize,
        mut na: usize,
        b_start: usize,
        mut nb: usize,
    ) -> Result<(), E> {
        self.left.clear();
        self.left
            .extend_from_slice(&self.order[a_start..a_start + na]);
        self.right.clear();
        self.right
            .extend_from_slice(&self.order[b_start..b_start + nb]);
        let mut dest = b_start + nb - 1;
        let mut ai = na - 1;
        let mut bj = nb - 1;

        self.order[dest] = self.left[ai];
        dest -= 1;
        na -= 1;
        if na == 0 {
            self.copy_remaining_b(dest, nb);
            return Ok(());
        }
        ai -= 1;
        if nb == 1 {
            self.copy_a(dest, ai, na, bj);
            return Ok(());
        }

        let mut min_gallop = self.min_gallop;
        loop {
            let mut acount = 0isize;
            let mut bcount = 0isize;
            loop {
                if self.lt(self.right[bj], self.left[ai])? {
                    self.order[dest] = self.left[ai];
                    dest -= 1;
                    na -= 1;
                    acount += 1;
                    bcount = 0;
                    if na == 0 {
                        self.copy_remaining_b(dest, nb);
                        return Ok(());
                    }
                    ai -= 1;
                    if acount >= min_gallop {
                        break;
                    }
                } else {
                    self.order[dest] = self.right[bj];
                    dest -= 1;
                    bj -= 1;
                    bcount += 1;
                    acount = 0;
                    nb -= 1;
                    if nb == 1 {
                        self.copy_a(dest, ai, na, bj);
                        return Ok(());
                    }
                    if bcount >= min_gallop {
                        break;
                    }
                }
            }

            min_gallop += 1;
            loop {
                if min_gallop > 1 {
                    min_gallop -= 1;
                }
                self.min_gallop = min_gallop;
                let k = self.gallop_right(self.right[bj], &self.left[0..na], na, na - 1)?;
                let k = na - k;
                acount = k as isize;
                if k > 0 {
                    let source = ai + 1 - k;
                    let target = dest + 1 - k;
                    self.order[target..target + k].copy_from_slice(&self.left[source..source + k]);
                    dest -= k;
                    na -= k;
                    if na == 0 {
                        self.copy_remaining_b(dest, nb);
                        return Ok(());
                    }
                    ai -= k;
                }
                self.order[dest] = self.right[bj];
                dest -= 1;
                bj -= 1;
                nb -= 1;
                if nb == 1 {
                    self.copy_a(dest, ai, na, bj);
                    return Ok(());
                }

                let position = self.gallop_left(self.left[ai], &self.right[0..nb], nb, nb - 1)?;
                let k = nb - position;
                bcount = k as isize;
                if k > 0 {
                    let source = bj + 1 - k;
                    let target = dest + 1 - k;
                    self.order[target..target + k].copy_from_slice(&self.right[source..source + k]);
                    dest -= k;
                    nb -= k;
                    if nb == 1 {
                        self.copy_a(dest, ai, na, 0);
                        return Ok(());
                    }
                    if nb == 0 {
                        self.copy_remaining_a_hi(dest, ai, na);
                        return Ok(());
                    }
                    bj = nb - 1;
                }
                self.order[dest] = self.left[ai];
                dest -= 1;
                na -= 1;
                if na == 0 {
                    self.copy_remaining_b(dest, nb);
                    return Ok(());
                }
                ai -= 1;
                if acount >= MIN_GALLOP || bcount >= MIN_GALLOP {
                    continue;
                }
                min_gallop += 1;
                self.min_gallop = min_gallop;
                break;
            }
        }
    }

    /// Finish a low merge whose A run has one element left.
    fn copy_b(&mut self, dest: usize, bi: usize, nb: usize, a_last: usize) {
        self.order[dest..dest + nb].copy_from_slice(&self.right[bi..bi + nb]);
        self.order[dest + nb] = a_last;
    }

    /// Finish a high merge whose B run has one element left.
    fn copy_a(&mut self, dest: usize, ai: usize, na: usize, bj: usize) {
        let source = ai + 1 - na;
        let target = dest + 1 - na;
        self.order[target..target + na].copy_from_slice(&self.left[source..source + na]);
        self.order[target - 1] = self.right[bj];
    }

    /// Scratch B is not in place here: copy its remaining prefix down.
    fn copy_remaining_b(&mut self, dest: usize, nb: usize) {
        let target = dest + 1 - nb;
        self.order[target..target + nb].copy_from_slice(&self.right[0..nb]);
    }

    /// Scratch A is not in place here: copy its remaining prefix down.
    fn copy_remaining_a_hi(&mut self, dest: usize, ai: usize, na: usize) {
        let source = ai + 1 - na;
        let target = dest + 1 - na;
        self.order[target..target + na].copy_from_slice(&self.left[source..source + na]);
    }

    /// Finish a low merge whose B run is exhausted.
    fn copy_remaining_a(&mut self, dest: usize, ai: usize, na: usize) {
        self.order[dest..dest + na].copy_from_slice(&self.left[ai..ai + na]);
    }

    /// Locate `key` at or left of the first equal slot in a sorted run.
    fn gallop_left(&self, key: usize, a: &[usize], n: usize, hint: usize) -> Result<usize, E> {
        let mut lastofs = 0isize;
        let mut ofs = 1isize;
        if self.lt(a[hint], key)? {
            let maxofs = (n - hint) as isize;
            while ofs < maxofs {
                if self.lt(a[(hint as isize + ofs) as usize], key)? {
                    lastofs = ofs;
                    ofs = (ofs << 1) + 1;
                } else {
                    break;
                }
            }
            if ofs > maxofs {
                ofs = maxofs;
            }
            lastofs += hint as isize;
            ofs += hint as isize;
        } else {
            let maxofs = (hint + 1) as isize;
            while ofs < maxofs {
                if self.lt(a[(hint as isize - ofs) as usize], key)? {
                    break;
                }
                lastofs = ofs;
                ofs = (ofs << 1) + 1;
            }
            if ofs > maxofs {
                ofs = maxofs;
            }
            let k = lastofs;
            lastofs = hint as isize - ofs;
            ofs = hint as isize - k;
        }

        let mut lastofs = lastofs + 1;
        while lastofs < ofs {
            let middle = lastofs + ((ofs - lastofs) >> 1);
            if self.lt(a[middle as usize], key)? {
                lastofs = middle + 1;
            } else {
                ofs = middle;
            }
        }
        Ok(ofs as usize)
    }

    /// Locate `key` at or right of the last equal slot in a sorted run.
    fn gallop_right(&self, key: usize, a: &[usize], n: usize, hint: usize) -> Result<usize, E> {
        let mut lastofs = 0isize;
        let mut ofs = 1isize;
        if self.lt(key, a[hint])? {
            let maxofs = (hint + 1) as isize;
            while ofs < maxofs {
                if self.lt(key, a[(hint as isize - ofs) as usize])? {
                    lastofs = ofs;
                    ofs = (ofs << 1) + 1;
                } else {
                    break;
                }
            }
            if ofs > maxofs {
                ofs = maxofs;
            }
            let k = lastofs;
            lastofs = hint as isize - ofs;
            ofs = hint as isize - k;
        } else {
            let maxofs = (n - hint) as isize;
            while ofs < maxofs {
                if self.lt(key, a[(hint as isize + ofs) as usize])? {
                    break;
                }
                lastofs = ofs;
                ofs = (ofs << 1) + 1;
            }
            if ofs > maxofs {
                ofs = maxofs;
            }
            lastofs += hint as isize;
            ofs += hint as isize;
        }

        let mut lastofs = lastofs + 1;
        while lastofs < ofs {
            let middle = lastofs + ((ofs - lastofs) >> 1);
            if self.lt(key, a[middle as usize])? {
                ofs = middle;
            } else {
                lastofs = middle + 1;
            }
        }
        Ok(ofs as usize)
    }

    /// Powersort: merge stack runs while the saved power is larger.
    fn found_new_run(&mut self, next_len: usize) -> Result<(), E> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let last = self.pending.len() - 1;
        let start = self.pending[last].start;
        let len = self.pending[last].len;
        let power = powerloop(start, len, next_len, self.keys.len());
        while self.pending.len() > 1 && self.pending[self.pending.len() - 2].power > power {
            let index = self.pending.len() - 2;
            self.merge_at(index)?;
        }
        let last = self.pending.len() - 1;
        self.pending[last].power = power;
        Ok(())
    }

    /// Merge every remaining run into one.
    fn merge_force_collapse(&mut self) -> Result<(), E> {
        while self.pending.len() > 1 {
            let mut index = self.pending.len() - 2;
            if index > 0 && self.pending[index - 1].len < self.pending[index + 1].len {
                index -= 1;
            }
            self.merge_at(index)?;
        }
        Ok(())
    }
}

/// Sort indices with a fallible predicate, returning the stable permutation.
///
/// The comparison log and any refusal are exactly the engine's sequence: the
/// caller's data is never moved before the sort succeeds.
fn sort_indices<K, E, F>(keys: &[K], less: F) -> Result<Vec<usize>, E>
where
    F: Fn(&K, &K) -> Result<bool, E>,
{
    let mut state = SortState {
        keys,
        less,
        order: (0..keys.len()).collect(),
        left: Vec::new(),
        right: Vec::new(),
        pending: Vec::new(),
        min_gallop: MIN_GALLOP,
        error: PhantomData,
    };
    state.run()?;
    Ok(state.order)
}

/// The one stable keyed sequence facade used by the Source fold and its peers.
///
/// Keys are computed exactly once per item. The ordering predicate is Python's
/// tuple `<` from [`super::order`]; an incomparable pair surfaces the reference's
/// `TypeError` and leaves `items` untouched, matching `sorted()`'s contract
/// (the input iterable is not mutated when the key comparison raises).
pub(super) fn python_json_stable_sort_by_key<T>(
    items: &mut Vec<T>,
    key: impl Fn(&T) -> Vec<PythonJsonValue>,
) -> Result<(), PythonJsonCompareError> {
    let keys: Vec<Vec<PythonJsonValue>> = items.iter().map(&key).collect();
    if keys.len() < 2 {
        return Ok(());
    }
    let order = sort_indices(&keys, |left, right| python_json_tuple_lt(left, right))?;
    let mut slots: Vec<Option<T>> = std::mem::take(items).into_iter().map(Some).collect();
    let mut ordered = Vec::with_capacity(slots.len());
    for index in order {
        ordered.push(
            slots[index]
                .take()
                .expect("sort order is a permutation of the input"),
        );
    }
    *items = ordered;
    Ok(())
}

/// CPython's minimum natural run length for an input of `n` elements.
fn merge_compute_minrun(n: usize) -> usize {
    let mut n = n;
    let mut r = 0usize;
    while n >= MIN_RUN {
        r |= n & 1;
        n >>= 1;
    }
    n + r
}

/// Powersort node depth for two adjacent runs inside a list of `length` items.
fn powerloop(start: usize, left: usize, right: usize, length: usize) -> usize {
    let length = length as u128;
    let mut a = 2 * start as u128 + left as u128;
    let mut b = a + left as u128 + right as u128;
    let mut power = 0usize;
    loop {
        power += 1;
        if a >= length {
            a -= length;
            b -= length;
        } else if b >= length {
            break;
        }
        a <<= 1;
        b <<= 1;
    }
    power
}

#[cfg(test)]
mod tests {
    use super::super::parse;
    use super::*;
    use std::cell::RefCell;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn oracle_root(repo: &Path) -> PathBuf {
        let python = std::env::var_os("SK_KERNEL_PYTHON")
            .map(PathBuf::from)
            .unwrap_or_else(|| repo.join(".venv-rust-oracle/bin/python"));
        let verified = Command::new(&python)
            .arg(repo.join("scripts/rust/verify_oracle.py"))
            .arg("--root")
            .arg(repo)
            .output()
            .expect("run required oracle verification");
        assert!(
            verified.status.success(),
            "required Python oracle is not lock-exact: {}",
            String::from_utf8_lossy(&verified.stderr)
        );
        python
    }

    fn roots() -> [(&'static str, &'static str); 9] {
        [
            ("HOME", "home"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_CACHE_HOME", "cache"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_STATE_HOME", "state"),
            ("XDG_RUNTIME_DIR", "runtime"),
            ("TMPDIR", "tmp"),
            ("TMP", "tmp"),
            ("TEMP", "tmp"),
        ]
    }

    fn child_command(root: &Path, repo: &Path, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(program);
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .current_dir(root)
            .env("SK_CF60_ROOT", root)
            .env("SK_CF60_REPO", repo)
            .env("PYTHONPATH", repo.join("src"))
            .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
            .env("PYTHONHASHSEED", "0")
            .env("PYTHONDONTWRITEBYTECODE", "1");
        for (key, name) in roots() {
            command.env(key, root.join(name));
        }
        command
    }

    fn assert_isolated_child(root: &Path) {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("repository root");
        assert_eq!(std::env::current_dir().expect("cwd"), root);
        for (key, name) in roots() {
            assert_eq!(
                std::env::var_os(key).map(PathBuf::from),
                Some(root.join(name)),
                "{key}"
            );
        }
        assert_eq!(
            std::env::var_os("SK_CF60_REPO").map(PathBuf::from),
            Some(repo.clone())
        );
        let python_path = PathBuf::from(std::env::var_os("PYTHONPATH").expect("PYTHONPATH"));
        assert!(python_path.is_absolute());
        assert_eq!(python_path, repo.join("src"));
        let packs = PathBuf::from(
            std::env::var_os("SPEC_KITTY_PACKS_ROOT").expect("SPEC_KITTY_PACKS_ROOT"),
        );
        assert!(packs.is_absolute());
        assert_eq!(packs, repo.join("packs"));
        assert_eq!(std::env::var("PYTHONHASHSEED").expect("hash seed"), "0");
    }

    fn domain_cases() -> Vec<(String, Vec<Vec<String>>)> {
        let nan = || "NaN".to_owned();
        let mut cases = vec![
            (
                "parent_short".to_owned(),
                vec![
                    vec!["3".to_owned()],
                    vec!["2".to_owned()],
                    vec![nan()],
                    vec!["1".to_owned()],
                ],
            ),
            (
                "nan_first".to_owned(),
                vec![
                    vec![nan()],
                    vec!["3".to_owned()],
                    vec!["2".to_owned()],
                    vec!["1".to_owned()],
                ],
            ),
            (
                "nan_middle".to_owned(),
                vec![
                    vec!["3".to_owned()],
                    vec![nan()],
                    vec!["2".to_owned()],
                    vec!["1".to_owned()],
                ],
            ),
            (
                "two_nans".to_owned(),
                vec![
                    vec![nan()],
                    vec!["2".to_owned()],
                    vec![nan()],
                    vec!["1".to_owned()],
                    vec![nan()],
                ],
            ),
            (
                "stable_equal".to_owned(),
                vec![
                    vec!["1".to_owned()],
                    vec!["1".to_owned()],
                    vec!["0".to_owned()],
                    vec!["1".to_owned()],
                    vec!["0".to_owned()],
                    vec!["1".to_owned()],
                ],
            ),
            (
                "array_vs_int".to_owned(),
                vec![vec!["[1]".to_owned()], vec!["1".to_owned()]],
            ),
            (
                "int_vs_str".to_owned(),
                vec![vec!["1".to_owned()], vec!["\"a\"".to_owned()]],
            ),
            (
                "str_vs_int".to_owned(),
                vec![vec!["\"a\"".to_owned()], vec!["1".to_owned()]],
            ),
            (
                "tuple_second_elem".to_owned(),
                vec![
                    vec!["1".to_owned(), "\"a\"".to_owned()],
                    vec!["1".to_owned(), "2".to_owned()],
                ],
            ),
            (
                "nan_tuple_vs_finite".to_owned(),
                vec![
                    vec![nan(), "1".to_owned()],
                    vec![nan(), "0".to_owned()],
                    vec!["1".to_owned(), "2".to_owned()],
                ],
            ),
            (
                "nan_last".to_owned(),
                vec![
                    vec!["3".to_owned()],
                    vec!["2".to_owned()],
                    vec!["1".to_owned()],
                    vec![nan()],
                ],
            ),
            (
                "infinity".to_owned(),
                vec![
                    vec!["Infinity".to_owned()],
                    vec!["2".to_owned()],
                    vec!["-Infinity".to_owned()],
                    vec!["1".to_owned()],
                ],
            ),
            (
                "bool_int_equal".to_owned(),
                vec![
                    vec!["true".to_owned()],
                    vec!["1".to_owned()],
                    vec!["false".to_owned()],
                    vec!["0".to_owned()],
                    vec!["1.0".to_owned()],
                ],
            ),
            (
                "big_ints".to_owned(),
                vec![
                    vec!["9007199254740993".to_owned()],
                    vec!["9007199254740992".to_owned()],
                    vec!["1".to_owned()],
                    vec!["-1".to_owned()],
                ],
            ),
            (
                "floats".to_owned(),
                vec![
                    vec!["1.5".to_owned()],
                    vec!["1.0".to_owned()],
                    vec!["-0.0".to_owned()],
                    vec!["0.0".to_owned()],
                    vec!["1e308".to_owned()],
                ],
            ),
            (
                "strings".to_owned(),
                vec![
                    vec!["\"10\"".to_owned()],
                    vec!["\"2\"".to_owned()],
                    vec!["\"a\"".to_owned()],
                    vec!["\"\u{00e9}\"".to_owned()],
                    vec!["\"z\"".to_owned()],
                ],
            ),
            (
                "nested_arrays".to_owned(),
                vec![
                    vec!["[1,2]".to_owned()],
                    vec!["[1]".to_owned()],
                    vec!["[1,2,3]".to_owned()],
                    vec!["[]".to_owned()],
                    vec!["[0,9]".to_owned()],
                ],
            ),
            (
                "tuple_second_array".to_owned(),
                vec![
                    vec!["1".to_owned(), "[2]".to_owned()],
                    vec!["1".to_owned(), "\"a\"".to_owned()],
                ],
            ),
        ];
        for n in (0usize..=5).chain([7, 16, 31, 32, 33, 63, 64, 65, 100, 129]) {
            cases.push((
                format!("ascending_{n}"),
                (0..n).map(|index| vec![index.to_string()]).collect(),
            ));
            cases.push((
                format!("descending_{n}"),
                (0..n).rev().map(|index| vec![index.to_string()]).collect(),
            ));
            if n > 0 {
                cases.push((
                    format!("nan_mid_{n}"),
                    (0..n)
                        .map(|index| {
                            if index == n / 2 {
                                vec![nan()]
                            } else {
                                vec![index.to_string()]
                            }
                        })
                        .collect(),
                ));
            }
            if n >= 8 {
                cases.push((
                    format!("ties_{n}"),
                    (0..n)
                        .map(|index| vec![((index * 37) % 17).to_string()])
                        .collect(),
                ));
                let mut tail: Vec<Vec<String>> = (0..n - 1)
                    .map(|index| vec![(2 * index).to_string()])
                    .collect();
                tail.push(vec!["1".to_owned()]);
                cases.push((format!("gallop_tail_{n}"), tail));
                let mut high: Vec<Vec<String>> = (0..n - 1)
                    .map(|index| vec![(2 * index).to_string()])
                    .collect();
                high.push(vec![(2 * n).to_string()]);
                cases.push((format!("gallop_hi_{n}"), high));
            }
        }
        cases
    }

    fn load_keys(rows: &[Vec<String>]) -> Vec<Vec<PythonJsonValue>> {
        rows.iter()
            .map(|row| {
                row.iter()
                    .map(|token| parse(token).expect("case token"))
                    .collect()
            })
            .collect()
    }

    fn render_value(value: &PythonJsonValue) -> String {
        super::super::python_repr_value(value)
    }

    fn render_key(key: &[PythonJsonValue]) -> String {
        let parts: Vec<String> = key.iter().map(render_value).collect();
        match parts.as_slice() {
            [] => "()".to_owned(),
            [single] => format!("({single},)"),
            _ => format!("({})", parts.join(", ")),
        }
    }

    #[test]
    fn parent_short_sequence_matches_the_recorded_source_order() {
        let keys = load_keys(&[
            vec!["3".to_owned()],
            vec!["2".to_owned()],
            vec!["NaN".to_owned()],
            vec!["1".to_owned()],
        ]);
        let mut indices: Vec<usize> = (0..keys.len()).collect();
        python_json_stable_sort_by_key(&mut indices, |index| keys[*index].clone())
            .expect("NaN tuples do not raise");
        assert_eq!(indices, vec![3, 1, 0, 2]);
        let rendered: Vec<String> = indices
            .iter()
            .map(|index| render_key(&keys[*index]))
            .collect();
        assert_eq!(
            rendered,
            vec!["(1,)", "(2,)", "(3,)", "(nan,)"],
            "recorded Source .vaGKB6 order"
        );
    }

    #[test]
    fn stable_sort_keeps_equal_keys_in_input_order() {
        let keys = load_keys(&[
            vec!["1".to_owned()],
            vec!["1".to_owned()],
            vec!["0".to_owned()],
            vec!["1".to_owned()],
            vec!["0".to_owned()],
            vec!["1".to_owned()],
        ]);
        let mut indices: Vec<usize> = (0..keys.len()).collect();
        python_json_stable_sort_by_key(&mut indices, |index| keys[*index].clone())
            .expect("equal keys order");
        assert_eq!(indices, vec![2, 4, 0, 1, 3, 5]);
    }

    #[test]
    fn heterogeneous_refusal_keeps_every_item() {
        let keys = load_keys(&[vec!["1".to_owned()], vec!["\"a\"".to_owned()]]);
        let mut indices: Vec<usize> = (0..keys.len()).collect();
        let error = python_json_stable_sort_by_key(&mut indices, |index| keys[*index].clone())
            .expect_err("mixed ranking raises");
        assert_eq!(
            error.message(),
            "'<' not supported between instances of 'str' and 'int'"
        );
        assert_eq!(indices, vec![0, 1], "items are preserved on refusal");
        assert_eq!(error.to_string(), error.message());
    }

    #[test]
    fn keys_are_computed_once_and_items_survive_a_refusal() {
        let calls = std::cell::Cell::new(0usize);
        let mut items: Vec<String> = vec!["c".to_owned(), "a".to_owned(), "b".to_owned()];
        python_json_stable_sort_by_key(&mut items, |item| {
            calls.set(calls.get() + 1);
            vec![PythonJsonValue::scalar(item)]
        })
        .expect("strings order");
        assert_eq!(calls.get(), 3, "exactly one key per item");
        assert_eq!(items, vec!["a", "b", "c"]);

        let calls = std::cell::Cell::new(0usize);
        let mut mixed: Vec<String> = vec!["1".to_owned(), "\"a\"".to_owned(), "2".to_owned()];
        let original = mixed.clone();
        let error = python_json_stable_sort_by_key(&mut mixed, |item| {
            calls.set(calls.get() + 1);
            vec![parse(item).expect("numeric or string token")]
        })
        .expect_err("int/str ranking raises");
        assert_eq!(
            error.message(),
            "'<' not supported between instances of 'str' and 'int'"
        );
        assert_eq!(calls.get(), 3, "exactly one key per item even on refusal");
        assert_eq!(
            mixed, original,
            "ownership and order are preserved on refusal"
        );
    }

    #[test]
    fn sequence_engine_matches_actual_source_comparison_order() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("repository root");
        if let Some(root) = std::env::var_os("SK_CF60_NATIVE") {
            let root = PathBuf::from(root);
            assert_isolated_child(&root);
            let output = run_native_sequence(&root);
            fs::write(root.join("native.json"), output.to_string()).expect("write native receipt");
            return;
        }

        let root = tempfile::tempdir().expect("evidence root").keep();
        for (_, name) in roots() {
            fs::create_dir_all(root.join(name)).expect("isolated root");
        }
        write_cases(&root);
        eprintln!("cf60 sequence evidence: {}", root.display());
        let python = oracle_root(&repo);
        let source = child_command(&root, &repo, python)
            .args(["-c", SOURCE_SEQUENCE_SCRIPT])
            .output()
            .expect("run Source child");
        fs::write(root.join("source.stdout"), &source.stdout).expect("source stdout");
        fs::write(root.join("source.stderr"), &source.stderr).expect("source stderr");
        assert!(
            source.status.success(),
            "Source FIRST: {}",
            String::from_utf8_lossy(&source.stderr)
        );
        let source: serde_json::Value =
            serde_json::from_slice(&source.stdout).expect("Source receipt is JSON");

        let native = child_command(&root, &repo, std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "python_json::sort::tests::sequence_engine_matches_actual_source_comparison_order",
                "--nocapture",
            ])
            .env("SK_CF60_NATIVE", &root)
            .output()
            .expect("run Native child");
        fs::write(root.join("native.stdout"), &native.stdout).expect("native stdout");
        fs::write(root.join("native.stderr"), &native.stderr).expect("native stderr");
        assert!(
            native.status.success(),
            "Native: {} {}",
            String::from_utf8_lossy(&native.stdout),
            String::from_utf8_lossy(&native.stderr)
        );
        let native: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("native.json")).expect("native receipt"))
                .expect("Native receipt is JSON");
        assert_eq!(source["python"], "3.11.15");
        assert_eq!(
            source["sequence"], native["sequence"],
            "comparison logs, orders and refusal pairs must match the reference exactly"
        );
        assert_eq!(
            source["domain"], native["domain"],
            "sorted() index permutations / errors"
        );
        let source_sequence = source["sequence"].as_array().expect("sequence array");
        let source_domain = source["domain"].as_array().expect("domain array");
        assert!(source_sequence.len() >= 100, "sequence controls");
        assert!(source_domain.len() >= 80, "domain controls");
        assert!(
            source_sequence.iter().any(|case| case["error"].is_string()),
            "at least one refusal control"
        );
        assert!(
            source_sequence
                .iter()
                .any(|case| case["log"].as_array().is_some_and(|log| log.len() > 100)),
            "at least one run/merge/gallop control"
        );
    }

    fn native_sequence_cases() -> Vec<(String, Vec<String>, Vec<(usize, usize)>)> {
        let mut random = Random(6153);
        let mut cases = Vec::new();
        for n in [0usize, 1, 2, 3, 4, 5, 7, 8, 20, 63, 64, 65, 100, 129, 300] {
            cases.push((
                format!("asc_{n}"),
                (0..n).map(|index| index.to_string()).collect(),
                Vec::new(),
            ));
            cases.push((
                format!("desc_{n}"),
                (0..n).rev().map(|index| index.to_string()).collect(),
                Vec::new(),
            ));
            cases.push((
                format!("ties_{n}"),
                (0..n).map(|_| random.next(5).to_string()).collect(),
                Vec::new(),
            ));
            cases.push((
                format!("nan_{n}"),
                (0..n)
                    .map(|index| {
                        if index % 7 == 3 {
                            "NaN".to_owned()
                        } else {
                            random.next(9).to_string()
                        }
                    })
                    .collect(),
                Vec::new(),
            ));
            let organ: Vec<String> = (0..n / 2)
                .chain((0..=n / 2).rev())
                .take(n)
                .map(|value| value.to_string())
                .collect();
            cases.push((format!("organ_{n}"), organ, Vec::new()));
        }
        let bases = cases.clone();
        for (name, values, _) in bases {
            if values.len() < 12 {
                continue;
            }
            let mut pairs = Vec::new();
            for offset in [2usize, 5, 9] {
                if values.len() > offset + 3 {
                    pairs.push((offset, offset + 3));
                    pairs.push((offset + 3, offset));
                }
            }
            cases.push((format!("error_{name}"), values, pairs));
        }
        cases
    }

    fn write_cases(root: &Path) {
        let sequence: Vec<serde_json::Value> = native_sequence_cases()
            .iter()
            .map(|(name, values, pairs)| {
                serde_json::json!({"name": name, "values": values, "error_pairs": pairs})
            })
            .collect();
        let domain: Vec<serde_json::Value> = domain_cases()
            .iter()
            .map(|(name, rows)| serde_json::json!({"name": name, "rows": rows}))
            .collect();
        fs::write(
            root.join("cases.json"),
            serde_json::json!({"sequence": sequence, "domain": domain}).to_string(),
        )
        .expect("write shared cases");
    }

    fn load_sequence_cases(root: &Path) -> Vec<(String, Vec<String>, Vec<(usize, usize)>)> {
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("cases.json")).expect("cases.json"))
                .expect("cases JSON");
        value["sequence"]
            .as_array()
            .expect("sequence cases")
            .iter()
            .map(|case| {
                let name = case["name"].as_str().expect("case name").to_owned();
                let values = case["values"]
                    .as_array()
                    .expect("case values")
                    .iter()
                    .map(|value| value.as_str().expect("value token").to_owned())
                    .collect();
                let pairs = case["error_pairs"]
                    .as_array()
                    .expect("error pairs")
                    .iter()
                    .map(|pair| {
                        (
                            pair[0].as_u64().expect("left id") as usize,
                            pair[1].as_u64().expect("right id") as usize,
                        )
                    })
                    .collect();
                (name, values, pairs)
            })
            .collect()
    }

    fn load_domain_cases(root: &Path) -> Vec<(String, Vec<Vec<String>>)> {
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("cases.json")).expect("cases.json"))
                .expect("cases JSON");
        value["domain"]
            .as_array()
            .expect("domain cases")
            .iter()
            .map(|case| {
                let name = case["name"].as_str().expect("case name").to_owned();
                let rows = case["rows"]
                    .as_array()
                    .expect("case rows")
                    .iter()
                    .map(|row| {
                        row.as_array()
                            .expect("row tokens")
                            .iter()
                            .map(|token| token.as_str().expect("token").to_owned())
                            .collect()
                    })
                    .collect();
                (name, rows)
            })
            .collect()
    }

    /// Small deterministic LCG so both receipts generate identical values.
    struct Random(u64);

    impl Random {
        fn next(&mut self, modulo: u64) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) % modulo
        }
    }

    struct Recorded {
        id: usize,
        value: OrdValue,
    }

    #[derive(Clone, Copy)]
    enum OrdValue {
        Finite(f64),
        Nan,
    }

    impl OrdValue {
        fn less(self, other: Self) -> bool {
            match (self, other) {
                (Self::Finite(left), Self::Finite(right)) => left < right,
                _ => false,
            }
        }
    }

    fn run_native_sequence(root: &Path) -> serde_json::Value {
        let cases = load_sequence_cases(root);
        let mut out = Vec::new();
        for (name, values, error_pairs) in &cases {
            let log = RefCell::new(Vec::<(usize, usize)>::new());
            let keys: Vec<Recorded> = values
                .iter()
                .enumerate()
                .map(|(id, token)| Recorded {
                    id,
                    value: if token == "NaN" {
                        OrdValue::Nan
                    } else {
                        OrdValue::Finite(token.parse::<f64>().expect("finite value"))
                    },
                })
                .collect();
            let result = sort_indices(&keys, |left, right| {
                log.borrow_mut().push((left.id, right.id));
                if error_pairs.contains(&(left.id, right.id)) {
                    return Err(format!("err {} {}", left.id, right.id));
                }
                Ok(left.value.less(right.value))
            });
            let (order, error) = match result {
                Ok(order) => (
                    Some(
                        order
                            .iter()
                            .map(|index| keys[*index].id)
                            .collect::<Vec<_>>(),
                    ),
                    serde_json::Value::Null,
                ),
                Err(message) => (None, serde_json::Value::String(message)),
            };
            out.push(serde_json::json!({
                "name": name,
                "order": order,
                "log": log.into_inner(),
                "error": error,
            }));
        }
        let mut domain = Vec::new();
        for (name, rows) in load_domain_cases(root) {
            let keys = load_keys(&rows);
            let mut indices: Vec<usize> = (0..keys.len()).collect();
            let outcome =
                python_json_stable_sort_by_key(&mut indices, |index| keys[*index].clone());
            match outcome {
                Ok(()) => domain.push(serde_json::json!({"name": name, "order": indices})),
                Err(error) => domain.push(serde_json::json!({
                    "name": name,
                    "error": error.message(),
                })),
            }
        }
        serde_json::json!({"sequence": out, "domain": domain})
    }

    /// Emitted verbatim into the Source child; it asserts its own pre-work env.
    const SOURCE_SEQUENCE_SCRIPT: &str = r#"
import json, os, random, sys
from pathlib import Path
root = Path(os.environ["SK_CF60_ROOT"])
assert Path.cwd() == root and sys.version_info[:3] == (3, 11, 15)
for key, name in [("HOME","home"),("XDG_CONFIG_HOME","config"),("XDG_CACHE_HOME","cache"),("XDG_DATA_HOME","data"),("XDG_STATE_HOME","state"),("XDG_RUNTIME_DIR","runtime"),("TMPDIR","tmp"),("TMP","tmp"),("TEMP","tmp")]:
    assert Path(os.environ[key]) == root / name, key
assert os.environ["PYTHONHASHSEED"] == "0"
assert Path(os.environ["PYTHONPATH"]).is_absolute() and Path(os.environ["PYTHONPATH"]) == Path(os.environ["SK_CF60_REPO"]) / "src"
assert Path(os.environ["SPEC_KITTY_PACKS_ROOT"]).is_absolute() and Path(os.environ["SPEC_KITTY_PACKS_ROOT"]) == Path(os.environ["SK_CF60_REPO"]) / "packs"
assert json.loads("NaN") is json.loads("NaN")

class Rec:
    log = []
    error_pairs = set()
    def __init__(self, ident, value):
        self.ident = ident
        self.value = value
    def __lt__(self, other):
        type(self).log.append([self.ident, other.ident])
        if (self.ident, other.ident) in type(self).error_pairs:
            raise RuntimeError(f"err {self.ident} {other.ident}")
        return self.value < other.value

data = json.loads((root / "cases.json").read_text())

def run(name, values, pairs):
    Rec.log = []
    Rec.error_pairs = set(tuple(pair) for pair in pairs)
    items = [Rec(i, v) for i, v in enumerate(values)]
    try:
        order = sorted(items)
        return {"name": name, "order": [i.ident for i in order], "log": Rec.log, "error": None}
    except Exception as exc:
        return {"name": name, "order": None, "log": Rec.log, "error": str(exc)}

sequence = []
for case in data["sequence"]:
    values = [float("nan") if value == "NaN" else float(value) for value in case["values"]]
    sequence.append(run(case["name"], values, case["error_pairs"]))

domain = []
for case in data["domain"]:
    keys = [tuple(json.loads(token) for token in row) for row in case["rows"]]
    indices = list(range(len(keys)))
    try:
        domain.append({"name": case["name"], "order": sorted(indices, key=keys.__getitem__)})
    except Exception as exc:
        domain.append({"name": case["name"], "error": str(exc)})

print(json.dumps({
    "python": sys.version.split()[0],
    "sequence": sequence,
    "domain": domain,
}, allow_nan=True))
"#;
}
