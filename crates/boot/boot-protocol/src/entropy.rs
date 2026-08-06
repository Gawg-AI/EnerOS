use sha2::{Digest, Sha512};

pub const RAW_BOOT_ENTROPY_SIZE: usize = 128;
pub const CONDITIONED_BOOT_SEED_SIZE: usize = 64;

const HEALTH_BLOCK_SIZE: usize = 32;
const MAX_IDENTICAL_BYTE_RUN: usize = 8;
const MIN_ONE_BITS: u32 = 256;
const MAX_ONE_BITS: u32 = 768;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntropyError {
    Length,
    AllZero,
    Repetition,
    RepeatedBlock,
    Bias,
    ConditionedZero,
    Domain,
    Context,
}

pub fn derive_boot_domain(
    seed: &[u8; CONDITIONED_BOOT_SEED_SIZE],
    domain: &[u8],
    context: &[u8],
) -> Result<[u8; CONDITIONED_BOOT_SEED_SIZE], EntropyError> {
    if domain.is_empty() || domain.len() > 64 || domain.iter().all(|byte| *byte == 0) {
        return Err(EntropyError::Domain);
    }
    if context.len() > 256 {
        return Err(EntropyError::Context);
    }
    let mut hash = Sha512::new();
    hash.update(b"EnerOS/boot-domain-derivation/v1");
    hash.update((domain.len() as u64).to_le_bytes());
    hash.update(domain);
    hash.update((context.len() as u64).to_le_bytes());
    hash.update(context);
    hash.update(seed);
    let output: [u8; CONDITIONED_BOOT_SEED_SIZE] = hash.finalize().into();
    if output.iter().all(|byte| *byte == 0) {
        return Err(EntropyError::ConditionedZero);
    }
    Ok(output)
}

pub fn condition_boot_entropy(
    raw: &[u8],
) -> Result<[u8; CONDITIONED_BOOT_SEED_SIZE], EntropyError> {
    if raw.len() != RAW_BOOT_ENTROPY_SIZE {
        return Err(EntropyError::Length);
    }
    if raw.iter().all(|byte| *byte == 0) {
        return Err(EntropyError::AllZero);
    }

    let mut run = 1usize;
    for pair in raw.windows(2) {
        if pair[0] == pair[1] {
            run += 1;
            if run > MAX_IDENTICAL_BYTE_RUN {
                return Err(EntropyError::Repetition);
            }
        } else {
            run = 1;
        }
    }

    let block_count = RAW_BOOT_ENTROPY_SIZE / HEALTH_BLOCK_SIZE;
    for left in 0..block_count {
        let left_start = left * HEALTH_BLOCK_SIZE;
        for right in left + 1..block_count {
            let right_start = right * HEALTH_BLOCK_SIZE;
            if raw[left_start..left_start + HEALTH_BLOCK_SIZE]
                == raw[right_start..right_start + HEALTH_BLOCK_SIZE]
            {
                return Err(EntropyError::RepeatedBlock);
            }
        }
    }

    let one_bits = raw.iter().map(|byte| byte.count_ones()).sum::<u32>();
    if !(MIN_ONE_BITS..=MAX_ONE_BITS).contains(&one_bits) {
        return Err(EntropyError::Bias);
    }

    let mut hash = Sha512::new();
    hash.update(b"EnerOS/conditioned-boot-seed/v1");
    hash.update((RAW_BOOT_ENTROPY_SIZE as u64).to_le_bytes());
    hash.update(raw);
    let seed: [u8; CONDITIONED_BOOT_SEED_SIZE] = hash.finalize().into();
    if seed.iter().all(|byte| *byte == 0) {
        return Err(EntropyError::ConditionedZero);
    }
    Ok(seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy() -> [u8; RAW_BOOT_ENTROPY_SIZE] {
        core::array::from_fn(|index| (index as u8).wrapping_mul(73).wrapping_add(19))
    }

    #[test]
    fn healthy_input_is_conditioned_and_every_input_bit_is_bound() {
        let raw = healthy();
        let seed = condition_boot_entropy(&raw).unwrap();
        assert!(seed.iter().any(|byte| *byte != 0));
        assert_ne!(&seed[..], &raw[..CONDITIONED_BOOT_SEED_SIZE]);

        for index in 0..raw.len() {
            let mut changed = raw;
            changed[index] ^= 1;
            assert_ne!(condition_boot_entropy(&changed).unwrap(), seed);
        }
    }

    #[test]
    fn wrong_length_zero_stuck_repeated_and_biased_sources_fail_closed() {
        assert_eq!(condition_boot_entropy(&[1; 127]), Err(EntropyError::Length));
        assert_eq!(
            condition_boot_entropy(&[0; RAW_BOOT_ENTROPY_SIZE]),
            Err(EntropyError::AllZero)
        );

        let mut stuck = healthy();
        stuck[40..49].fill(0xa5);
        assert_eq!(
            condition_boot_entropy(&stuck),
            Err(EntropyError::Repetition)
        );

        let repeated: [u8; RAW_BOOT_ENTROPY_SIZE] =
            core::array::from_fn(|index| (index % HEALTH_BLOCK_SIZE) as u8);
        assert_eq!(
            condition_boot_entropy(&repeated),
            Err(EntropyError::RepeatedBlock)
        );

        let biased: [u8; RAW_BOOT_ENTROPY_SIZE] =
            core::array::from_fn(|index| if index % 5 == 0 { 1 } else { 0 });
        assert_eq!(condition_boot_entropy(&biased), Err(EntropyError::Bias));
    }

    #[test]
    fn domain_derivation_binds_domain_context_and_seed() {
        let seed = condition_boot_entropy(&healthy()).unwrap();
        let boot_nonce = derive_boot_domain(&seed, b"boot-nonce", b"attempt-7").unwrap();
        assert_ne!(
            boot_nonce,
            derive_boot_domain(&seed, b"quote-nonce", b"attempt-7").unwrap()
        );
        assert_ne!(
            boot_nonce,
            derive_boot_domain(&seed, b"boot-nonce", b"attempt-8").unwrap()
        );
        let mut other_seed = seed;
        other_seed[0] ^= 1;
        assert_ne!(
            boot_nonce,
            derive_boot_domain(&other_seed, b"boot-nonce", b"attempt-7").unwrap()
        );
        assert_eq!(
            derive_boot_domain(&seed, b"", b"context"),
            Err(EntropyError::Domain)
        );
        assert_eq!(
            derive_boot_domain(&seed, b"domain", &[0; 257]),
            Err(EntropyError::Context)
        );
    }
}
