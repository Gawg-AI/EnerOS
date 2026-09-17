//! Durable ownership of accepted plans (ADR-0054). Events and their complete
//! certified input are committed together; RAM never outruns the durable copy.
use alloc::{boxed::Box, rc::Rc, string::String, vec::Vec};
use core::cell::RefCell;

use eneros_decision_contract::{CandidatePlan, CommandBatch, CommandIntent};
use eneros_durable_state::{
    Decode, DurableStateStore, Encode, Generation, PersistIo, RegionStateStore, StoreError,
};

use crate::certify::CertifierReply;
use crate::dispatch::AuthenticatedActor;
use crate::saga::{
    BatchSaga, BatchState, CmdState, EventKind, SagaEvent, SagaEventStore, SagaPolicy,
};

const MAX_PLANS: usize = 2;
const MAX_INTENTS: usize = 8;
const MAX_EVENTS: usize = 96;
const MAX_BATCHES: usize = 2;
const MAX_BYTES: usize = 60_000;

#[derive(Clone)]
pub(crate) struct HeldBatch {
    pub id: [u8; 16],
    pub intents: Vec<CommandIntent>,
    pub edges: Vec<(u32, u32)>,
    pub deadline_ms: u64,
    pub events: Vec<SagaEvent>,
}
impl HeldBatch {
    pub fn saga(&self) -> Result<BatchSaga, String> {
        if self.events.is_empty() {
            Ok(BatchSaga::new(
                self.id,
                &self.intents,
                &self.edges,
                self.deadline_ms,
                SagaPolicy::default(),
            ))
        } else {
            BatchSaga::restore(
                &self.events,
                &self.intents,
                &self.edges,
                self.deadline_ms,
                SagaPolicy::default(),
            )
            .ok_or_else(|| String::from("held plan event stream invalid"))
        }
    }
}

#[derive(Clone)]
pub(crate) struct HeldPlan {
    pub plan: CandidatePlan,
    pub certificate: CertifierReply,
    pub input: Vec<u8>,
    pub actor: AuthenticatedActor,
    pub batches: Vec<HeldBatch>,
    /// Set durably before handling requests in a new process. Monotonic times
    /// from a previous process must never authorize a fresh device write.
    pub recovered: bool,
}
impl HeldPlan {
    pub fn terminal(&self) -> bool {
        self.batches
            .iter()
            .all(|b| b.saga().map(|s| s.state().is_terminal()).unwrap_or(false))
    }
    pub fn unresolved(&self) -> bool {
        self.batches.iter().any(|b| {
            b.saga()
                .map(|s| {
                    s.report().commands.iter().any(|c| {
                        matches!(
                            c.state,
                            CmdState::Executing
                                | CmdState::Unknown
                                | CmdState::Reconciling
                                | CmdState::Compensating
                        )
                    })
                })
                .unwrap_or(true)
        })
    }
}

#[derive(Clone, Default)]
struct HoldState {
    plans: Vec<HeldPlan>,
}
struct Repository {
    store: Box<dyn DurableStateStore<HoldState>>,
    generation: Option<Generation>,
    state: HoldState,
    poisoned: bool,
}
impl Repository {
    fn commit(&mut self, next: HoldState) -> Result<(), String> {
        if self.poisoned {
            return Err(String::from("held storage uncertain; reopen required"));
        }
        // Reject shapes that this format cannot restore before acknowledging
        // ownership (including bounds and transition validation).
        let mut encoded = alloc::vec![0u8; 8];
        encoded.extend_from_slice(
            &next
                .encode()
                .map_err(|e| alloc::format!("held encode: {e}"))?,
        );
        HoldState::decode(&encoded).map_err(|e| alloc::format!("held validate: {e}"))?;
        match self.store.compare_and_swap(self.generation, &next) {
            Ok(generation) => {
                self.generation = Some(generation);
                self.state = next;
                Ok(())
            }
            Err(error) => {
                self.poisoned = true;
                Err(alloc::format!("held storage: {error}"))
            }
        }
    }
}

/// One active and one queued plan; all updates use the registered dual-slot
/// region. Opening corrupt/unavailable storage is an error, never an empty plan.
pub struct DurablePlans {
    repository: Rc<RefCell<Repository>>,
}
impl DurablePlans {
    pub fn open<Io: PersistIo + 'static>(io: Io, offset: u64, len: u64) -> Result<Self, String> {
        let mut store = RegionStateStore::<HoldState, _>::new(io, offset, len);
        let loaded = store
            .load()
            .map_err(|e| alloc::format!("held storage open: {e}"))?;
        let (generation, state) = loaded
            .map(|v| (Some(v.generation), v.state))
            .unwrap_or_default();
        let mut repository = Repository {
            store: Box::new(store),
            generation,
            state,
            poisoned: false,
        };
        let mut next = repository.state.clone();
        let mut changed = false;
        for plan in &mut next.plans {
            if !plan.terminal() && !plan.recovered {
                plan.recovered = true;
                changed = true;
            }
        }
        if changed {
            repository.commit(next)?;
        }
        Ok(Self {
            repository: Rc::new(RefCell::new(repository)),
        })
    }
    pub(crate) fn plans(&self) -> Result<Vec<HeldPlan>, String> {
        let repository = self.repository.borrow();
        if repository.poisoned {
            return Err(String::from("held storage uncertain; reopen required"));
        }
        Ok(repository.state.plans.clone())
    }
    pub(crate) fn accept(
        &mut self,
        plan: &CandidatePlan,
        certificate: CertifierReply,
        input: Vec<u8>,
        actor: AuthenticatedActor,
        batches: &[CommandBatch],
    ) -> Result<(), String> {
        let mut repository = self.repository.borrow_mut();
        let mut next = repository.state.clone();
        if next.plans.iter().any(|p| p.plan.id() == plan.id()) {
            return Err(String::from("held duplicate identity"));
        }
        // Never evict an unresolved physical effect, even if its batch has
        // stopped in ManualIntervention. Completed entries remain queryable
        // until capacity is needed by a new accepted plan.
        if next.plans.len() == MAX_PLANS {
            if let Some(index) = next
                .plans
                .iter()
                .position(|p| p.terminal() && !p.unresolved())
            {
                next.plans.remove(index);
            }
        }
        if next.plans.len() >= MAX_PLANS {
            return Err(String::from("held plan capacity full"));
        }
        if batches.is_empty()
            || batches.len() > MAX_BATCHES
            || batches.iter().map(|b| b.intents().len()).sum::<usize>() > MAX_INTENTS * MAX_BATCHES
        {
            return Err(String::from(
                "held plan exceeds two batches of eight intents",
            ));
        }
        next.plans.push(HeldPlan {
            plan: plan.clone(),
            certificate,
            input,
            actor,
            recovered: false,
            batches: batches
                .iter()
                .map(|b| HeldBatch {
                    id: *b.batch_id(),
                    intents: b.intents().to_vec(),
                    edges: b.dependency_edges().to_vec(),
                    deadline_ms: b.deadline_ms(),
                    events: Vec::new(),
                })
                .collect(),
        });
        repository.commit(next)
    }
    pub(crate) fn saga(&self, batch: &HeldBatch) -> Result<BatchSaga, String> {
        Ok(batch.saga()?.with_event_store(Box::new(EventStore {
            repository: self.repository.clone(),
        })))
    }
}
struct EventStore {
    repository: Rc<RefCell<Repository>>,
}
impl SagaEventStore for EventStore {
    fn append(&mut self, event: &SagaEvent) -> Result<(), String> {
        let mut repository = self.repository.borrow_mut();
        let mut next = repository.state.clone();
        let batch = next
            .plans
            .iter_mut()
            .flat_map(|p| p.batches.iter_mut())
            .find(|b| b.id == event.batch_id)
            .ok_or_else(|| String::from("held batch missing"))?;
        if batch.events.len() >= MAX_EVENTS || event.seq != batch.events.len() as u64 + 1 {
            return Err(String::from("held event capacity/sequence"));
        }
        batch.events.push(event.clone());
        batch.saga()?; // Validate the complete transition before persisting.
        repository.commit(next)
    }
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), StoreError> {
    let len = u16::try_from(bytes.len()).map_err(|_| StoreError::OutOfSpace)?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}
const BATCH_STATES: [BatchState; 9] = [
    BatchState::Pending,
    BatchState::Authorizing,
    BatchState::Executing,
    BatchState::Reconciling,
    BatchState::Completed,
    BatchState::Failed,
    BatchState::Compensating,
    BatchState::Compensated,
    BatchState::ManualIntervention,
];
const CMD_STATES: [CmdState; 9] = [
    CmdState::Pending,
    CmdState::Executing,
    CmdState::Applied,
    CmdState::Failed,
    CmdState::Unknown,
    CmdState::Blocked,
    CmdState::Reconciling,
    CmdState::Compensating,
    CmdState::Compensated,
];
fn code<T: PartialEq>(states: &[T], state: &T) -> Result<u8, StoreError> {
    states
        .iter()
        .position(|v| v == state)
        .map(|n| n as u8)
        .ok_or(StoreError::Corrupt)
}
impl Encode for HoldState {
    fn encode(&self) -> Result<Vec<u8>, StoreError> {
        if self.plans.len() > MAX_PLANS {
            return Err(StoreError::OutOfSpace);
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"HPL1");
        out.push(self.plans.len() as u8);
        for p in &self.plans {
            put_bytes(&mut out, &p.plan.encode_v1())?;
            put_bytes(&mut out, &p.certificate.certificate)?;
            out.extend_from_slice(&p.certificate.key_id.to_le_bytes());
            out.extend_from_slice(&p.certificate.key_version.to_le_bytes());
            out.extend_from_slice(&p.certificate.mac);
            put_bytes(&mut out, &p.input)?;
            if p.actor.kind != eneros_control_gateway::envelope::ActorKind::Ai {
                return Err(StoreError::Corrupt);
            }
            out.extend_from_slice(&p.actor.service);
            out.extend_from_slice(&p.actor.session_epoch.to_le_bytes());
            out.push(u8::from(p.recovered));
            out.push(p.batches.len() as u8);
            for b in &p.batches {
                out.extend_from_slice(&b.id);
                out.extend_from_slice(&b.deadline_ms.to_le_bytes());
                out.push(b.intents.len() as u8);
                for intent in &b.intents {
                    out.extend_from_slice(&intent.encode_v1());
                }
                out.push(b.edges.len() as u8);
                for (a, z) in &b.edges {
                    out.extend_from_slice(&a.to_le_bytes());
                    out.extend_from_slice(&z.to_le_bytes());
                }
                out.push(b.events.len() as u8);
                for e in &b.events {
                    out.extend_from_slice(&e.seq.to_le_bytes());
                    match e.kind {
                        EventKind::Batch { from, to } => {
                            out.push(0);
                            out.push(code(&BATCH_STATES, &from)?);
                            out.push(code(&BATCH_STATES, &to)?);
                        }
                        EventKind::Command {
                            intent_id,
                            from,
                            to,
                        } => {
                            out.push(1);
                            out.extend_from_slice(&intent_id);
                            out.push(code(&CMD_STATES, &from)?);
                            out.push(code(&CMD_STATES, &to)?);
                        }
                    }
                    out.extend_from_slice(&e.attempt.to_le_bytes());
                    out.extend_from_slice(&e.digest);
                    out.push(u8::from(e.readback.is_some()));
                    out.extend_from_slice(&e.readback.unwrap_or(0.0).to_le_bytes());
                    put_bytes(&mut out, e.reason.as_bytes())?;
                    out.extend_from_slice(&e.at_ms.to_le_bytes());
                }
            }
        }
        if out.len() > MAX_BYTES {
            return Err(StoreError::OutOfSpace);
        }
        Ok(out)
    }
}
struct Reader<'a> {
    bytes: &'a [u8],
}
impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], StoreError> {
        if len > self.bytes.len() {
            return Err(StoreError::Corrupt);
        }
        let (part, rest) = self.bytes.split_at(len);
        self.bytes = rest;
        Ok(part)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], StoreError> {
        self.take(N)?.try_into().map_err(|_| StoreError::Corrupt)
    }
    fn byte(&mut self) -> Result<u8, StoreError> {
        Ok(self.array::<1>()?[0])
    }
    fn u32(&mut self) -> Result<u32, StoreError> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    fn u64(&mut self) -> Result<u64, StoreError> {
        Ok(u64::from_le_bytes(self.array()?))
    }
    fn bytes(&mut self, max: usize) -> Result<Vec<u8>, StoreError> {
        let n = u16::from_le_bytes(self.array()?) as usize;
        if n > max {
            return Err(StoreError::Corrupt);
        }
        Ok(self.take(n)?.to_vec())
    }
    fn count(&mut self, max: usize) -> Result<usize, StoreError> {
        let n = self.byte()? as usize;
        if n > max {
            return Err(StoreError::Corrupt);
        }
        Ok(n)
    }
    fn boolean(&mut self) -> Result<bool, StoreError> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(StoreError::Corrupt),
        }
    }
}
impl Decode for HoldState {
    fn decode(bytes: &[u8]) -> Result<(Generation, Self), StoreError> {
        if bytes.len() > MAX_BYTES + 8 {
            return Err(StoreError::Corrupt);
        }
        let mut r = Reader { bytes };
        let generation = Generation(r.u64()?);
        if r.take(4)? != b"HPL1" {
            return Err(StoreError::Corrupt);
        }
        let mut plans = Vec::new();
        for _ in 0..r.count(MAX_PLANS)? {
            let plan =
                CandidatePlan::decode_v1(&r.bytes(4096)?).map_err(|_| StoreError::Corrupt)?;
            let certificate = CertifierReply {
                certificate: r.bytes(1024)?,
                key_id: u16::from_le_bytes(r.array()?),
                key_version: r.u32()?,
                mac: r.array()?,
            };
            let input = r.bytes(eneros_agent_sdk::snap_input_wire::SNAP_INPUT_WIRE_LEN)?;
            let trusted = eneros_agent_sdk::snap_input_wire::TrustedInput::decode(&input)
                .ok_or(StoreError::Corrupt)?;
            if trusted.facts().snapshot_id != plan.snapshot_id().0
                || trusted.facts().catalog_version != plan.resource_catalog_version()
            {
                return Err(StoreError::Corrupt);
            }
            let actor = AuthenticatedActor {
                kind: eneros_control_gateway::envelope::ActorKind::Ai,
                service: r.array()?,
                session_epoch: r.u64()?,
            };
            let recovered = r.boolean()?;
            let mut batches = Vec::new();
            let mut total = 0;
            for _ in 0..r.count(MAX_BATCHES)? {
                let id = r.array()?;
                let deadline_ms = r.u64()?;
                let mut intents = Vec::new();
                for _ in 0..r.count(MAX_INTENTS)? {
                    let intent = CommandIntent::decode_v1(
                        r.take(eneros_decision_contract::intent::INTENT_WIRE_LEN)?,
                    )
                    .map_err(|_| StoreError::Corrupt)?;
                    if intent.plan_id() != plan.id()
                        || intent.plan_sha256() != &CandidatePlan::plan_wire_sha256(&plan)
                        || intents
                            .iter()
                            .any(|i: &CommandIntent| i.intent_id() == intent.intent_id())
                    {
                        return Err(StoreError::Corrupt);
                    }
                    intents.push(intent);
                }
                total += intents.len();
                if intents.is_empty() || total > MAX_INTENTS * MAX_BATCHES {
                    return Err(StoreError::Corrupt);
                }
                let mut edges = Vec::new();
                for _ in 0..r.count(MAX_INTENTS * MAX_INTENTS)? {
                    let a = r.u32()?;
                    let z = r.u32()?;
                    // Compiler orders intents topologically; reverse/self edges
                    // are unsupported in this durable version, not guessed.
                    if a >= z || z as usize >= intents.len() {
                        return Err(StoreError::Corrupt);
                    }
                    edges.push((a, z));
                }
                let mut events = Vec::new();
                for _ in 0..r.count(MAX_EVENTS)? {
                    let seq = r.u64()?;
                    let kind = match r.byte()? {
                        0 => EventKind::Batch {
                            from: *BATCH_STATES
                                .get(r.byte()? as usize)
                                .ok_or(StoreError::Corrupt)?,
                            to: *BATCH_STATES
                                .get(r.byte()? as usize)
                                .ok_or(StoreError::Corrupt)?,
                        },
                        1 => EventKind::Command {
                            intent_id: r.array()?,
                            from: *CMD_STATES
                                .get(r.byte()? as usize)
                                .ok_or(StoreError::Corrupt)?,
                            to: *CMD_STATES
                                .get(r.byte()? as usize)
                                .ok_or(StoreError::Corrupt)?,
                        },
                        _ => return Err(StoreError::Corrupt),
                    };
                    let attempt = r.u32()?;
                    let digest = r.array()?;
                    let has_readback = r.boolean()?;
                    let value = f64::from_le_bytes(r.array()?);
                    if !value.is_finite() {
                        return Err(StoreError::Corrupt);
                    }
                    let reason =
                        String::from_utf8(r.bytes(1024)?).map_err(|_| StoreError::Corrupt)?;
                    events.push(SagaEvent {
                        seq,
                        batch_id: id,
                        kind,
                        attempt,
                        digest,
                        readback: has_readback.then_some(value),
                        reason,
                        at_ms: r.u64()?,
                    });
                }
                let batch = HeldBatch {
                    id,
                    intents,
                    edges,
                    deadline_ms,
                    events,
                };
                batch.saga().map_err(|_| StoreError::Corrupt)?;
                batches.push(batch);
            }
            if batches.is_empty() || plans.iter().any(|p: &HeldPlan| p.plan.id() == plan.id()) {
                return Err(StoreError::Corrupt);
            }
            plans.push(HeldPlan {
                plan,
                certificate,
                input,
                actor,
                batches,
                recovered,
            });
        }
        if !r.bytes.is_empty() {
            return Err(StoreError::Corrupt);
        }
        Ok((generation, Self { plans }))
    }
}
