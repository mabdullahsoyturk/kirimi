//! Delimiter search for segment bodies.
//!
//! `memchr::memmem` picks two "rare" needle bytes for its SIMD prefilter and
//! switches the prefilter off if it produces too many candidates. For
//! multipart bodies an attacker controls the haystack and can make any fixed
//! byte pair match frequently, which drops `memmem` to its scalar fallback.
//!
//! Instead, this module scans for the leading CR of the delimiter together
//! with its last byte, verifies each candidate inline and never gives up on
//! the SIMD scan. Verification stays linear: a candidate can only take long to
//! verify if the haystack matches a long prefix of the delimiter there, and
//! such prefixes cannot overlap because the delimiter contains a single CR
//! (boundaries must not contain CR).
//!
//! Haystacks too short for the SIMD finder, and platforms without one, use a
//! scalar scan from CR to CR, which is linear for the same reason. It needs no
//! precomputed tables, which keeps parser construction cheap.

use memchr::arch::all::packedpair::Pair;
use memchr::memchr;

pub(crate) struct DelimiterFinder {
    needle: Box<[u8]>,
    simd: Simd,
}

enum Simd {
    #[cfg(target_arch = "x86_64")]
    Avx2(memchr::arch::x86_64::avx2::packedpair::Finder),
    #[cfg(target_arch = "x86_64")]
    Sse2(memchr::arch::x86_64::sse2::packedpair::Finder),
    #[cfg(target_arch = "aarch64")]
    Neon(memchr::arch::aarch64::neon::packedpair::Finder),
    None,
}

impl DelimiterFinder {
    pub(crate) fn new(needle: &[u8]) -> Self {
        debug_assert_eq!(needle.first(), Some(&b'\r'));
        let simd = u8::try_from(needle.len() - 1)
            .ok()
            .and_then(|last| Pair::with_indices(needle, 0, last))
            .map_or(Simd::None, |pair| Simd::new(needle, pair));
        Self {
            needle: needle.into(),
            simd,
        }
    }

    /// Return the start of the first occurrence of the needle in `haystack`.
    #[inline]
    pub(crate) fn find(&self, haystack: &[u8]) -> Option<usize> {
        match &self.simd {
            #[cfg(target_arch = "x86_64")]
            Simd::Avx2(f) if haystack.len() >= f.min_haystack_len() => {
                f.find(haystack, &self.needle)
            }
            #[cfg(target_arch = "x86_64")]
            Simd::Sse2(f) if haystack.len() >= f.min_haystack_len() => {
                f.find(haystack, &self.needle)
            }
            #[cfg(target_arch = "aarch64")]
            Simd::Neon(f) if haystack.len() >= f.min_haystack_len() => {
                f.find(haystack, &self.needle)
            }
            _ => self.find_scalar(haystack),
        }
    }

    fn find_scalar(&self, haystack: &[u8]) -> Option<usize> {
        let mut start = 0;
        while let Some(i) = memchr(b'\r', &haystack[start..]) {
            let pos = start + i;
            let candidate = haystack.get(pos..pos + self.needle.len())?;
            if candidate == &*self.needle {
                return Some(pos);
            }
            start = pos + 1;
        }
        None
    }
}

impl Simd {
    #[allow(unused_variables)]
    fn new(needle: &[u8], pair: Pair) -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            use memchr::arch::x86_64::{avx2, sse2};
            if let Some(f) = avx2::packedpair::Finder::with_pair(needle, pair) {
                return Simd::Avx2(f);
            }
            if let Some(f) = sse2::packedpair::Finder::with_pair(needle, pair) {
                return Simd::Sse2(f);
            }
        }
        #[cfg(target_arch = "aarch64")]
        {
            use memchr::arch::aarch64::neon;
            if let Some(f) = neon::packedpair::Finder::with_pair(needle, pair) {
                return Simd::Neon(f);
            }
        }
        Simd::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_all_positions() {
        let needle = b"\r\n--boundary";
        let finder = DelimiterFinder::new(needle);
        for len in 0..200 {
            for pos in 0..=len {
                let mut haystack = vec![b'x'; len + needle.len()];
                haystack[pos..pos + needle.len()].copy_from_slice(needle);
                assert_eq!(finder.find(&haystack), Some(pos), "len {len} pos {pos}");
                assert_eq!(finder.find(&haystack[..pos + needle.len() - 1]), None);
            }
        }
    }

    #[test]
    fn near_misses() {
        let needle = b"\r\n--boundary";
        let finder = DelimiterFinder::new(needle);
        for i in 1..needle.len() {
            let mut near = needle.to_vec();
            near[i] = b'X';
            let mut haystack = near.repeat(100);
            assert_eq!(finder.find(&haystack), None);
            haystack.extend_from_slice(needle);
            assert_eq!(finder.find(&haystack), Some(near.len() * 100));
        }
    }

    #[test]
    fn long_boundary_uses_fallback() {
        let mut needle = b"\r\n--".to_vec();
        needle.extend(std::iter::repeat_n(b'b', 300));
        let finder = DelimiterFinder::new(&needle);
        assert!(matches!(finder.simd, Simd::None));
        let mut haystack = vec![b'x'; 1000];
        haystack.extend_from_slice(&needle);
        assert_eq!(finder.find(&haystack), Some(1000));
    }
}
