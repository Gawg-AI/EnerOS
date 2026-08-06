use sha2::{Digest, Sha256};

use crate::BootSlot;

pub const BOOT_CONTROL_MAGIC: [u8; 8] = *b"ENERBC01";
pub const BOOT_CONTROL_VERSION: u16 = 1;
pub const BOOT_CONTROL_HEADER_SIZE: u16 = 192;
pub const BOOT_CONTROL_RECORD_SIZE: usize = 4096;
pub const BOOT_CONTROL_COPY_OFFSETS: [u64; 2] = [0, 4096];
pub const BOOT_CONTROL_MAX_PRIORITY: u8 = 15;
pub const BOOT_CONTROL_MAX_TRIES: u8 = 7;
pub const BOOT_CONTROL_FLAG_MANUAL_RECOVERY: u32 = 1;
pub const BOOT_CONTROL_ANCHOR_SIZE: usize = 64;

const BOOT_CONTROL_KNOWN_FLAGS: u32 = BOOT_CONTROL_FLAG_MANUAL_RECOVERY;
const BOOT_CONTROL_ANCHOR_MAGIC: [u8; 8] = *b"ENRBCA01";
const BOOT_CONTROL_ANCHOR_VERSION: u16 = 1;
const SLOT_SIZE: usize = 64;
const SLOT_A_OFFSET: usize = 64;
const SLOT_B_OFFSET: usize = SLOT_A_OFFSET + SLOT_SIZE;
const CRC_OFFSET: usize = BOOT_CONTROL_RECORD_SIZE - 4;
const SLOT_FLAG_SUCCESSFUL: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SlotError {
    None = 0,
    AttemptInProgress = 1,
    Verification = 2,
    BootFailure = 3,
    HealthFailure = 4,
}

impl TryFrom<u32> for SlotError {
    type Error = BootControlError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::None),
            1 => Ok(Self::AttemptInProgress),
            2 => Ok(Self::Verification),
            3 => Ok(Self::BootFailure),
            4 => Ok(Self::HealthFailure),
            _ => Err(BootControlError::SlotError),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotState {
    pub priority: u8,
    pub tries_remaining: u8,
    pub successful: bool,
    pub security_version: u64,
    pub image_id: [u8; 16],
    pub last_error: SlotError,
    pub boot_attempts: u64,
}

impl SlotState {
    pub const fn trial(
        priority: u8,
        tries_remaining: u8,
        security_version: u64,
        image_id: [u8; 16],
    ) -> Self {
        Self {
            priority,
            tries_remaining,
            successful: false,
            security_version,
            image_id,
            last_error: SlotError::None,
            boot_attempts: 0,
        }
    }

    pub const fn successful(priority: u8, security_version: u64, image_id: [u8; 16]) -> Self {
        Self {
            priority,
            tries_remaining: 0,
            successful: true,
            security_version,
            image_id,
            last_error: SlotError::None,
            boot_attempts: 0,
        }
    }

    pub const fn disabled(security_version: u64, image_id: [u8; 16]) -> Self {
        Self {
            priority: 0,
            tries_remaining: 0,
            successful: false,
            security_version,
            image_id,
            last_error: SlotError::None,
            boot_attempts: 0,
        }
    }

    fn validate(&self) -> Result<(), BootControlError> {
        if self.priority > BOOT_CONTROL_MAX_PRIORITY {
            return Err(BootControlError::Priority);
        }
        if self.tries_remaining > BOOT_CONTROL_MAX_TRIES {
            return Err(BootControlError::Tries);
        }
        if self.security_version == 0 || is_zero(&self.image_id) {
            return Err(BootControlError::Identity);
        }
        if (self.priority == 0 && (self.successful || self.tries_remaining != 0))
            || (self.successful && self.tries_remaining != 0)
        {
            return Err(BootControlError::State);
        }
        Ok(())
    }

    fn eligible(&self, floor: u64) -> bool {
        self.priority != 0
            && self.security_version >= floor
            && (self.successful || self.tries_remaining != 0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootControl {
    pub sequence: u64,
    pub security_floor: u64,
    pub recovery_security_version: u64,
    pub recovery_image_id: [u8; 16],
    pub flags: u32,
    pub slot_a: SlotState,
    pub slot_b: SlotState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootSelection {
    pub slot: BootSlot,
    pub trial: bool,
    pub security_version: u64,
    pub image_id: [u8; 16],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootControlAnchor {
    pub sequence: u64,
    pub security_floor: u64,
    pub record_digest: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootControlCopy {
    First,
    Second,
}

impl BootControlCopy {
    pub const fn other(self) -> Self {
        match self {
            Self::First => Self::Second,
            Self::Second => Self::First,
        }
    }

    pub const fn offset(self) -> u64 {
        match self {
            Self::First => BOOT_CONTROL_COPY_OFFSETS[0],
            Self::Second => BOOT_CONTROL_COPY_OFFSETS[1],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedBootControl {
    pub control: BootControl,
    pub source: BootControlCopy,
    pub anchor: BootControlAnchor,
    pub needs_anchor_initialization: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootControlTransaction {
    pub target: BootControlCopy,
    pub mirror: BootControlCopy,
    pub record: [u8; BOOT_CONTROL_RECORD_SIZE],
    pub anchor: BootControlAnchor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootControlError {
    Length,
    Magic,
    Version,
    Reserved,
    Crc,
    Flags,
    Priority,
    Tries,
    SlotFlags,
    SlotError,
    Identity,
    State,
    NoBootableSlot,
    SequenceOverflow,
    Anchor,
    AnchorMismatch,
    Genesis,
    Transaction,
    Rollback,
}

impl BootControlAnchor {
    pub fn new(
        control: &BootControl,
        record: &[u8; BOOT_CONTROL_RECORD_SIZE],
    ) -> Result<Self, BootControlError> {
        control.validate()?;
        if control.encode()? != *record {
            return Err(BootControlError::Anchor);
        }
        Ok(Self {
            sequence: control.sequence,
            security_floor: control.security_floor,
            record_digest: BootControl::digest(record),
        })
    }

    pub fn encode(&self) -> Result<[u8; BOOT_CONTROL_ANCHOR_SIZE], BootControlError> {
        if self.security_floor == 0 || is_zero(&self.record_digest) {
            return Err(BootControlError::Anchor);
        }
        let mut bytes = [0u8; BOOT_CONTROL_ANCHOR_SIZE];
        bytes[0..8].copy_from_slice(&BOOT_CONTROL_ANCHOR_MAGIC);
        put_u16(&mut bytes, 8, BOOT_CONTROL_ANCHOR_VERSION);
        put_u16(&mut bytes, 10, BOOT_CONTROL_ANCHOR_SIZE as u16);
        put_u64(&mut bytes, 16, self.sequence);
        put_u64(&mut bytes, 24, self.security_floor);
        bytes[32..64].copy_from_slice(&self.record_digest);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, BootControlError> {
        if bytes.len() != BOOT_CONTROL_ANCHOR_SIZE {
            return Err(BootControlError::Length);
        }
        if bytes[0..8] != BOOT_CONTROL_ANCHOR_MAGIC {
            return Err(BootControlError::Magic);
        }
        if get_u16(bytes, 8) != BOOT_CONTROL_ANCHOR_VERSION
            || usize::from(get_u16(bytes, 10)) != BOOT_CONTROL_ANCHOR_SIZE
            || !is_zero(&bytes[12..16])
        {
            return Err(BootControlError::Version);
        }
        let anchor = Self {
            sequence: get_u64(bytes, 16),
            security_floor: get_u64(bytes, 24),
            record_digest: bytes[32..64]
                .try_into()
                .map_err(|_| BootControlError::Length)?,
        };
        if anchor.security_floor == 0 || is_zero(&anchor.record_digest) {
            return Err(BootControlError::Anchor);
        }
        if anchor.encode()?.as_slice() != bytes {
            return Err(BootControlError::Anchor);
        }
        Ok(anchor)
    }
}

impl BootControlTransaction {
    pub fn plan(
        current: &ResolvedBootControl,
        next: &BootControl,
    ) -> Result<Self, BootControlError> {
        if next.sequence
            != current
                .control
                .sequence
                .checked_add(1)
                .ok_or(BootControlError::SequenceOverflow)?
        {
            return Err(BootControlError::Transaction);
        }
        if next.security_floor < current.control.security_floor {
            return Err(BootControlError::Rollback);
        }
        let record = next.encode()?;
        let anchor = BootControlAnchor::new(next, &record)?;
        Ok(Self {
            target: current.source.other(),
            mirror: current.source,
            record,
            anchor,
        })
    }
}

pub fn resolve_boot_control(
    first: &[u8; BOOT_CONTROL_RECORD_SIZE],
    second: &[u8; BOOT_CONTROL_RECORD_SIZE],
    protected_anchor: Option<&BootControlAnchor>,
) -> Result<ResolvedBootControl, BootControlError> {
    match protected_anchor {
        Some(anchor) => {
            for (source, record) in [
                (BootControlCopy::First, first),
                (BootControlCopy::Second, second),
            ] {
                let Ok(control) = BootControl::decode(record) else {
                    continue;
                };
                if control.sequence == anchor.sequence
                    && control.security_floor == anchor.security_floor
                    && BootControl::digest(record) == anchor.record_digest
                {
                    return Ok(ResolvedBootControl {
                        control,
                        source,
                        anchor: *anchor,
                        needs_anchor_initialization: false,
                    });
                }
            }
            Err(BootControlError::AnchorMismatch)
        }
        None => {
            let first_control = BootControl::decode(first)?;
            let second_control = BootControl::decode(second)?;
            if first != second || first_control != second_control || first_control.sequence != 0 {
                return Err(BootControlError::Genesis);
            }
            let anchor = BootControlAnchor::new(&first_control, first)?;
            Ok(ResolvedBootControl {
                control: first_control,
                source: BootControlCopy::First,
                anchor,
                needs_anchor_initialization: true,
            })
        }
    }
}

impl BootControl {
    pub const fn factory(
        image_id: [u8; 16],
        security_version: u64,
        recovery_image_id: [u8; 16],
    ) -> Self {
        Self {
            sequence: 0,
            security_floor: security_version,
            recovery_security_version: security_version,
            recovery_image_id,
            flags: 0,
            slot_a: SlotState::trial(15, 3, security_version, image_id),
            slot_b: SlotState::successful(14, security_version, image_id),
        }
    }

    pub fn validate(&self) -> Result<(), BootControlError> {
        if self.security_floor == 0
            || self.recovery_security_version == 0
            || is_zero(&self.recovery_image_id)
        {
            return Err(BootControlError::Identity);
        }
        if self.flags & !BOOT_CONTROL_KNOWN_FLAGS != 0 {
            return Err(BootControlError::Flags);
        }
        self.slot_a.validate()?;
        self.slot_b.validate()?;
        Ok(())
    }

    pub fn select(&self) -> Result<BootSelection, BootControlError> {
        self.validate()?;
        if self.flags & BOOT_CONTROL_FLAG_MANUAL_RECOVERY != 0 {
            return self.recovery_selection();
        }
        let a = self.slot_a.eligible(self.security_floor);
        let b = self.slot_b.eligible(self.security_floor);
        let slot = match (a, b) {
            (true, false) => BootSlot::A,
            (false, true) => BootSlot::B,
            (true, true) => {
                if self.slot_b.priority > self.slot_a.priority {
                    BootSlot::B
                } else if self.slot_b.priority < self.slot_a.priority {
                    BootSlot::A
                } else if self.slot_b.successful && !self.slot_a.successful {
                    BootSlot::B
                } else {
                    BootSlot::A
                }
            }
            (false, false) => return self.recovery_selection(),
        };
        let selected = self.slot(slot)?;
        Ok(BootSelection {
            slot,
            trial: !selected.successful,
            security_version: selected.security_version,
            image_id: selected.image_id,
        })
    }

    pub fn prepare_attempt(&self) -> Result<(BootSelection, Option<Self>), BootControlError> {
        let selection = self.select()?;
        if !selection.trial || selection.slot == BootSlot::Recovery {
            return Ok((selection, None));
        }
        let mut next = *self;
        next.sequence = next
            .sequence
            .checked_add(1)
            .ok_or(BootControlError::SequenceOverflow)?;
        let state = next.slot_mut(selection.slot)?;
        state.tries_remaining = state
            .tries_remaining
            .checked_sub(1)
            .ok_or(BootControlError::State)?;
        state.last_error = SlotError::AttemptInProgress;
        state.boot_attempts = state
            .boot_attempts
            .checked_add(1)
            .ok_or(BootControlError::SequenceOverflow)?;
        next.validate()?;
        Ok((selection, Some(next)))
    }

    pub fn mark_verification_failed(&self, slot: BootSlot) -> Result<Self, BootControlError> {
        if slot == BootSlot::Recovery {
            return Err(BootControlError::NoBootableSlot);
        }
        let mut next = *self;
        next.sequence = next
            .sequence
            .checked_add(1)
            .ok_or(BootControlError::SequenceOverflow)?;
        let state = next.slot_mut(slot)?;
        state.priority = 0;
        state.tries_remaining = 0;
        state.successful = false;
        state.last_error = SlotError::Verification;
        next.validate()?;
        Ok(next)
    }

    pub fn encode(&self) -> Result<[u8; BOOT_CONTROL_RECORD_SIZE], BootControlError> {
        self.validate()?;
        let mut bytes = [0u8; BOOT_CONTROL_RECORD_SIZE];
        bytes[0..8].copy_from_slice(&BOOT_CONTROL_MAGIC);
        put_u16(&mut bytes, 8, BOOT_CONTROL_VERSION);
        put_u16(&mut bytes, 10, BOOT_CONTROL_HEADER_SIZE);
        put_u32(&mut bytes, 12, BOOT_CONTROL_RECORD_SIZE as u32);
        put_u64(&mut bytes, 16, self.sequence);
        put_u64(&mut bytes, 24, self.security_floor);
        put_u64(&mut bytes, 32, self.recovery_security_version);
        bytes[40..56].copy_from_slice(&self.recovery_image_id);
        put_u32(&mut bytes, 56, self.flags);
        encode_slot(
            &self.slot_a,
            &mut bytes[SLOT_A_OFFSET..SLOT_A_OFFSET + SLOT_SIZE],
        );
        encode_slot(
            &self.slot_b,
            &mut bytes[SLOT_B_OFFSET..SLOT_B_OFFSET + SLOT_SIZE],
        );
        let crc = crc32(&bytes[..CRC_OFFSET]);
        put_u32(&mut bytes, CRC_OFFSET, crc);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, BootControlError> {
        if bytes.len() != BOOT_CONTROL_RECORD_SIZE {
            return Err(BootControlError::Length);
        }
        if bytes[0..8] != BOOT_CONTROL_MAGIC {
            return Err(BootControlError::Magic);
        }
        if get_u16(bytes, 8) != BOOT_CONTROL_VERSION
            || get_u16(bytes, 10) != BOOT_CONTROL_HEADER_SIZE
            || usize::try_from(get_u32(bytes, 12)).map_err(|_| BootControlError::Length)?
                != BOOT_CONTROL_RECORD_SIZE
        {
            return Err(BootControlError::Version);
        }
        if crc32(&bytes[..CRC_OFFSET]) != get_u32(bytes, CRC_OFFSET) {
            return Err(BootControlError::Crc);
        }
        if !is_zero(&bytes[60..64])
            || !is_zero(&bytes[BOOT_CONTROL_HEADER_SIZE as usize..CRC_OFFSET])
        {
            return Err(BootControlError::Reserved);
        }
        let control = Self {
            sequence: get_u64(bytes, 16),
            security_floor: get_u64(bytes, 24),
            recovery_security_version: get_u64(bytes, 32),
            recovery_image_id: get_array_16(bytes, 40),
            flags: get_u32(bytes, 56),
            slot_a: decode_slot(&bytes[SLOT_A_OFFSET..SLOT_A_OFFSET + SLOT_SIZE])?,
            slot_b: decode_slot(&bytes[SLOT_B_OFFSET..SLOT_B_OFFSET + SLOT_SIZE])?,
        };
        control.validate()?;
        if control.encode()?.as_slice() != bytes {
            return Err(BootControlError::Reserved);
        }
        Ok(control)
    }

    pub fn digest(record: &[u8; BOOT_CONTROL_RECORD_SIZE]) -> [u8; 32] {
        Sha256::digest(record).into()
    }

    fn recovery_selection(&self) -> Result<BootSelection, BootControlError> {
        if self.recovery_security_version < self.security_floor {
            return Err(BootControlError::NoBootableSlot);
        }
        Ok(BootSelection {
            slot: BootSlot::Recovery,
            trial: false,
            security_version: self.recovery_security_version,
            image_id: self.recovery_image_id,
        })
    }

    fn slot(&self, slot: BootSlot) -> Result<&SlotState, BootControlError> {
        match slot {
            BootSlot::A => Ok(&self.slot_a),
            BootSlot::B => Ok(&self.slot_b),
            BootSlot::Recovery => Err(BootControlError::State),
        }
    }

    fn slot_mut(&mut self, slot: BootSlot) -> Result<&mut SlotState, BootControlError> {
        match slot {
            BootSlot::A => Ok(&mut self.slot_a),
            BootSlot::B => Ok(&mut self.slot_b),
            BootSlot::Recovery => Err(BootControlError::State),
        }
    }
}

fn encode_slot(slot: &SlotState, bytes: &mut [u8]) {
    bytes[0] = slot.priority;
    bytes[1] = slot.tries_remaining;
    bytes[2] = if slot.successful {
        SLOT_FLAG_SUCCESSFUL
    } else {
        0
    };
    put_u64(bytes, 8, slot.security_version);
    bytes[16..32].copy_from_slice(&slot.image_id);
    put_u32(bytes, 32, slot.last_error as u32);
    put_u64(bytes, 40, slot.boot_attempts);
}

fn decode_slot(bytes: &[u8]) -> Result<SlotState, BootControlError> {
    if bytes[2] & !SLOT_FLAG_SUCCESSFUL != 0
        || bytes[3] != 0
        || !is_zero(&bytes[4..8])
        || !is_zero(&bytes[36..40])
        || !is_zero(&bytes[48..64])
    {
        return Err(BootControlError::SlotFlags);
    }
    Ok(SlotState {
        priority: bytes[0],
        tries_remaining: bytes[1],
        successful: bytes[2] & SLOT_FLAG_SUCCESSFUL != 0,
        security_version: get_u64(bytes, 8),
        image_id: get_array_16(bytes, 16),
        last_error: SlotError::try_from(get_u32(bytes, 32))?,
        boot_attempts: get_u64(bytes, 40),
    })
}

fn crc32(bytes: &[u8]) -> u32 {
    // HYG-001：CRC32 唯一实现归一到 abi/ipc-wire（IEEE 802.3 0xEDB88320）。
    eneros_ipc_wire::crc32(bytes)
}

fn is_zero(bytes: &[u8]) -> bool {
    bytes.iter().all(|byte| *byte == 0)
}

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn get_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn get_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn get_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

fn get_array_16(bytes: &[u8], offset: usize) -> [u8; 16] {
    bytes[offset..offset + 16].try_into().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: u8) -> [u8; 16] {
        [value; 16]
    }

    fn state(priority: u8, tries: u8, successful: bool, image_id: [u8; 16]) -> SlotState {
        if priority == 0 {
            SlotState::disabled(1, image_id)
        } else if successful {
            SlotState::successful(priority, 1, image_id)
        } else {
            SlotState::trial(priority, tries, 1, image_id)
        }
    }

    fn control(a: SlotState, b: SlotState) -> BootControl {
        BootControl {
            sequence: 7,
            security_floor: 1,
            recovery_security_version: 1,
            recovery_image_id: id(3),
            flags: 0,
            slot_a: a,
            slot_b: b,
        }
    }

    #[test]
    fn canonical_record_has_fixed_layout_digest_and_crc() {
        let record = BootControl::factory(id(1), 1, id(3)).encode().unwrap();
        assert_eq!(record.len(), 4096);
        assert_eq!(&record[0..8], b"ENERBC01");
        assert_eq!(u64::from_le_bytes(record[24..32].try_into().unwrap()), 1);
        assert_eq!(BootControl::decode(&record).unwrap().sequence, 0);
        assert_ne!(BootControl::digest(&record), [0; 32]);
    }

    #[test]
    fn selector_exhaustively_covers_priorities_tries_and_success() {
        for priority_a in 0..=BOOT_CONTROL_MAX_PRIORITY {
            for tries_a in 0..=BOOT_CONTROL_MAX_TRIES {
                for successful_a in [false, true] {
                    for priority_b in 0..=BOOT_CONTROL_MAX_PRIORITY {
                        for tries_b in 0..=BOOT_CONTROL_MAX_TRIES {
                            for successful_b in [false, true] {
                                let a = state(priority_a, tries_a, successful_a, id(1));
                                let b = state(priority_b, tries_b, successful_b, id(2));
                                let selected = control(a, b).select().unwrap().slot;
                                let a_ok = a.eligible(1);
                                let b_ok = b.eligible(1);
                                let expected = if a_ok && !b_ok {
                                    BootSlot::A
                                } else if b_ok && !a_ok {
                                    BootSlot::B
                                } else if !a_ok && !b_ok {
                                    BootSlot::Recovery
                                } else if b.priority > a.priority
                                    || (b.priority == a.priority && b.successful && !a.successful)
                                {
                                    BootSlot::B
                                } else {
                                    BootSlot::A
                                };
                                assert_eq!(selected, expected);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn trial_attempt_decrements_before_handoff_without_marking_success() {
        let initial = BootControl::factory(id(1), 1, id(3));
        let (selection, next) = initial.prepare_attempt().unwrap();
        assert_eq!(selection.slot, BootSlot::A);
        assert!(selection.trial);
        let next = next.unwrap();
        assert_eq!(next.sequence, 1);
        assert_eq!(next.slot_a.tries_remaining, 2);
        assert!(!next.slot_a.successful);
        assert_eq!(next.slot_a.last_error, SlotError::AttemptInProgress);
    }

    #[test]
    fn corruption_and_noncanonical_states_are_rejected() {
        let record = BootControl::factory(id(1), 1, id(3)).encode().unwrap();
        for index in (0..record.len()).step_by(17) {
            let mut corrupted = record;
            corrupted[index] ^= 1;
            assert!(BootControl::decode(&corrupted).is_err());
        }
        let mut bad = BootControl::factory(id(1), 1, id(3));
        bad.slot_a.priority = 16;
        assert_eq!(bad.encode(), Err(BootControlError::Priority));
        let mut bad = BootControl::factory(id(1), 1, id(3));
        bad.slot_a.successful = true;
        assert_eq!(bad.encode(), Err(BootControlError::State));
        let mut bad = BootControl::factory(id(1), 1, id(3));
        bad.security_floor = 0;
        assert_eq!(bad.encode(), Err(BootControlError::Identity));
    }

    #[test]
    fn manual_and_exhausted_normal_slots_select_recovery() {
        let mut state = BootControl::factory(id(1), 1, id(3));
        state.flags = BOOT_CONTROL_FLAG_MANUAL_RECOVERY;
        assert_eq!(state.select().unwrap().slot, BootSlot::Recovery);
        state.flags = 0;
        state.slot_a = SlotState::disabled(1, id(1));
        state.slot_b = SlotState::trial(14, 0, 1, id(2));
        assert_eq!(state.select().unwrap().slot, BootSlot::Recovery);
        state.recovery_security_version = 0;
        assert!(state.select().is_err());
    }

    #[test]
    fn dual_copy_transaction_recovers_at_every_power_cut_boundary() {
        let old_control = BootControl::factory(id(1), 1, id(3));
        let old_record = old_control.encode().unwrap();
        let old_anchor = BootControlAnchor::new(&old_control, &old_record).unwrap();
        let current = resolve_boot_control(&old_record, &old_record, Some(&old_anchor)).unwrap();
        let (_, next) = old_control.prepare_attempt().unwrap();
        let next = next.unwrap();
        let transaction = BootControlTransaction::plan(&current, &next).unwrap();

        let mut first = old_record;
        let mut second = old_record;
        assert_eq!(
            resolve_boot_control(&first, &second, Some(&old_anchor))
                .unwrap()
                .control,
            old_control
        );

        second[..2048].copy_from_slice(&transaction.record[..2048]);
        assert_eq!(
            resolve_boot_control(&first, &second, Some(&old_anchor))
                .unwrap()
                .control,
            old_control,
            "cut during target write must retain the old anchored copy"
        );

        second = transaction.record;
        assert_eq!(
            resolve_boot_control(&first, &second, Some(&old_anchor))
                .unwrap()
                .control,
            old_control,
            "cut after target verify but before anchor commit must retain old state"
        );

        assert_eq!(
            resolve_boot_control(&first, &second, Some(&transaction.anchor))
                .unwrap()
                .control,
            next,
            "anchor commit makes the verified target authoritative"
        );

        first[..2048].copy_from_slice(&transaction.record[..2048]);
        assert_eq!(
            resolve_boot_control(&first, &second, Some(&transaction.anchor))
                .unwrap()
                .control,
            next,
            "cut during mirror write must retain the new anchored target"
        );

        first = transaction.record;
        assert_eq!(
            resolve_boot_control(&first, &second, Some(&transaction.anchor))
                .unwrap()
                .control,
            next
        );
    }

    #[test]
    fn anchor_mismatch_and_floor_decrease_fail_closed() {
        let control = BootControl::factory(id(1), 2, id(3));
        let record = control.encode().unwrap();
        let mut anchor = BootControlAnchor::new(&control, &record).unwrap();
        anchor.record_digest[0] ^= 1;
        assert_eq!(
            resolve_boot_control(&record, &record, Some(&anchor)),
            Err(BootControlError::AnchorMismatch)
        );

        let anchor = BootControlAnchor::new(&control, &record).unwrap();
        let resolved = resolve_boot_control(&record, &record, Some(&anchor)).unwrap();
        let mut next = control;
        next.sequence += 1;
        next.security_floor = 1;
        assert_eq!(
            BootControlTransaction::plan(&resolved, &next),
            Err(BootControlError::Rollback)
        );
    }
}
