use rand::rngs::StdRng;
use rand::SeedableRng;
use sha2::{Digest, Sha256};

pub fn seeded_rng(test_name: &str) -> StdRng {
    let seed: [u8; 32] = Sha256::digest(test_name.as_bytes()).into();
    StdRng::from_seed(seed)
}
