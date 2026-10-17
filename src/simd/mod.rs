// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Provides various high-throughput utilities.

pub mod lines_bwd;
pub mod lines_fwd;
mod memchr2;
mod memset;

pub use lines_bwd::*;
pub use lines_fwd::*;
pub use memchr2::*;
pub use memset::*;

#[cfg(test)]
mod test {
    // Knuth's MMIX LCG
    pub fn make_rng() -> impl FnMut() -> usize {
        let mut state = 1442695040888963407u64;
        move || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            state as usize
        }
    }

    pub fn generate_random_text(len: usize) -> String {
        const ALPHABET: &[u8; 20] = b"0123456789abcdef\n\n\n\n";

        let mut rng = make_rng();
        let mut res = String::new();

        for _ in 0..len {
            res.push(ALPHABET[rng() % ALPHABET.len()] as char);
        }

        res
    }

    pub fn count_lines(text: &str) -> usize {
        text.lines().count()
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;

    #[test]
    fn dispatch_is_safe_for_simultaneous_background_workers() {
        std::thread::scope(|scope| {
            for _ in 0..16 {
                scope.spawn(|| {
                    let text = b"alpha\nbeta\ngamma".repeat(512);
                    for turn in 0..128 {
                        assert_eq!(memchr2(b'\n', b'!', &text, 0), 5);
                        assert_eq!(lines_fwd(&text, 0, 0, 2), (11, 2));
                        assert_eq!(lines_bwd(&text, 11, 2, 0), (0, 0));
                        let mut values = [0u32; 257];
                        memset(&mut values[1..256], turn);
                        assert_eq!(values[0], 0);
                        assert_eq!(values[256], 0);
                        assert!(values[1..256].iter().all(|&x| x == turn));
                    }
                });
            }
        });
    }
}
