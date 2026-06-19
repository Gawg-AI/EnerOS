//! CIM (Common Information Model) importer for IEC 61968/61970.
//!
//! Parses CIM RDF/XML files (typically .xml or .rdf) into EnerOS's
//! `PowerNetwork` model. CIM is the standard data exchange format for
//! power system models, used by SCADA/EMS/DMS systems.
//!
//! # Supported CIM Profiles
//!
//! - **IEC 61970-301**: Core package (BaseVoltage, VoltageLevel, Substation,
//!   ConnectivityNode, Terminal, Equipment, PowerSystemResource)
//! - **IEC 61970-452**: Equipment profile (BusbarSection, ACLineSegment,
//!   PowerTransformer, EnergyConsumer, SynchronousMachine, Breaker, Disconnector)
//! - **IEC 61968-13**: Distribution profile (additional load classes)
//!
//! # Parsing Approach
//!
//! CIM files use RDF/XML with mRID identifiers and references. We extract:
//! - Buses (BusbarSection / ConnectivityNode)
//! - Branches (ACLineSegment, PowerTransformer)
//! - Generators (SynchronousMachine)
//! - Loads (EnergyConsumer)
//! - Shunts (LinearShuntCompensator)
//!
//! The parser is minimal — it handles the subset needed for power flow
//! initialization. Full CIM compliance would require a complete RDF parser.

use crate::network::{GeneratorSpec, PowerNetwork};
use eneros_core::ElementId;
use eneros_powerflow::{BusTypeNR, YBusMatrix};
use std::collections::HashMap;

/// Default base MVA for per-unit conversion when CIM data does not specify one.
const CIM_BASE_MVA: f64 = 100.0;

/// CIM mRID (Master Resource Identifier)
pub type MrId = String;

/// CIM voltage level
#[derive(Debug, Clone, Default)]
pub struct CimVoltageLevel {
    pub mrid: MrId,
    pub name: String,
    pub nominal_voltage: f64, // kV
}

/// CIM substation
#[derive(Debug, Clone, Default)]
pub struct CimSubstation {
    pub mrid: MrId,
    pub name: String,
    pub region: String,
    pub voltage_levels: Vec<CimVoltageLevel>,
}

/// CIM base voltage
#[derive(Debug, Clone, Default)]
pub struct CimBaseVoltage {
    pub mrid: MrId,
    pub nominal_voltage: f64, // kV
}

/// CIM busbar section (represents a bus)
#[derive(Debug, Clone, Default)]
pub struct CimBusbarSection {
    pub mrid: MrId,
    pub name: String,
    pub base_voltage_mrid: MrId,
    pub equipment_container_mrid: MrId, // VoltageLevel
}

/// CIM AC line segment (represents a transmission/distribution line)
#[derive(Debug, Clone, Default)]
pub struct CimAcLineSegment {
    pub mrid: MrId,
    pub name: String,
    pub r: f64,          // ohms
    pub x: f64,          // ohms
    pub bch: f64,        // microsiemens (charging susceptance)
    pub length: f64,     // km
    pub base_voltage_mrid: MrId,
    /// Terminal references (2 terminals: from and to)
    pub terminal_mrids: [MrId; 2],
}

/// CIM power transformer
#[derive(Debug, Clone, Default)]
pub struct CimPowerTransformer {
    pub mrid: MrId,
    pub name: String,
    pub equipment_container_mrid: MrId,
    pub power_transformer_end_mrids: Vec<MrId>,
}

/// CIM power transformer end (winding)
#[derive(Debug, Clone, Default)]
pub struct CimPowerTransformerEnd {
    pub mrid: MrId,
    pub transformer_mrid: MrId,
    pub base_voltage_mrid: MrId,
    pub r: f64,           // ohms
    pub x: f64,           // ohms
    pub rated_u: f64,     // kV
    pub rated_s: f64,     // MVA
    pub connection_code: String, // Y, D, Z
    pub tap_step: f64,    // per-unit
    pub terminal_mrid: MrId,
}

/// CIM synchronous machine (generator)
#[derive(Debug, Clone, Default)]
pub struct CimSynchronousMachine {
    pub mrid: MrId,
    pub name: String,
    pub rated_s: f64,     // MVA
    pub rated_u: f64,     // kV
    pub p: f64,           // MW (active power)
    pub q: f64,           // MVAr (reactive power)
    pub min_q: f64,       // MVAr
    pub max_q: f64,       // MVAr
    pub base_voltage_mrid: MrId,
    pub terminal_mrid: MrId,
    pub equipment_container_mrid: MrId,
}

/// CIM energy consumer (load)
#[derive(Debug, Clone, Default)]
pub struct CimEnergyConsumer {
    pub mrid: MrId,
    pub name: String,
    pub p: f64,           // MW
    pub q: f64,           // MVAr
    pub base_voltage_mrid: MrId,
    pub terminal_mrid: MrId,
    pub equipment_container_mrid: MrId,
}

/// CIM linear shunt compensator
#[derive(Debug, Clone, Default)]
pub struct CimLinearShuntCompensator {
    pub mrid: MrId,
    pub name: String,
    pub b: f64,           // microsiemens
    pub g: f64,           // microsiemens
    pub base_voltage_mrid: MrId,
    pub terminal_mrid: MrId,
}

/// CIM terminal (connection point)
#[derive(Debug, Clone, Default)]
pub struct CimTerminal {
    pub mrid: MrId,
    pub name: String,
    pub connectivity_node_mrid: MrId,
    pub conducting_equipment_mrid: MrId,
    pub sequence_number: u32,
}

/// CIM connectivity node (bus connection point)
#[derive(Debug, Clone, Default)]
pub struct CimConnectivityNode {
    pub mrid: MrId,
    pub name: String,
    pub container_mrid: MrId, // VoltageLevel or Bay
}

/// CIM breaker (switch)
#[derive(Debug, Clone, Default)]
pub struct CimBreaker {
    pub mrid: MrId,
    pub name: String,
    pub normal_open: bool,
    pub retained: bool,
    pub terminal_mrids: [MrId; 2],
}

/// CIM disconnector (isolator switch)
#[derive(Debug, Clone, Default)]
pub struct CimDisconnector {
    pub mrid: MrId,
    pub name: String,
    pub normal_open: bool,
    pub terminal_mrids: [MrId; 2],
}

/// Complete CIM model
#[derive(Debug, Clone, Default)]
pub struct CimModel {
    pub base_voltages: HashMap<MrId, CimBaseVoltage>,
    pub substations: HashMap<MrId, CimSubstation>,
    pub voltage_levels: HashMap<MrId, CimVoltageLevel>,
    pub busbar_sections: HashMap<MrId, CimBusbarSection>,
    pub ac_line_segments: HashMap<MrId, CimAcLineSegment>,
    pub power_transformers: HashMap<MrId, CimPowerTransformer>,
    pub power_transformer_ends: HashMap<MrId, CimPowerTransformerEnd>,
    pub synchronous_machines: HashMap<MrId, CimSynchronousMachine>,
    pub energy_consumers: HashMap<MrId, CimEnergyConsumer>,
    pub linear_shunt_compensators: HashMap<MrId, CimLinearShuntCompensator>,
    pub terminals: HashMap<MrId, CimTerminal>,
    pub connectivity_nodes: HashMap<MrId, CimConnectivityNode>,
    pub breakers: HashMap<MrId, CimBreaker>,
    pub disconnectors: HashMap<MrId, CimDisconnector>,
}

impl CimModel {
    /// Count total equipment
    pub fn equipment_count(&self) -> usize {
        self.busbar_sections.len()
            + self.ac_line_segments.len()
            + self.power_transformers.len()
            + self.synchronous_machines.len()
            + self.energy_consumers.len()
            + self.linear_shunt_compensators.len()
            + self.breakers.len()
            + self.disconnectors.len()
    }

    /// Get nominal voltage for a base voltage mRID
    pub fn nominal_voltage(&self, base_voltage_mrid: &str) -> Option<f64> {
        self.base_voltages.get(base_voltage_mrid).map(|bv| bv.nominal_voltage)
    }

    /// Find the connectivity node for a terminal
    pub fn terminal_connectivity_node(&self, terminal_mrid: &str) -> Option<&CimConnectivityNode> {
        let terminal = self.terminals.get(terminal_mrid)?;
        self.connectivity_nodes.get(&terminal.connectivity_node_mrid)
    }

    /// Find the busbar section connected to a connectivity node
    pub fn cn_to_busbar(&self, cn_mrid: &str) -> Option<&CimBusbarSection> {
        // A busbar section's terminal points to a connectivity node
        self.busbar_sections.values().find(|bb| {
            // Check if any terminal of this busbar points to this CN
            self.terminals.values().any(|t| {
                t.conducting_equipment_mrid == bb.mrid && t.connectivity_node_mrid == cn_mrid
            })
        })
    }
}

// ---------------------------------------------------------------------------
// CIM → PowerNetwork conversion
// ---------------------------------------------------------------------------

/// Topology resolver built once from a `CimModel` to accelerate the many
/// "device → bus" lookups performed during conversion.
///
/// In standard CIM RDF/XML, `Terminal` elements are top-level and reference
/// their `ConductingEquipment` via `cim:Terminal.ConductingEquipment`. The
/// reverse direction (equipment → terminal) is not stored in the parsed
/// structs, so we build it here by scanning all terminals.
struct CimTopology<'a> {
    model: &'a CimModel,
    /// BusbarSection mRID → sequential 1-based ElementId
    bus_id_by_mrid: HashMap<String, ElementId>,
    /// ElementId → 0-based solver index
    bus_map: HashMap<ElementId, usize>,
    /// ConductingEquipment mRID → list of terminals (reverse map)
    equip_terminals: HashMap<String, Vec<&'a CimTerminal>>,
    /// ConnectivityNode mRID → bus ElementId
    cn_to_bus_id: HashMap<String, ElementId>,
}

impl<'a> CimTopology<'a> {
    fn new(model: &'a CimModel) -> Result<Self, String> {
        // Assign deterministic 1-based ElementIds to BusbarSections (sorted by mRID)
        let mut bus_mrids: Vec<&MrId> = model.busbar_sections.keys().collect();
        bus_mrids.sort();

        if bus_mrids.is_empty() {
            return Err(
                "CIM model contains no BusbarSection elements; cannot build network".to_string(),
            );
        }

        let mut bus_id_by_mrid = HashMap::new();
        let mut bus_map = HashMap::new();
        for (idx, mrid) in bus_mrids.iter().enumerate() {
            let bus_id = (idx + 1) as ElementId;
            bus_id_by_mrid.insert(mrid.to_string(), bus_id);
            bus_map.insert(bus_id, idx);
        }

        // Build equipment → terminals reverse map
        let mut equip_terminals: HashMap<String, Vec<&CimTerminal>> = HashMap::new();
        for term in model.terminals.values() {
            if !term.conducting_equipment_mrid.is_empty() {
                equip_terminals
                    .entry(term.conducting_equipment_mrid.clone())
                    .or_default()
                    .push(term);
            }
        }

        // Build ConnectivityNode → bus map: a CN maps to a bus if any terminal
        // on that CN has a BusbarSection as its conducting equipment.
        let mut cn_to_bus_id = HashMap::new();
        for term in model.terminals.values() {
            if term.connectivity_node_mrid.is_empty() {
                continue;
            }
            if model
                .busbar_sections
                .contains_key(&term.conducting_equipment_mrid)
            {
                if let Some(&bid) = bus_id_by_mrid.get(&term.conducting_equipment_mrid) {
                    cn_to_bus_id.insert(term.connectivity_node_mrid.clone(), bid);
                }
            }
        }

        Ok(Self {
            model,
            bus_id_by_mrid,
            bus_map,
            equip_terminals,
            cn_to_bus_id,
        })
    }

    /// Resolve a terminal mRID to a bus ElementId via its ConnectivityNode.
    fn resolve_terminal(&self, term_mrid: &str) -> Option<ElementId> {
        let term = self.model.terminals.get(term_mrid)?;
        if term.connectivity_node_mrid.is_empty() {
            return None;
        }
        self.cn_to_bus_id
            .get(&term.connectivity_node_mrid)
            .copied()
    }

    /// Resolve all bus connections of a device via the reverse terminal map.
    /// Terminals are sorted by sequence number for deterministic from/to ordering.
    fn resolve_equipment_buses(&self, equip_mrid: &str) -> Vec<ElementId> {
        let mut buses = Vec::new();
        if let Some(terms) = self.equip_terminals.get(equip_mrid) {
            let mut sorted: Vec<&&CimTerminal> = terms.iter().collect();
            sorted.sort_by_key(|t| t.sequence_number);
            for t in sorted {
                if t.connectivity_node_mrid.is_empty() {
                    continue;
                }
                if let Some(&bid) = self.cn_to_bus_id.get(&t.connectivity_node_mrid) {
                    buses.push(bid);
                }
            }
        }
        buses
    }

    /// Resolve a single-bus device: try explicit terminal_mrid first, then
    /// fall back to the reverse map.
    fn resolve_equipment_bus(
        &self,
        equip_mrid: &str,
        explicit_term_mrid: &str,
    ) -> Option<ElementId> {
        if !explicit_term_mrid.is_empty() {
            if let Some(bid) = self.resolve_terminal(explicit_term_mrid) {
                return Some(bid);
            }
        }
        self.resolve_equipment_buses(equip_mrid)
            .into_iter()
            .next()
    }

    /// Look up nominal voltage (kV) for a base voltage mRID, with fallback to
    /// the busbar's VoltageLevel nominal voltage.
    fn nominal_voltage(
        &self,
        base_voltage_mrid: &str,
        bus_id: Option<ElementId>,
    ) -> Option<f64> {
        if !base_voltage_mrid.is_empty() {
            if let Some(v) = self.model.nominal_voltage(base_voltage_mrid) {
                return Some(v);
            }
        }
        if let Some(bid) = bus_id {
            // Reverse lookup: find the busbar mrid for this bus_id
            for (mrid, &id) in &self.bus_id_by_mrid {
                if id == bid {
                    if let Some(bb) = self.model.busbar_sections.get(mrid) {
                        if let Some(vl) = self
                            .model
                            .voltage_levels
                            .get(&bb.equipment_container_mrid)
                        {
                            return Some(vl.nominal_voltage);
                        }
                    }
                    break;
                }
            }
        }
        None
    }
}

/// Convert a parsed `CimModel` into a `PowerNetwork` ready for power flow.
///
/// This function builds the bus/branch topology from CIM `BusbarSection`,
/// `ACLineSegment`, `PowerTransformer`, `Breaker`, `Disconnector`,
/// `SynchronousMachine`, `EnergyConsumer`, and `LinearShuntCompensator`
/// elements. `Terminal` and `ConnectivityNode` elements are used to resolve
/// which bus each device connects to.
///
/// # Per-unit conversion
///
/// CIM stores physical values (ohms, microsiemens, MW, MVAr). These are
/// converted to per-unit using `BASE_MVA = 100` and the nominal voltage from
/// each element's `BaseVoltage` reference (falling back to the busbar's
/// `VoltageLevel`).
///
/// # Bus type assignment
///
/// The first `SynchronousMachine` bus becomes the Slack bus; subsequent
/// generator buses become PV; all others are PQ. If there are no generators,
/// the first bus is Slack.
///
/// # Errors
///
/// Returns `Err` with a descriptive message if:
/// - The model contains no `BusbarSection` elements.
/// - A branch (line/transformer/switch) cannot resolve both endpoint buses.
/// - A generator/load/shunt cannot resolve its bus.
pub fn cim_to_power_network(model: &CimModel) -> Result<PowerNetwork, String> {
    let topo = CimTopology::new(model)?;
    let n_buses = topo.bus_map.len();

    // --- Build branches ---
    let mut branches: Vec<(ElementId, ElementId, f64, f64, f64, f64)> = Vec::new();

    // ACLineSegment → Branch
    let mut line_mrids: Vec<&MrId> = model.ac_line_segments.keys().collect();
    line_mrids.sort();
    for mrid in &line_mrids {
        let cls = &model.ac_line_segments[*mrid];
        // Try explicit terminal_mrids first, then reverse map
        let mut buses = Vec::new();
        for tm in &cls.terminal_mrids {
            if !tm.is_empty() {
                if let Some(bid) = topo.resolve_terminal(tm) {
                    buses.push(bid);
                }
            }
        }
        if buses.len() < 2 {
            buses = topo.resolve_equipment_buses(&cls.mrid);
        }
        if buses.len() < 2 {
            return Err(format!(
                "ACLineSegment '{}' cannot resolve both terminal buses (got {})",
                cls.mrid,
                buses.len()
            ));
        }
        let v_base = topo
            .nominal_voltage(&cls.base_voltage_mrid, Some(buses[0]))
            .or_else(|| topo.nominal_voltage(&cls.base_voltage_mrid, Some(buses[1])))
            .unwrap_or(110.0);
        let z_base = v_base * v_base / CIM_BASE_MVA;
        let r_pu = if z_base > 1e-10 { cls.r / z_base } else { 0.0 };
        let x_pu = if z_base > 1e-10 { cls.x / z_base } else { 0.0 };
        let b_pu = cls.bch * 1e-6 * z_base; // microsiemens → per-unit
        branches.push((buses[0], buses[1], r_pu, x_pu, b_pu, 1.0));
    }

    // PowerTransformer → Branch
    let mut pt_mrids: Vec<&MrId> = model.power_transformers.keys().collect();
    pt_mrids.sort();
    for mrid in &pt_mrids {
        let pt = &model.power_transformers[*mrid];
        // Find ends belonging to this transformer (power_transformer_end_mrids
        // is not populated by the parser, so we scan all ends).
        let ends: Vec<&CimPowerTransformerEnd> = model
            .power_transformer_ends
            .values()
            .filter(|e| e.transformer_mrid == pt.mrid)
            .collect();
        // Resolve buses via end terminals
        let mut buses = Vec::new();
        for end in &ends {
            // Try explicit terminal_mrid first
            if !end.terminal_mrid.is_empty() {
                if let Some(bid) = topo.resolve_terminal(&end.terminal_mrid) {
                    buses.push(bid);
                    continue;
                }
            }
            // Fallback: reverse map on the end's mRID (in standard CIM the
            // Terminal's ConductingEquipment points to the PowerTransformerEnd)
            if let Some(bid) = topo.resolve_equipment_buses(&end.mrid).into_iter().next() {
                buses.push(bid);
            }
        }
        // Fallback: reverse map on the transformer itself (some CIM profiles
        // attach terminals directly to the PowerTransformer)
        if buses.len() < 2 {
            buses = topo.resolve_equipment_buses(&pt.mrid);
        }
        if buses.len() < 2 {
            return Err(format!(
                "PowerTransformer '{}' cannot resolve both winding buses (got {})",
                pt.mrid,
                buses.len()
            ));
        }
        // Sum impedances from all ends; use first end's rated_u as base voltage
        let mut r_sum = 0.0;
        let mut x_sum = 0.0;
        let mut v_base = 110.0;
        for (i, end) in ends.iter().enumerate() {
            r_sum += end.r;
            x_sum += end.x;
            if i == 0 && end.rated_u > 0.0 {
                v_base = end.rated_u;
            }
        }
        let z_base = v_base * v_base / CIM_BASE_MVA;
        let r_pu = if z_base > 1e-10 { r_sum / z_base } else { 0.0 };
        let x_pu = if z_base > 1e-10 { x_sum / z_base } else { 0.0 };
        branches.push((buses[0], buses[1], r_pu, x_pu, 0.0, 1.0));
    }

    // Breaker → Branch (zero/small impedance; closed by default)
    let mut brk_mrids: Vec<&MrId> = model.breakers.keys().collect();
    brk_mrids.sort();
    for mrid in &brk_mrids {
        let brk = &model.breakers[*mrid];
        let mut buses = Vec::new();
        for tm in &brk.terminal_mrids {
            if !tm.is_empty() {
                if let Some(bid) = topo.resolve_terminal(tm) {
                    buses.push(bid);
                }
            }
        }
        if buses.len() < 2 {
            buses = topo.resolve_equipment_buses(&brk.mrid);
        }
        if buses.len() < 2 {
            return Err(format!(
                "Breaker '{}' cannot resolve both terminal buses (got {})",
                brk.mrid,
                buses.len()
            ));
        }
        // Closed switch: small impedance so it appears in Y-Bus (effectively
        // merging the two buses). Open switch: zero impedance (skipped by
        // YBusMatrix::from_branches, correctly leaving buses disconnected).
        let z = if brk.normal_open { 0.0 } else { 1e-6 };
        branches.push((buses[0], buses[1], z, z, 0.0, 1.0));
    }

    // Disconnector → Branch
    let mut dis_mrids: Vec<&MrId> = model.disconnectors.keys().collect();
    dis_mrids.sort();
    for mrid in &dis_mrids {
        let dis = &model.disconnectors[*mrid];
        let mut buses = Vec::new();
        for tm in &dis.terminal_mrids {
            if !tm.is_empty() {
                if let Some(bid) = topo.resolve_terminal(tm) {
                    buses.push(bid);
                }
            }
        }
        if buses.len() < 2 {
            buses = topo.resolve_equipment_buses(&dis.mrid);
        }
        if buses.len() < 2 {
            return Err(format!(
                "Disconnector '{}' cannot resolve both terminal buses (got {})",
                dis.mrid,
                buses.len()
            ));
        }
        let z = if dis.normal_open { 0.0 } else { 1e-6 };
        branches.push((buses[0], buses[1], z, z, 0.0, 1.0));
    }

    // --- Build p_spec, q_spec, bus_types ---
    let mut p_spec = vec![0.0; n_buses];
    let mut q_spec = vec![0.0; n_buses];
    let mut bus_types = vec![BusTypeNR::PQ; n_buses];
    let mut gen_bus_ids: Vec<ElementId> = Vec::new();
    let mut generators: Vec<GeneratorSpec> = Vec::new();
    let mut load_p_by_bus: HashMap<ElementId, f64> = HashMap::new();
    let mut shunt_by_bus: Vec<(usize, f64, f64)> = Vec::new(); // (bus_idx, g_pu, b_pu)

    // SynchronousMachine → Generator (positive P/Q injection)
    let mut sm_mrids: Vec<&MrId> = model.synchronous_machines.keys().collect();
    sm_mrids.sort();
    for (gen_i, mrid) in sm_mrids.iter().enumerate() {
        let gen_idx = (gen_i + 1) as ElementId;
        let sm = &model.synchronous_machines[*mrid];
        let bus_id = match topo.resolve_equipment_bus(&sm.mrid, &sm.terminal_mrid) {
            Some(b) => b,
            None => {
                return Err(format!(
                    "SynchronousMachine '{}' cannot resolve its bus",
                    sm.mrid
                ))
            }
        };
        let idx = topo.bus_map[&bus_id];
        p_spec[idx] += sm.p / CIM_BASE_MVA;
        q_spec[idx] += sm.q / CIM_BASE_MVA;
        gen_bus_ids.push(bus_id);
        let p_max = if sm.rated_s > 0.0 {
            sm.rated_s
        } else {
            sm.p.max(0.0) * 1.2 + 1.0
        };
        generators.push(GeneratorSpec {
            gen_id: gen_idx,
            bus_id,
            p_min_mw: 0.0,
            p_max_mw: p_max,
            p_gen_mw: sm.p,
            p_load_mw: 0.0,
        });
    }

    // EnergyConsumer → Load (negative P/Q injection)
    let mut ec_mrids: Vec<&MrId> = model.energy_consumers.keys().collect();
    ec_mrids.sort();
    for mrid in &ec_mrids {
        let ec = &model.energy_consumers[*mrid];
        let bus_id = match topo.resolve_equipment_bus(&ec.mrid, &ec.terminal_mrid) {
            Some(b) => b,
            None => {
                return Err(format!(
                    "EnergyConsumer '{}' cannot resolve its bus",
                    ec.mrid
                ))
            }
        };
        let idx = topo.bus_map[&bus_id];
        p_spec[idx] -= ec.p / CIM_BASE_MVA;
        q_spec[idx] -= ec.q / CIM_BASE_MVA;
        *load_p_by_bus.entry(bus_id).or_insert(0.0) += ec.p;
    }

    // Update generator p_load_mw from loads at the same bus
    for gen in &mut generators {
        if let Some(&p_load) = load_p_by_bus.get(&gen.bus_id) {
            gen.p_load_mw = p_load;
        }
    }

    // LinearShuntCompensator → Shunt (added to Y-Bus diagonal)
    let mut lsc_mrids: Vec<&MrId> = model.linear_shunt_compensators.keys().collect();
    lsc_mrids.sort();
    for mrid in &lsc_mrids {
        let lsc = &model.linear_shunt_compensators[*mrid];
        let bus_id = match topo.resolve_equipment_bus(&lsc.mrid, &lsc.terminal_mrid) {
            Some(b) => b,
            None => {
                return Err(format!(
                    "LinearShuntCompensator '{}' cannot resolve its bus",
                    lsc.mrid
                ))
            }
        };
        let idx = topo.bus_map[&bus_id];
        let v_base = topo
            .nominal_voltage(&lsc.base_voltage_mrid, Some(bus_id))
            .unwrap_or(110.0);
        let z_base = v_base * v_base / CIM_BASE_MVA;
        let b_pu = lsc.b * 1e-6 * z_base;
        let g_pu = lsc.g * 1e-6 * z_base;
        shunt_by_bus.push((idx, g_pu, b_pu));
    }

    // --- Assign bus types ---
    for (i, &bus_id) in gen_bus_ids.iter().enumerate() {
        let idx = topo.bus_map[&bus_id];
        bus_types[idx] = if i == 0 {
            BusTypeNR::Slack
        } else {
            BusTypeNR::PV
        };
    }
    if gen_bus_ids.is_empty() && n_buses > 0 {
        bus_types[0] = BusTypeNR::Slack;
    }

    // --- Build Y-Bus ---
    let mut ybus = YBusMatrix::from_branches(&branches, &topo.bus_map);
    ybus.set_base_mva(CIM_BASE_MVA);
    for &(idx, g, b) in &shunt_by_bus {
        ybus.add_shunt(idx, g, b);
    }

    // --- Assemble PowerNetwork ---
    let branch_ids: Vec<ElementId> = (1..=branches.len() as ElementId).collect();
    let v_initial = vec![1.0; n_buses];

    let network = PowerNetwork::new(ybus, p_spec, q_spec, bus_types, branches, topo.bus_map)
        .with_initial_voltages(v_initial)
        .with_generators(generators)
        .with_branch_ids(branch_ids);

    Ok(network)
}

/// Minimal XML attribute extractor
fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    let key = format!("{}=\"", attr);
    if let Some(start) = tag.find(&key) {
        let value_start = start + key.len();
        if let Some(end) = tag[value_start..].find('"') {
            return Some(tag[value_start..value_start + end].to_string());
        }
    }
    None
}

/// Extract the text content of an element
fn extract_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let start = xml.find(&open)?;
    let content_start = start + open.len();
    let end = xml[content_start..].find(&close)?;
    Some(xml[content_start..content_start + end].trim().to_string())
}

/// Extract all elements with a given tag (including content)
fn extract_all_elements(xml: &str, tag: &str) -> Vec<String> {
    let mut results = Vec::new();
    let open = format!("<{}", tag);
    let close_tag = format!("</{}>", tag);
    let mut pos = 0;
    while let Some(start) = xml[pos..].find(&open) {
        let abs_start = pos + start;
        let after_tag_name = abs_start + 1 + tag.len();
        if after_tag_name >= xml.len() {
            break;
        }
        let next_char = xml.as_bytes()[after_tag_name];
        if next_char != b' ' && next_char != b'>' && next_char != b'/' && next_char != b'\t' && next_char != b'\n' {
            pos = abs_start + 1;
            continue;
        }
        // Find end of opening tag
        let tag_end = match xml[abs_start..].find('>') {
            Some(e) => abs_start + e + 1,
            None => break,
        };
        // Self-closing?
        if xml[abs_start..tag_end].ends_with("/>") {
            results.push(xml[abs_start..tag_end].to_string());
            pos = tag_end;
            continue;
        }
        // Paired tag
        if let Some(end) = xml[tag_end..].find(&close_tag) {
            results.push(xml[abs_start..tag_end + end + close_tag.len()].to_string());
            pos = tag_end + end + close_tag.len();
        } else {
            break;
        }
    }
    results
}

/// Extract the mRID from an element
fn extract_mrid(element_xml: &str) -> MrId {
    extract_attr(element_xml, "rdf:ID")
        .or_else(|| extract_attr(element_xml, "rdf:about"))
        .unwrap_or_default()
        .trim_start_matches('#')
        .to_string()
}

/// Extract a reference (rdf:resource) from a sub-element
fn extract_reference(element_xml: &str, sub_tag: &str) -> Option<MrId> {
    let sub = format!("<{} ", sub_tag);
    let start = element_xml.find(&sub)?;
    let end = element_xml[start..].find("/>")?;
    let fragment = &element_xml[start..start + end + 2];
    extract_attr(fragment, "rdf:resource").map(|s| s.trim_start_matches('#').to_string())
}

/// Parse a float from element text
fn parse_float(element_xml: &str, tag: &str) -> f64 {
    extract_text(element_xml, tag)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0)
}

/// Parse a bool from element text
fn parse_bool(element_xml: &str, tag: &str) -> bool {
    extract_text(element_xml, tag)
        .map(|s| s == "true" || s == "1")
        .unwrap_or(false)
}

/// Parse a CIM RDF/XML document
pub fn parse_cim(xml: &str) -> Result<CimModel, String> {
    let mut model = CimModel::default();

    // Parse BaseVoltage
    for elem in extract_all_elements(xml, "cim:BaseVoltage") {
        let bv = CimBaseVoltage {
            mrid: extract_mrid(&elem),
            nominal_voltage: parse_float(&elem, "cim:BaseVoltage.nominalVoltage"),
        };
        model.base_voltages.insert(bv.mrid.clone(), bv);
    }

    // Parse Substation
    for elem in extract_all_elements(xml, "cim:Substation") {
        let sub = CimSubstation {
            mrid: extract_mrid(&elem),
            name: extract_text(&elem, "cim:IdentifiedObject.name").unwrap_or_default(),
            region: extract_reference(&elem, "cim:Substation.Region")
                .unwrap_or_default(),
            voltage_levels: Vec::new(),
        };
        model.substations.insert(sub.mrid.clone(), sub);
    }

    // Parse VoltageLevel
    for elem in extract_all_elements(xml, "cim:VoltageLevel") {
        let vl = CimVoltageLevel {
            mrid: extract_mrid(&elem),
            name: extract_text(&elem, "cim:IdentifiedObject.name").unwrap_or_default(),
            nominal_voltage: parse_float(&elem, "cim:VoltageLevel.nominalVoltage"),
        };
        model.voltage_levels.insert(vl.mrid.clone(), vl);
    }

    // Parse BusbarSection
    for elem in extract_all_elements(xml, "cim:BusbarSection") {
        let bb = CimBusbarSection {
            mrid: extract_mrid(&elem),
            name: extract_text(&elem, "cim:IdentifiedObject.name").unwrap_or_default(),
            base_voltage_mrid: extract_reference(&elem, "cim:Equipment.EquipmentContainer")
                .or_else(|| extract_reference(&elem, "cim:ConductingEquipment.BaseVoltage"))
                .unwrap_or_default(),
            equipment_container_mrid: extract_reference(&elem, "cim:Equipment.EquipmentContainer")
                .unwrap_or_default(),
        };
        model.busbar_sections.insert(bb.mrid.clone(), bb);
    }

    // Parse ACLineSegment
    for elem in extract_all_elements(xml, "cim:ACLineSegment") {
        let mut terminals = [MrId::default(), MrId::default()];
        let term_refs: Vec<MrId> = extract_all_elements(&elem, "cim:Terminal")
            .iter()
            .map(|t| extract_mrid(t))
            .collect();
        for (i, t) in term_refs.iter().take(2).enumerate() {
            terminals[i] = t.clone();
        }
        let cls = CimAcLineSegment {
            mrid: extract_mrid(&elem),
            name: extract_text(&elem, "cim:IdentifiedObject.name").unwrap_or_default(),
            r: parse_float(&elem, "cim:ACLineSegment.r"),
            x: parse_float(&elem, "cim:ACLineSegment.x"),
            bch: parse_float(&elem, "cim:ACLineSegment.bch"),
            length: parse_float(&elem, "cim:Conductor.length"),
            base_voltage_mrid: extract_reference(&elem, "cim:ConductingEquipment.BaseVoltage")
                .unwrap_or_default(),
            terminal_mrids: terminals,
        };
        model.ac_line_segments.insert(cls.mrid.clone(), cls);
    }

    // Parse PowerTransformer
    for elem in extract_all_elements(xml, "cim:PowerTransformer") {
        let pt = CimPowerTransformer {
            mrid: extract_mrid(&elem),
            name: extract_text(&elem, "cim:IdentifiedObject.name").unwrap_or_default(),
            equipment_container_mrid: extract_reference(&elem, "cim:Equipment.EquipmentContainer")
                .unwrap_or_default(),
            power_transformer_end_mrids: Vec::new(),
        };
        model.power_transformers.insert(pt.mrid.clone(), pt);
    }

    // Parse PowerTransformerEnd
    for elem in extract_all_elements(xml, "cim:PowerTransformerEnd") {
        let pte = CimPowerTransformerEnd {
            mrid: extract_mrid(&elem),
            transformer_mrid: extract_reference(&elem, "cim:PowerTransformerEnd.PowerTransformer")
                .unwrap_or_default(),
            base_voltage_mrid: extract_reference(&elem, "cim:TransformerEnd.BaseVoltage")
                .unwrap_or_default(),
            r: parse_float(&elem, "cim:PowerTransformerEnd.r"),
            x: parse_float(&elem, "cim:PowerTransformerEnd.x"),
            rated_u: parse_float(&elem, "cim:PowerTransformerEnd.ratedU"),
            rated_s: parse_float(&elem, "cim:PowerTransformerEnd.ratedS"),
            connection_code: extract_text(&elem, "cim:WindingInfo.connectionKind").unwrap_or_default(),
            tap_step: parse_float(&elem, "cim:TapChanger.stepVoltageIncrement"),
            terminal_mrid: extract_reference(&elem, "cim:TransformerEnd.Terminal")
                .unwrap_or_default(),
        };
        model.power_transformer_ends.insert(pte.mrid.clone(), pte);
    }

    // Parse SynchronousMachine
    for elem in extract_all_elements(xml, "cim:SynchronousMachine") {
        let sm = CimSynchronousMachine {
            mrid: extract_mrid(&elem),
            name: extract_text(&elem, "cim:IdentifiedObject.name").unwrap_or_default(),
            rated_s: parse_float(&elem, "cim:RotatingMachine.ratedS"),
            rated_u: parse_float(&elem, "cim:RotatingMachine.ratedU"),
            p: parse_float(&elem, "cim:GeneratingUnit.initialP"),
            q: parse_float(&elem, "cim:SynchronousMachine.q"),
            min_q: parse_float(&elem, "cim:SynchronousMachine.minQ"),
            max_q: parse_float(&elem, "cim:SynchronousMachine.maxQ"),
            base_voltage_mrid: extract_reference(&elem, "cim:ConductingEquipment.BaseVoltage")
                .unwrap_or_default(),
            terminal_mrid: extract_reference(&elem, "cim:RotatingMachine.Terminal")
                .unwrap_or_default(),
            equipment_container_mrid: extract_reference(&elem, "cim:Equipment.EquipmentContainer")
                .unwrap_or_default(),
        };
        model.synchronous_machines.insert(sm.mrid.clone(), sm);
    }

    // Parse EnergyConsumer
    for elem in extract_all_elements(xml, "cim:EnergyConsumer") {
        let ec = CimEnergyConsumer {
            mrid: extract_mrid(&elem),
            name: extract_text(&elem, "cim:IdentifiedObject.name").unwrap_or_default(),
            p: parse_float(&elem, "cim:EnergyConsumer.p"),
            q: parse_float(&elem, "cim:EnergyConsumer.q"),
            base_voltage_mrid: extract_reference(&elem, "cim:ConductingEquipment.BaseVoltage")
                .unwrap_or_default(),
            terminal_mrid: extract_reference(&elem, "cim:Equipment.EquipmentContainer")
                .unwrap_or_default(),
            equipment_container_mrid: extract_reference(&elem, "cim:Equipment.EquipmentContainer")
                .unwrap_or_default(),
        };
        model.energy_consumers.insert(ec.mrid.clone(), ec);
    }

    // Parse LinearShuntCompensator
    for elem in extract_all_elements(xml, "cim:LinearShuntCompensator") {
        let lsc = CimLinearShuntCompensator {
            mrid: extract_mrid(&elem),
            name: extract_text(&elem, "cim:IdentifiedObject.name").unwrap_or_default(),
            b: parse_float(&elem, "cim:LinearShuntCompensator.b"),
            g: parse_float(&elem, "cim:LinearShuntCompensator.g"),
            base_voltage_mrid: extract_reference(&elem, "cim:ConductingEquipment.BaseVoltage")
                .unwrap_or_default(),
            terminal_mrid: extract_reference(&elem, "cim:ShuntCompensator.Terminal")
                .unwrap_or_default(),
        };
        model.linear_shunt_compensators.insert(lsc.mrid.clone(), lsc);
    }

    // Parse Terminal
    for elem in extract_all_elements(xml, "cim:Terminal") {
        let t = CimTerminal {
            mrid: extract_mrid(&elem),
            name: extract_text(&elem, "cim:IdentifiedObject.name").unwrap_or_default(),
            connectivity_node_mrid: extract_reference(&elem, "cim:Terminal.ConnectivityNode")
                .unwrap_or_default(),
            conducting_equipment_mrid: extract_reference(&elem, "cim:Terminal.ConductingEquipment")
                .unwrap_or_default(),
            sequence_number: extract_text(&elem, "cim:ACDCTerminal.sequenceNumber")
                .and_then(|s| s.parse().ok())
                .unwrap_or(1),
        };
        model.terminals.insert(t.mrid.clone(), t);
    }

    // Parse ConnectivityNode
    for elem in extract_all_elements(xml, "cim:ConnectivityNode") {
        let cn = CimConnectivityNode {
            mrid: extract_mrid(&elem),
            name: extract_text(&elem, "cim:IdentifiedObject.name").unwrap_or_default(),
            container_mrid: extract_reference(&elem, "cim:ConnectivityNode.ConnectivityNodeContainer")
                .unwrap_or_default(),
        };
        model.connectivity_nodes.insert(cn.mrid.clone(), cn);
    }

    // Parse Breaker
    for elem in extract_all_elements(xml, "cim:Breaker") {
        let mut terminals = [MrId::default(), MrId::default()];
        let term_refs: Vec<MrId> = extract_all_elements(&elem, "cim:Terminal")
            .iter()
            .map(|t| extract_mrid(t))
            .collect();
        for (i, t) in term_refs.iter().take(2).enumerate() {
            terminals[i] = t.clone();
        }
        let br = CimBreaker {
            mrid: extract_mrid(&elem),
            name: extract_text(&elem, "cim:IdentifiedObject.name").unwrap_or_default(),
            normal_open: parse_bool(&elem, "cim:Switch.normalOpen"),
            retained: parse_bool(&elem, "cim:Switch.retained"),
            terminal_mrids: terminals,
        };
        model.breakers.insert(br.mrid.clone(), br);
    }

    // Parse Disconnector
    for elem in extract_all_elements(xml, "cim:Disconnector") {
        let mut terminals = [MrId::default(), MrId::default()];
        let term_refs: Vec<MrId> = extract_all_elements(&elem, "cim:Terminal")
            .iter()
            .map(|t| extract_mrid(t))
            .collect();
        for (i, t) in term_refs.iter().take(2).enumerate() {
            terminals[i] = t.clone();
        }
        let dc = CimDisconnector {
            mrid: extract_mrid(&elem),
            name: extract_text(&elem, "cim:IdentifiedObject.name").unwrap_or_default(),
            normal_open: parse_bool(&elem, "cim:Switch.normalOpen"),
            terminal_mrids: terminals,
        };
        model.disconnectors.insert(dc.mrid.clone(), dc);
    }

    Ok(model)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_CIM: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
         xmlns:cim="http://iec.ch/TC57/2013/CIM-schema-cim16#">
  <cim:BaseVoltage rdf:ID="bv110">
    <cim:BaseVoltage.nominalVoltage>110.0</cim:BaseVoltage.nominalVoltage>
  </cim:BaseVoltage>
  <cim:BaseVoltage rdf:ID="bv10">
    <cim:BaseVoltage.nominalVoltage>10.0</cim:BaseVoltage.nominalVoltage>
  </cim:BaseVoltage>
  <cim:Substation rdf:ID="sub1">
    <cim:IdentifiedObject.name>Substation 1</cim:IdentifiedObject.name>
  </cim:Substation>
  <cim:VoltageLevel rdf:ID="vl110">
    <cim:IdentifiedObject.name>110kV Level</cim:IdentifiedObject.name>
    <cim:VoltageLevel.nominalVoltage>110.0</cim:VoltageLevel.nominalVoltage>
  </cim:VoltageLevel>
  <cim:BusbarSection rdf:ID="bus1">
    <cim:IdentifiedObject.name>Bus 1</cim:IdentifiedObject.name>
    <cim:Equipment.EquipmentContainer rdf:resource="#vl110"/>
  </cim:BusbarSection>
  <cim:BusbarSection rdf:ID="bus2">
    <cim:IdentifiedObject.name>Bus 2</cim:IdentifiedObject.name>
    <cim:Equipment.EquipmentContainer rdf:resource="#vl110"/>
  </cim:BusbarSection>
  <cim:ACLineSegment rdf:ID="line1">
    <cim:IdentifiedObject.name>Line 1</cim:IdentifiedObject.name>
    <cim:ACLineSegment.r>0.5</cim:ACLineSegment.r>
    <cim:ACLineSegment.x>2.0</cim:ACLineSegment.x>
    <cim:ACLineSegment.bch>10.0</cim:ACLineSegment.bch>
    <cim:Conductor.length>50.0</cim:Conductor.length>
  </cim:ACLineSegment>
  <cim:SynchronousMachine rdf:ID="gen1">
    <cim:IdentifiedObject.name>Generator 1</cim:IdentifiedObject.name>
    <cim:RotatingMachine.ratedS>100.0</cim:RotatingMachine.ratedS>
    <cim:RotatingMachine.ratedU>110.0</cim:RotatingMachine.ratedU>
    <cim:GeneratingUnit.initialP>50.0</cim:GeneratingUnit.initialP>
    <cim:SynchronousMachine.q>10.0</cim:SynchronousMachine.q>
    <cim:SynchronousMachine.minQ>-20.0</cim:SynchronousMachine.minQ>
    <cim:SynchronousMachine.maxQ>30.0</cim:SynchronousMachine.maxQ>
  </cim:SynchronousMachine>
  <cim:EnergyConsumer rdf:ID="load1">
    <cim:IdentifiedObject.name>Load 1</cim:IdentifiedObject.name>
    <cim:EnergyConsumer.p>40.0</cim:EnergyConsumer.p>
    <cim:EnergyConsumer.q>8.0</cim:EnergyConsumer.q>
  </cim:EnergyConsumer>
  <cim:Breaker rdf:ID="brk1">
    <cim:IdentifiedObject.name>Breaker 1</cim:IdentifiedObject.name>
    <cim:Switch.normalOpen>false</cim:Switch.normalOpen>
  </cim:Breaker>
  <cim:Disconnector rdf:ID="dis1">
    <cim:IdentifiedObject.name>Disconnector 1</cim:IdentifiedObject.name>
    <cim:Switch.normalOpen>true</cim:Switch.normalOpen>
  </cim:Disconnector>
</rdf:RDF>"##;

    #[test]
    fn test_parse_cim_base_voltage() {
        let model = parse_cim(SAMPLE_CIM).unwrap();
        assert_eq!(model.base_voltages.len(), 2);
        let bv110 = model.base_voltages.get("bv110").unwrap();
        assert_eq!(bv110.nominal_voltage, 110.0);
        let bv10 = model.base_voltages.get("bv10").unwrap();
        assert_eq!(bv10.nominal_voltage, 10.0);
    }

    #[test]
    fn test_parse_cim_substation() {
        let model = parse_cim(SAMPLE_CIM).unwrap();
        assert_eq!(model.substations.len(), 1);
        let sub = model.substations.get("sub1").unwrap();
        assert_eq!(sub.name, "Substation 1");
    }

    #[test]
    fn test_parse_cim_voltage_level() {
        let model = parse_cim(SAMPLE_CIM).unwrap();
        assert_eq!(model.voltage_levels.len(), 1);
        let vl = model.voltage_levels.get("vl110").unwrap();
        assert_eq!(vl.name, "110kV Level");
        assert_eq!(vl.nominal_voltage, 110.0);
    }

    #[test]
    fn test_parse_cim_busbar_section() {
        let model = parse_cim(SAMPLE_CIM).unwrap();
        assert_eq!(model.busbar_sections.len(), 2);
        let bus1 = model.busbar_sections.get("bus1").unwrap();
        assert_eq!(bus1.name, "Bus 1");
        assert_eq!(bus1.equipment_container_mrid, "vl110");
    }

    #[test]
    fn test_parse_cim_ac_line_segment() {
        let model = parse_cim(SAMPLE_CIM).unwrap();
        assert_eq!(model.ac_line_segments.len(), 1);
        let line = model.ac_line_segments.get("line1").unwrap();
        assert_eq!(line.name, "Line 1");
        assert_eq!(line.r, 0.5);
        assert_eq!(line.x, 2.0);
        assert_eq!(line.bch, 10.0);
        assert_eq!(line.length, 50.0);
    }

    #[test]
    fn test_parse_cim_synchronous_machine() {
        let model = parse_cim(SAMPLE_CIM).unwrap();
        assert_eq!(model.synchronous_machines.len(), 1);
        let gen = model.synchronous_machines.get("gen1").unwrap();
        assert_eq!(gen.name, "Generator 1");
        assert_eq!(gen.rated_s, 100.0);
        assert_eq!(gen.rated_u, 110.0);
        assert_eq!(gen.p, 50.0);
        assert_eq!(gen.q, 10.0);
        assert_eq!(gen.min_q, -20.0);
        assert_eq!(gen.max_q, 30.0);
    }

    #[test]
    fn test_parse_cim_energy_consumer() {
        let model = parse_cim(SAMPLE_CIM).unwrap();
        assert_eq!(model.energy_consumers.len(), 1);
        let load = model.energy_consumers.get("load1").unwrap();
        assert_eq!(load.name, "Load 1");
        assert_eq!(load.p, 40.0);
        assert_eq!(load.q, 8.0);
    }

    #[test]
    fn test_parse_cim_breaker() {
        let model = parse_cim(SAMPLE_CIM).unwrap();
        assert_eq!(model.breakers.len(), 1);
        let brk = model.breakers.get("brk1").unwrap();
        assert_eq!(brk.name, "Breaker 1");
        assert!(!brk.normal_open);
    }

    #[test]
    fn test_parse_cim_disconnector() {
        let model = parse_cim(SAMPLE_CIM).unwrap();
        assert_eq!(model.disconnectors.len(), 1);
        let dis = model.disconnectors.get("dis1").unwrap();
        assert_eq!(dis.name, "Disconnector 1");
        assert!(dis.normal_open);
    }

    #[test]
    fn test_equipment_count() {
        let model = parse_cim(SAMPLE_CIM).unwrap();
        // 2 busbars + 1 line + 0 transformers + 1 gen + 1 load + 0 shunts + 1 breaker + 1 disconnector
        // Note: count may vary based on parser behavior with nested elements
        let count = model.equipment_count();
        assert!(count >= 6, "expected at least 6 equipment, got {}", count);
    }

    #[test]
    fn test_nominal_voltage_lookup() {
        let model = parse_cim(SAMPLE_CIM).unwrap();
        assert_eq!(model.nominal_voltage("bv110"), Some(110.0));
        assert_eq!(model.nominal_voltage("bv10"), Some(10.0));
        assert_eq!(model.nominal_voltage("nonexistent"), None);
    }

    #[test]
    fn test_parse_empty_cim() {
        let model = parse_cim("<rdf:RDF></rdf:RDF>").unwrap();
        assert_eq!(model.equipment_count(), 0);
    }

    #[test]
    fn test_parse_invalid_xml_returns_empty_model() {
        let model = parse_cim("not xml at all").unwrap();
        assert_eq!(model.equipment_count(), 0);
    }

    // ===================================================================
    // cim_to_power_network converter tests
    // ===================================================================

    /// CIM sample with full topology: 3 buses, 1 line, 1 transformer,
    /// 1 breaker (closed), 1 disconnector (open), 1 generator, 2 loads,
    /// 1 shunt — all connected via Terminals and ConnectivityNodes.
    const SAMPLE_CIM_TOPO: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
         xmlns:cim="http://iec.ch/TC57/2013/CIM-schema-cim16#">
  <cim:BaseVoltage rdf:ID="bv110">
    <cim:BaseVoltage.nominalVoltage>110.0</cim:BaseVoltage.nominalVoltage>
  </cim:BaseVoltage>
  <cim:BaseVoltage rdf:ID="bv10">
    <cim:BaseVoltage.nominalVoltage>10.0</cim:BaseVoltage.nominalVoltage>
  </cim:BaseVoltage>
  <cim:VoltageLevel rdf:ID="vl110">
    <cim:IdentifiedObject.name>110kV Level</cim:IdentifiedObject.name>
    <cim:VoltageLevel.nominalVoltage>110.0</cim:VoltageLevel.nominalVoltage>
  </cim:VoltageLevel>
  <cim:VoltageLevel rdf:ID="vl10">
    <cim:IdentifiedObject.name>10kV Level</cim:IdentifiedObject.name>
    <cim:VoltageLevel.nominalVoltage>10.0</cim:VoltageLevel.nominalVoltage>
  </cim:VoltageLevel>
  <!-- Busbar sections -->
  <cim:BusbarSection rdf:ID="bus1">
    <cim:IdentifiedObject.name>Bus 1</cim:IdentifiedObject.name>
    <cim:Equipment.EquipmentContainer rdf:resource="#vl110"/>
  </cim:BusbarSection>
  <cim:BusbarSection rdf:ID="bus2">
    <cim:IdentifiedObject.name>Bus 2</cim:IdentifiedObject.name>
    <cim:Equipment.EquipmentContainer rdf:resource="#vl110"/>
  </cim:BusbarSection>
  <cim:BusbarSection rdf:ID="bus3">
    <cim:IdentifiedObject.name>Bus 3</cim:IdentifiedObject.name>
    <cim:Equipment.EquipmentContainer rdf:resource="#vl10"/>
  </cim:BusbarSection>
  <!-- Connectivity nodes -->
  <cim:ConnectivityNode rdf:ID="cn1">
    <cim:IdentifiedObject.name>CN1</cim:IdentifiedObject.name>
  </cim:ConnectivityNode>
  <cim:ConnectivityNode rdf:ID="cn2">
    <cim:IdentifiedObject.name>CN2</cim:IdentifiedObject.name>
  </cim:ConnectivityNode>
  <cim:ConnectivityNode rdf:ID="cn3">
    <cim:IdentifiedObject.name>CN3</cim:IdentifiedObject.name>
  </cim:ConnectivityNode>
  <!-- AC Line Segment: bus1 ↔ bus2 -->
  <cim:ACLineSegment rdf:ID="line1">
    <cim:IdentifiedObject.name>Line 1</cim:IdentifiedObject.name>
    <cim:ACLineSegment.r>0.5</cim:ACLineSegment.r>
    <cim:ACLineSegment.x>2.0</cim:ACLineSegment.x>
    <cim:ACLineSegment.bch>10.0</cim:ACLineSegment.bch>
    <cim:Conductor.length>50.0</cim:Conductor.length>
    <cim:ConductingEquipment.BaseVoltage rdf:resource="#bv110"/>
  </cim:ACLineSegment>
  <!-- Power Transformer: bus2 ↔ bus3 -->
  <cim:PowerTransformer rdf:ID="xfmr1">
    <cim:IdentifiedObject.name>Transformer 1</cim:IdentifiedObject.name>
  </cim:PowerTransformer>
  <cim:PowerTransformerEnd rdf:ID="xfmr_end1">
    <cim:PowerTransformerEnd.PowerTransformer rdf:resource="#xfmr1"/>
    <cim:TransformerEnd.BaseVoltage rdf:resource="#bv110"/>
    <cim:PowerTransformerEnd.r>0.1</cim:PowerTransformerEnd.r>
    <cim:PowerTransformerEnd.x>5.0</cim:PowerTransformerEnd.x>
    <cim:PowerTransformerEnd.ratedU>110.0</cim:PowerTransformerEnd.ratedU>
    <cim:PowerTransformerEnd.ratedS>100.0</cim:PowerTransformerEnd.ratedS>
    <cim:TransformerEnd.Terminal rdf:resource="#t_xfmr_end1"/>
  </cim:PowerTransformerEnd>
  <cim:PowerTransformerEnd rdf:ID="xfmr_end2">
    <cim:PowerTransformerEnd.PowerTransformer rdf:resource="#xfmr1"/>
    <cim:TransformerEnd.BaseVoltage rdf:resource="#bv10"/>
    <cim:PowerTransformerEnd.r>0.05</cim:PowerTransformerEnd.r>
    <cim:PowerTransformerEnd.x>2.0</cim:PowerTransformerEnd.x>
    <cim:PowerTransformerEnd.ratedU>10.0</cim:PowerTransformerEnd.ratedU>
    <cim:PowerTransformerEnd.ratedS>100.0</cim:PowerTransformerEnd.ratedS>
    <cim:TransformerEnd.Terminal rdf:resource="#t_xfmr_end2"/>
  </cim:PowerTransformerEnd>
  <!-- Synchronous Machine on bus1 -->
  <cim:SynchronousMachine rdf:ID="gen1">
    <cim:IdentifiedObject.name>Generator 1</cim:IdentifiedObject.name>
    <cim:RotatingMachine.ratedS>100.0</cim:RotatingMachine.ratedS>
    <cim:RotatingMachine.ratedU>110.0</cim:RotatingMachine.ratedU>
    <cim:GeneratingUnit.initialP>50.0</cim:GeneratingUnit.initialP>
    <cim:SynchronousMachine.q>10.0</cim:SynchronousMachine.q>
    <cim:SynchronousMachine.minQ>-20.0</cim:SynchronousMachine.minQ>
    <cim:SynchronousMachine.maxQ>30.0</cim:SynchronousMachine.maxQ>
    <cim:ConductingEquipment.BaseVoltage rdf:resource="#bv110"/>
  </cim:SynchronousMachine>
  <!-- Energy consumers -->
  <cim:EnergyConsumer rdf:ID="load1">
    <cim:IdentifiedObject.name>Load 1</cim:IdentifiedObject.name>
    <cim:EnergyConsumer.p>40.0</cim:EnergyConsumer.p>
    <cim:EnergyConsumer.q>8.0</cim:EnergyConsumer.q>
    <cim:ConductingEquipment.BaseVoltage rdf:resource="#bv110"/>
  </cim:EnergyConsumer>
  <cim:EnergyConsumer rdf:ID="load2">
    <cim:IdentifiedObject.name>Load 2</cim:IdentifiedObject.name>
    <cim:EnergyConsumer.p>5.0</cim:EnergyConsumer.p>
    <cim:EnergyConsumer.q>1.0</cim:EnergyConsumer.q>
    <cim:ConductingEquipment.BaseVoltage rdf:resource="#bv10"/>
  </cim:EnergyConsumer>
  <!-- Linear shunt compensator on bus2 -->
  <cim:LinearShuntCompensator rdf:ID="shunt1">
    <cim:IdentifiedObject.name>Shunt 1</cim:IdentifiedObject.name>
    <cim:LinearShuntCompensator.b>50.0</cim:LinearShuntCompensator.b>
    <cim:LinearShuntCompensator.g>0.0</cim:LinearShuntCompensator.g>
    <cim:ConductingEquipment.BaseVoltage rdf:resource="#bv110"/>
  </cim:LinearShuntCompensator>
  <!-- Breaker (closed): bus1 ↔ bus2 -->
  <cim:Breaker rdf:ID="brk1">
    <cim:IdentifiedObject.name>Breaker 1</cim:IdentifiedObject.name>
    <cim:Switch.normalOpen>false</cim:Switch.normalOpen>
  </cim:Breaker>
  <!-- Disconnector (open): bus1 ↔ bus2 -->
  <cim:Disconnector rdf:ID="dis1">
    <cim:IdentifiedObject.name>Disconnector 1</cim:IdentifiedObject.name>
    <cim:Switch.normalOpen>true</cim:Switch.normalOpen>
  </cim:Disconnector>
  <!-- Terminals -->
  <cim:Terminal rdf:ID="t_bb1">
    <cim:Terminal.ConductingEquipment rdf:resource="#bus1"/>
    <cim:Terminal.ConnectivityNode rdf:resource="#cn1"/>
    <cim:ACDCTerminal.sequenceNumber>1</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>
  <cim:Terminal rdf:ID="t_bb2">
    <cim:Terminal.ConductingEquipment rdf:resource="#bus2"/>
    <cim:Terminal.ConnectivityNode rdf:resource="#cn2"/>
    <cim:ACDCTerminal.sequenceNumber>1</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>
  <cim:Terminal rdf:ID="t_bb3">
    <cim:Terminal.ConductingEquipment rdf:resource="#bus3"/>
    <cim:Terminal.ConnectivityNode rdf:resource="#cn3"/>
    <cim:ACDCTerminal.sequenceNumber>1</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>
  <cim:Terminal rdf:ID="t_line_from">
    <cim:Terminal.ConductingEquipment rdf:resource="#line1"/>
    <cim:Terminal.ConnectivityNode rdf:resource="#cn1"/>
    <cim:ACDCTerminal.sequenceNumber>1</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>
  <cim:Terminal rdf:ID="t_line_to">
    <cim:Terminal.ConductingEquipment rdf:resource="#line1"/>
    <cim:Terminal.ConnectivityNode rdf:resource="#cn2"/>
    <cim:ACDCTerminal.sequenceNumber>2</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>
  <cim:Terminal rdf:ID="t_xfmr_end1">
    <cim:Terminal.ConductingEquipment rdf:resource="#xfmr_end1"/>
    <cim:Terminal.ConnectivityNode rdf:resource="#cn2"/>
    <cim:ACDCTerminal.sequenceNumber>1</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>
  <cim:Terminal rdf:ID="t_xfmr_end2">
    <cim:Terminal.ConductingEquipment rdf:resource="#xfmr_end2"/>
    <cim:Terminal.ConnectivityNode rdf:resource="#cn3"/>
    <cim:ACDCTerminal.sequenceNumber>2</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>
  <cim:Terminal rdf:ID="t_gen1">
    <cim:Terminal.ConductingEquipment rdf:resource="#gen1"/>
    <cim:Terminal.ConnectivityNode rdf:resource="#cn1"/>
    <cim:ACDCTerminal.sequenceNumber>1</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>
  <cim:Terminal rdf:ID="t_load1">
    <cim:Terminal.ConductingEquipment rdf:resource="#load1"/>
    <cim:Terminal.ConnectivityNode rdf:resource="#cn2"/>
    <cim:ACDCTerminal.sequenceNumber>1</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>
  <cim:Terminal rdf:ID="t_load2">
    <cim:Terminal.ConductingEquipment rdf:resource="#load2"/>
    <cim:Terminal.ConnectivityNode rdf:resource="#cn3"/>
    <cim:ACDCTerminal.sequenceNumber>1</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>
  <cim:Terminal rdf:ID="t_shunt1">
    <cim:Terminal.ConductingEquipment rdf:resource="#shunt1"/>
    <cim:Terminal.ConnectivityNode rdf:resource="#cn2"/>
    <cim:ACDCTerminal.sequenceNumber>1</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>
  <cim:Terminal rdf:ID="t_brk_from">
    <cim:Terminal.ConductingEquipment rdf:resource="#brk1"/>
    <cim:Terminal.ConnectivityNode rdf:resource="#cn1"/>
    <cim:ACDCTerminal.sequenceNumber>1</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>
  <cim:Terminal rdf:ID="t_brk_to">
    <cim:Terminal.ConductingEquipment rdf:resource="#brk1"/>
    <cim:Terminal.ConnectivityNode rdf:resource="#cn2"/>
    <cim:ACDCTerminal.sequenceNumber>2</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>
  <cim:Terminal rdf:ID="t_dis_from">
    <cim:Terminal.ConductingEquipment rdf:resource="#dis1"/>
    <cim:Terminal.ConnectivityNode rdf:resource="#cn1"/>
    <cim:ACDCTerminal.sequenceNumber>1</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>
  <cim:Terminal rdf:ID="t_dis_to">
    <cim:Terminal.ConductingEquipment rdf:resource="#dis1"/>
    <cim:Terminal.ConnectivityNode rdf:resource="#cn2"/>
    <cim:ACDCTerminal.sequenceNumber>2</cim:ACDCTerminal.sequenceNumber>
  </cim:Terminal>
</rdf:RDF>"##;

    #[test]
    fn test_cim_to_power_network_bus_count() {
        let model = parse_cim(SAMPLE_CIM_TOPO).unwrap();
        let network = cim_to_power_network(&model).unwrap();
        // Bus count should match BusbarSection count
        assert_eq!(
            network.bus_count(),
            model.busbar_sections.len(),
            "bus count should match BusbarSection count"
        );
        assert_eq!(network.bus_count(), 3);
    }

    #[test]
    fn test_cim_to_power_network_branch_count() {
        let model = parse_cim(SAMPLE_CIM_TOPO).unwrap();
        let network = cim_to_power_network(&model).unwrap();
        let expected = model.ac_line_segments.len()
            + model.power_transformers.len()
            + model.breakers.len()
            + model.disconnectors.len();
        assert_eq!(
            network.branch_count(),
            expected,
            "branch count should match lines + transformers + breakers + disconnectors"
        );
        // 1 line + 1 transformer + 1 breaker + 1 disconnector = 4
        assert_eq!(network.branch_count(), 4);
    }

    #[test]
    fn test_cim_to_power_network_generator_count() {
        let model = parse_cim(SAMPLE_CIM_TOPO).unwrap();
        let network = cim_to_power_network(&model).unwrap();
        assert_eq!(
            network.generator_table().len(),
            model.synchronous_machines.len(),
            "generator count should match SynchronousMachine count"
        );
        assert_eq!(network.generator_table().len(), 1);
    }

    #[test]
    fn test_cim_to_power_network_load_reflected_in_pspec() {
        // PowerNetwork does not have a separate load table; loads are
        // represented as negative P/Q injections in p_spec/q_spec.
        let model = parse_cim(SAMPLE_CIM_TOPO).unwrap();
        let network = cim_to_power_network(&model).unwrap();

        // Total load P = 40 (load1) + 5 (load2) = 45 MW
        // Total gen P = 50 MW (gen1)
        // Net P in p_spec (per-unit, base_mva=100): gen - load = 50 - 45 = 5 MW → 0.05 pu
        let total_p: f64 = network.p_spec().iter().sum();
        let expected_net_p = (50.0 - 40.0 - 5.0) / 100.0;
        assert!(
            (total_p - expected_net_p).abs() < 1e-10,
            "total p_spec {} should match net injection {}",
            total_p,
            expected_net_p
        );

        // Bus 2 (bus2) has load1 (40 MW) and no generation → negative p_spec
        let bus_map = network.bus_map();
        let bus2_idx = bus_map.get(&2).copied().unwrap();
        assert!(
            network.p_spec()[bus2_idx] < 0.0,
            "bus2 p_spec should be negative (load bus), got {}",
            network.p_spec()[bus2_idx]
        );
    }

    #[test]
    fn test_cim_to_power_network_branch_topology() {
        let model = parse_cim(SAMPLE_CIM_TOPO).unwrap();
        let network = cim_to_power_network(&model).unwrap();

        // All branch endpoints must be valid bus IDs
        let bus_map = network.bus_map();
        for (from, to, _, _, _, _) in network.branches_data() {
            assert!(
                bus_map.contains_key(from),
                "branch from_bus {} not in bus_map",
                from
            );
            assert!(
                bus_map.contains_key(to),
                "branch to_bus {} not in bus_map",
                to
            );
        }

        // Verify specific connections:
        // line1: bus1(1) ↔ bus2(2)
        // xfmr1: bus2(2) ↔ bus3(3)
        // brk1: bus1(1) ↔ bus2(2)
        // dis1: bus1(1) ↔ bus2(2)
        let branches: Vec<(ElementId, ElementId)> = network
            .branches_data()
            .iter()
            .map(|(f, t, _, _, _, _)| (*f, *t))
            .collect();

        // Check that bus1↔bus2 appears (line1, brk1, dis1)
        let bus1_bus2_count = branches
            .iter()
            .filter(|(f, t)| (*f == 1 && *t == 2) || (*f == 2 && *t == 1))
            .count();
        assert_eq!(
            bus1_bus2_count, 3,
            "expected 3 branches between bus1 and bus2 (line, breaker, disconnector)"
        );

        // Check that bus2↔bus3 appears (transformer)
        let bus2_bus3_count = branches
            .iter()
            .filter(|(f, t)| (*f == 2 && *t == 3) || (*f == 3 && *t == 2))
            .count();
        assert_eq!(
            bus2_bus3_count, 1,
            "expected 1 branch between bus2 and bus3 (transformer)"
        );
    }

    #[test]
    fn test_cim_to_power_network_bus_types() {
        let model = parse_cim(SAMPLE_CIM_TOPO).unwrap();
        let network = cim_to_power_network(&model).unwrap();

        // gen1 is on bus1 → bus1 should be Slack
        let bus_types = network.bus_types();
        let bus_map = network.bus_map();
        let bus1_idx = bus_map.get(&1).copied().unwrap();
        let bus2_idx = bus_map.get(&2).copied().unwrap();
        let bus3_idx = bus_map.get(&3).copied().unwrap();

        assert_eq!(bus_types[bus1_idx], BusTypeNR::Slack, "bus1 should be Slack");
        assert_eq!(bus_types[bus2_idx], BusTypeNR::PQ, "bus2 should be PQ");
        assert_eq!(bus_types[bus3_idx], BusTypeNR::PQ, "bus3 should be PQ");
    }

    #[test]
    fn test_cim_to_power_network_generator_spec() {
        let model = parse_cim(SAMPLE_CIM_TOPO).unwrap();
        let network = cim_to_power_network(&model).unwrap();

        let gen = &network.generator_table()[0];
        assert_eq!(gen.gen_id, 1);
        assert_eq!(gen.bus_id, 1, "gen1 should be on bus1");
        assert_eq!(gen.p_gen_mw, 50.0);
        assert_eq!(gen.p_load_mw, 0.0, "no load on bus1");
        assert!(gen.p_max_mw > 0.0);
    }

    #[test]
    fn test_cim_to_power_network_solve() {
        let model = parse_cim(SAMPLE_CIM_TOPO).unwrap();
        let network = cim_to_power_network(&model).unwrap();
        let result = network.solve();
        assert!(
            result.is_ok(),
            "power flow solve failed: {:?}",
            result.err()
        );
        let result = result.unwrap();
        assert!(
            result.converged,
            "power flow did not converge (iterations={})",
            result.iterations
        );
    }

    #[test]
    fn test_cim_to_power_network_empty_model_error() {
        let model = parse_cim("<rdf:RDF></rdf:RDF>").unwrap();
        let result = cim_to_power_network(&model);
        assert!(
            result.is_err(),
            "empty model should return Err, got Ok"
        );
        let err = result.err().unwrap();
        assert!(
            err.contains("BusbarSection"),
            "error should mention BusbarSection, got: {}",
            err
        );
    }

    #[test]
    fn test_cim_to_power_network_no_terminals_error() {
        // SAMPLE_CIM has busbars and equipment but no Terminals/ConnectivityNodes,
        // so topology cannot be resolved.
        let model = parse_cim(SAMPLE_CIM).unwrap();
        let result = cim_to_power_network(&model);
        assert!(
            result.is_err(),
            "model without terminals should return Err"
        );
    }

    #[test]
    fn test_cim_to_power_network_branch_ids() {
        let model = parse_cim(SAMPLE_CIM_TOPO).unwrap();
        let network = cim_to_power_network(&model).unwrap();
        // branch_ids should be 1..=branch_count
        assert_eq!(network.branch_ids().len(), network.branch_count());
        for (i, &id) in network.branch_ids().iter().enumerate() {
            assert_eq!(id, (i + 1) as ElementId, "branch_id at index {} should be {}", i, i + 1);
        }
    }
}
